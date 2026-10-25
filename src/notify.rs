//! Notificações (notify-send) e sons (pw-play, com paplay de reserva).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Roda o comando sem esperar, mas recolhe o processo numa thread (sem zumbis).
pub fn spawn_detached(mut cmd: Command) -> bool {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
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

pub fn notify(title: &str, body: &str) {
    let mut c = Command::new("notify-send");
    c.args(["-a", "Foco", "-i", "foco", "-h", "string:x-canonical-private-synchronous:foco", title, body]);
    if !spawn_detached(c) {
        eprintln!("foco: {title}: {body}");
    }
}

pub fn sound(name: &str, volume: f32) {
    let file = format!("/usr/share/sounds/freedesktop/stereo/{name}.oga");
    if !Path::new(&file).exists() {
        return;
    }
    let mut pw = Command::new("pw-play");
    pw.arg(format!("--volume={:.2}", volume.clamp(0.0, 1.0))).arg(&file);
    if !spawn_detached(pw) {
        let mut pa = Command::new("paplay");
        pa.arg(format!("--volume={}", (volume.clamp(0.0, 1.0) * 65536.0) as u32)).arg(&file);
        spawn_detached(pa);
    }
}

/// Abre a central do Foco (instalada ao lado do binário, ou no repositório). O token de
/// ativação, quando há, deixa a janela vir para a frente já com o foco.
pub fn open_settings(token: Option<&str>, args: &[&str]) -> bool {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(p) = std::env::var_os("FOCO_SETTINGS") {
        candidates.push(PathBuf::from(p));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        candidates.push(dir.join("foco-settings"));
        // rodando de dentro do repositório (target/debug ou target/release)
        candidates.push(dir.join("../../settings/foco-settings.py"));
    }
    candidates.push(crate::config::home().join(".local/bin/foco-settings"));
    match candidates.into_iter().find(|p| p.exists()) {
        Some(path) => {
            let mut cmd = Command::new(path);
            cmd.args(args);
            if let Some(t) = token {
                cmd.env("XDG_ACTIVATION_TOKEN", t).env("DESKTOP_STARTUP_ID", t);
            }
            spawn_detached(cmd)
        }
        None => {
            notify("Foco", "Não achei o foco-settings. Rode o install.sh de novo.");
            false
        }
    }
}
