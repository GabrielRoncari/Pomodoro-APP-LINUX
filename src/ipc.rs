//! Controle por socket Unix: uma linha de comando, uma linha de JSON de resposta.
//! Serve para a tela de configurações, para atalhos de teclado e para scripts.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc;
use std::time::Duration;

use smithay_client_toolkit::reexports::calloop::channel::Sender;

use crate::config::socket_path;
use crate::timer::Phase;

#[derive(Debug, Clone)]
pub enum Cmd {
    Toggle,
    Start,
    Pause,
    Skip,
    Reset,
    ResetCycle,
    Phase(Phase),
    Overlay(Option<bool>),
    Lock(Option<bool>),
    Size(u32),
    Output(String),
    Reload,
    Settings,
    Status,
    Quit,
    /// Interno: o helper do bloqueio falhou.
    BlockError(String),
}

pub struct Msg {
    pub cmd: Cmd,
    pub reply: Option<mpsc::Sender<String>>,
}

pub const HELP: &str = "\
uso: foco [comando]

sem comando        inicia o Foco (overlay + bandeja)
toggle             inicia/pausa
start | pause      inicia ou continua / pausa
skip               pula para a próxima etapa
reset              volta a etapa atual ao início
reset-cycle        zera o ciclo e volta para o foco
phase focus|short|long
show | hide        mostra/esconde o overlay
overlay [on|off]   alterna o overlay
lock [on|off]      trava o overlay (cliques atravessam)
size <px>          tamanho do overlay
output <nome>      monitor do overlay (ex.: DP-1)
settings           abre as configurações
status [--json]    estado atual
reload             relê a configuração
quit               encerra";

fn on_off(arg: Option<&str>) -> Result<Option<bool>, String> {
    match arg {
        None | Some("toggle") => Ok(None),
        Some("on" | "1" | "true" | "show") => Ok(Some(true)),
        Some("off" | "0" | "false" | "hide") => Ok(Some(false)),
        Some(x) => Err(format!("valor inválido: {x}")),
    }
}

pub fn parse(line: &str) -> Result<Cmd, String> {
    let mut it = line.split_whitespace();
    let name = it.next().unwrap_or("");
    let arg = it.next();
    Ok(match name {
        "toggle" => Cmd::Toggle,
        "start" | "resume" => Cmd::Start,
        "pause" => Cmd::Pause,
        "skip" => Cmd::Skip,
        "reset" => Cmd::Reset,
        "reset-cycle" => Cmd::ResetCycle,
        "phase" => Cmd::Phase(arg.and_then(Phase::parse).ok_or("uso: phase focus|short|long")?),
        "show" => Cmd::Overlay(Some(true)),
        "hide" => Cmd::Overlay(Some(false)),
        "overlay" => Cmd::Overlay(on_off(arg)?),
        "lock" => Cmd::Lock(on_off(arg)?),
        "size" => Cmd::Size(arg.and_then(|a| a.parse().ok()).ok_or("uso: size <px>")?),
        "output" => Cmd::Output(arg.unwrap_or("").to_string()),
        "reload" => Cmd::Reload,
        "settings" => Cmd::Settings,
        "status" => Cmd::Status,
        "quit" => Cmd::Quit,
        _ => return Err(format!("comando desconhecido: {name}")),
    })
}

fn error_json(msg: &str) -> String {
    serde_json::json!({ "ok": false, "error": msg }).to_string()
}

/// Atende conexões numa thread; cada comando vai para o laço principal pelo canal.
pub fn serve(listener: UnixListener, tx: Sender<Msg>) {
    std::thread::Builder::new()
        .name("foco-ipc".into())
        .spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut stream) = conn else { continue };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut line = String::new();
                if BufReader::new(&stream).read_line(&mut line).is_err() {
                    continue;
                }
                let resp = match parse(line.trim()) {
                    Err(e) => error_json(&e),
                    Ok(cmd) => {
                        let (rtx, rrx) = mpsc::channel();
                        if tx.send(Msg { cmd, reply: Some(rtx) }).is_err() {
                            break;
                        }
                        rrx.recv_timeout(Duration::from_secs(3)).unwrap_or_else(|_| error_json("sem resposta"))
                    }
                };
                let _ = writeln!(stream, "{resp}");
            }
        })
        .expect("thread do socket");
}

pub fn request(line: &str) -> std::io::Result<String> {
    let mut s = UnixStream::connect(socket_path())?;
    s.set_read_timeout(Some(Duration::from_secs(4)))?;
    writeln!(s, "{line}")?;
    let mut out = String::new();
    BufReader::new(&s).read_line(&mut out)?;
    Ok(out.trim().to_string())
}

/// `foco <comando>`: fala com o daemon que já está rodando.
pub fn client(args: &[String]) -> i32 {
    let json_out = args.iter().any(|a| a == "--json");
    let line = args.iter().filter(|a| *a != "--json").cloned().collect::<Vec<_>>().join(" ");
    if let Err(e) = parse(&line) {
        eprintln!("{e}\n\n{HELP}");
        return 2;
    }
    let resp = match request(&line) {
        Ok(r) => r,
        Err(_) => {
            eprintln!("o Foco não está rodando (inicie com: foco)");
            return 1;
        }
    };
    let v: serde_json::Value = serde_json::from_str(&resp).unwrap_or_default();
    if v["ok"] != true {
        eprintln!("{}", v["error"].as_str().unwrap_or(&resp));
        return 1;
    }
    if line.starts_with("status") {
        if json_out {
            println!("{resp}");
        } else {
            let state = match v["state"].as_str() {
                Some("running") => "",
                Some("paused") => " (pausado)",
                _ => " (parado)",
            };
            let today = v["today"]["count"].as_u64().unwrap_or(0);
            println!(
                "{} {}{} · ciclo {}/{} · hoje {} {}{}",
                v["phase_name"].as_str().unwrap_or("?"),
                v["clock"].as_str().unwrap_or("?"),
                state,
                v["done_in_cycle"],
                v["every"],
                today,
                if today == 1 { "foco" } else { "focos" },
                if v["blocking"] == true { " · sites bloqueados" } else { "" },
            );
        }
    }
    0
}
