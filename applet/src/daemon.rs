//! Conversa com o daemon do Foco pelo socket de controle e abre os outros programas do Foco.
//! O applet não guarda estado próprio: o timer, o overlay e o bloqueio são todos do daemon.

use std::ffi::OsStr;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Status {
    pub phase: String,
    pub phase_name: String,
    pub state: String,
    pub remaining_ms: u64,
    pub total_ms: u64,
    pub color: String,
    pub done_in_cycle: u32,
    pub every: u32,
    pub strict_locked: bool,
    pub blocking: bool,
    pub overlay: Overlay,
    pub today: Today,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Overlay {
    pub visible: bool,
    pub locked: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Today {
    pub count: u32,
    pub minutes: u32,
}

impl Status {
    pub fn running(&self) -> bool {
        self.state == "running"
    }

    pub fn paused(&self) -> bool {
        self.state == "paused"
    }

    pub fn idle(&self) -> bool {
        !self.running() && !self.paused()
    }
}

/// Resultado de um pedido: o estado (`None` = o Foco não está rodando) e o erro do comando.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: Option<Status>,
    pub error: Option<String>,
}

fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(v) if !v.is_empty() => PathBuf::from(v).join("foco.sock"),
        _ => {
            let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0);
            PathBuf::from(format!("/tmp/foco-{uid}.sock"))
        }
    }
}

async fn request(line: &str) -> std::io::Result<serde_json::Value> {
    let exchange = async {
        let mut s = UnixStream::connect(socket_path()).await?;
        s.write_all(format!("{line}\n").as_bytes()).await?;
        let mut out = String::new();
        BufReader::new(s).read_line(&mut out).await?;
        serde_json::from_str(out.trim()).map_err(std::io::Error::other)
    };
    tokio::time::timeout(Duration::from_secs(4), exchange)
        .await
        .map_err(|_| std::io::Error::other("sem resposta"))?
}

/// Manda o comando (se houver) e depois pede o estado.
pub async fn send(cmd: Option<String>) -> Reply {
    let mut error = None;
    if let Some(cmd) = cmd {
        match request(&cmd).await {
            Ok(v) if v["ok"] == true => {}
            Ok(v) => error = Some(v["error"].as_str().unwrap_or("o comando falhou").to_string()),
            Err(_) => return Reply { status: None, error: None },
        }
    }
    let status = request("status")
        .await
        .ok()
        .filter(|v| v["ok"] == true)
        .and_then(|v| serde_json::from_value(v).ok());
    Reply { status, error }
}

/// Programa do Foco instalado ao lado do applet (ou em ~/.local/bin).
fn program(name: &str) -> PathBuf {
    let beside = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join(name)));
    let home = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin").join(name));
    beside
        .into_iter()
        .chain(home)
        .find(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Comando que roda fora do painel: o painel passa ao applet um socket Wayland só dele
/// (WAYLAND_SOCKET), que não serve para os outros programas.
fn outside_panel(program: impl AsRef<OsStr>) -> Command {
    let mut c = Command::new(program);
    for (key, _) in std::env::vars_os() {
        let k = key.to_string_lossy();
        if k == "WAYLAND_SOCKET" || k == "X_PRIVILEGED_WAYLAND_SOCKET" || k.starts_with("COSMIC_PANEL_") {
            c.env_remove(&key);
        }
    }
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // grupo próprio: o daemon continua vivo se o painel reiniciar o applet
    c.process_group(0);
    c
}

fn spawn(mut cmd: Command) -> bool {
    match cmd.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            true
        }
        Err(_) => false,
    }
}

pub fn start_daemon() -> bool {
    spawn(outside_panel(program("foco")))
}

pub fn open_settings(token: Option<String>) -> bool {
    let mut c = outside_panel(program("foco-settings"));
    if let Some(t) = token {
        c.env("XDG_ACTIVATION_TOKEN", &t).env("DESKTOP_STARTUP_ID", &t);
    }
    spawn(c)
}
