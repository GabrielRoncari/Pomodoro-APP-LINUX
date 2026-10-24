//! Bloqueio de sites: um helper root (instalado uma vez) reescreve um bloco marcado do
//! /etc/hosts. O polkit libera o helper sem senha para a sessão ativa.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub const HELPER: &str = "/usr/local/libexec/foco-hosts";
pub const POLICY: &str = "/usr/share/polkit-1/actions/dev.foco.hosts.policy";
const HOSTS: &str = "/etc/hosts";
const SITES_TAG: &str = "# foco-sites:";

pub fn installed() -> bool {
    Path::new(HELPER).exists() && Path::new(POLICY).exists()
}

/// Sites bloqueados agora segundo o /etc/hosts (None = sem bloco do Foco).
pub fn current() -> Option<Vec<String>> {
    let text = std::fs::read_to_string(HOSTS).ok()?;
    text.lines()
        .find_map(|l| l.strip_prefix(SITES_TAG))
        .map(|rest| rest.split_whitespace().map(String::from).collect())
}

enum Job {
    Apply(Option<Vec<String>>, bool),
    Flush(mpsc::Sender<()>),
}

pub struct Blocker {
    tx: mpsc::Sender<Job>,
    applied: Option<Vec<String>>,
}

impl Blocker {
    /// `on_error` roda na thread do bloqueador.
    pub fn new(on_error: impl Fn(String) + Send + 'static) -> Blocker {
        let (tx, rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("foco-bloqueio".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    match job {
                        Job::Apply(sites, kill) => {
                            if let Err(e) = run_helper(sites.as_deref(), kill) {
                                on_error(e);
                            }
                        }
                        Job::Flush(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            })
            .expect("thread do bloqueio");
        Blocker { tx, applied: current() }
    }

    pub fn applied(&self) -> Option<&Vec<String>> {
        self.applied.as_ref()
    }

    pub fn active(&self) -> bool {
        self.applied.is_some()
    }

    /// Relê o /etc/hosts (depois de uma falha do helper, o que pedimos pode não valer).
    pub fn resync(&mut self) {
        self.applied = current();
    }

    pub fn set(&mut self, want: Option<Vec<String>>, kill: bool) {
        if want == self.applied {
            return;
        }
        if !installed() {
            // sem helper não há o que aplicar nem limpar (e o pkexec pediria senha)
            return;
        }
        self.applied = want.clone();
        let _ = self.tx.send(Job::Apply(want, kill));
    }

    /// Espera os trabalhos pendentes terminarem (usado ao sair).
    pub fn flush(&self, timeout: Duration) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Job::Flush(tx)).is_ok() {
            let _ = rx.recv_timeout(timeout);
        }
    }
}

fn run_helper(sites: Option<&[String]>, kill: bool) -> Result<(), String> {
    let mut cmd = Command::new("pkexec");
    cmd.arg(HELPER);
    match sites {
        Some(_) => {
            cmd.arg("block");
            if kill {
                cmd.arg("--kill");
            }
        }
        None => {
            cmd.arg("clear");
        }
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("não consegui rodar o pkexec: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        for s in sites.unwrap_or_default() {
            let _ = writeln!(stdin, "{s}");
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(match out.status.code() {
        Some(126) => "a autorização foi negada".into(),
        Some(127) => "o polkit não autorizou o helper (reinstale o bloqueio)".into(),
        _ if !stderr.is_empty() => stderr,
        _ => format!("o helper saiu com {}", out.status),
    })
}
