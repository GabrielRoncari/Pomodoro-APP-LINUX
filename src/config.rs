use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::{Path, PathBuf};

use crate::timer::Timer;

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(fallback),
    }
}

pub fn config_path() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join("foco/config.json")
}

pub fn state_path() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("foco/state.json")
}

pub fn log_path() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("foco/foco.log")
}

/// Registra no foco.log: o daemon roda sem terminal e o stderr se perde.
pub fn log(msg: &str) {
    use std::io::Write;
    eprintln!("foco: {msg}");
    let path = log_path();
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 256 * 1024) {
        let _ = std::fs::rename(&path, path.with_extension("log.old"));
    }
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new("/tmp")));
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} [{}] {msg}", local_time(), std::process::id());
    }
}

fn local_time() -> String {
    // SAFETY: time/localtime_r só leem e escrevem nas variáveis locais passadas.
    let tm = unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm
    };
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

pub fn history_path() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share").join("foco/history.jsonl")
}

pub fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(v) if !v.is_empty() => PathBuf::from(v).join("foco.sock"),
        _ => PathBuf::from(format!("/tmp/foco-{}.sock", unsafe { libc::getuid() })),
    }
}

pub const SOCIAL: &[&str] = &[
    "instagram.com",
    "facebook.com",
    "tiktok.com",
    "x.com",
    "twitter.com",
    "reddit.com",
    "threads.com",
    "threads.net",
];

/// Preferências do usuário. Quem escreve este arquivo é a tela de configurações;
/// o daemon só lê (e cria com os padrões se não existir).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Config {
    pub focus_min: u32,
    pub short_break_min: u32,
    pub long_break_min: u32,
    pub long_break_every: u32,
    pub auto_start_breaks: bool,
    pub auto_start_focus: bool,
    pub strict_mode: bool,
    pub appearance: Appearance,
    pub block: Block,
    pub alerts: Alerts,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Appearance {
    pub focus_color: String,
    pub short_break_color: String,
    pub long_break_color: String,
    /// Opacidade do fundo do disco.
    pub background_opacity: f32,
    /// Opacidade do overlay inteiro quando o mouse não está em cima.
    pub idle_opacity: f32,
    /// Camada overlay (fica por cima até de tela cheia) em vez de top.
    pub above_fullscreen: bool,
    /// Ícone na bandeja do sistema (dispensável com o miniaplicativo no painel).
    pub tray_icon: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Block {
    pub enabled: bool,
    pub sites: Vec<String>,
    /// Derruba conexões já abertas com os sites bloqueados (abas abertas param na hora).
    pub kill_connections: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Alerts {
    pub notifications: bool,
    pub sound: bool,
    pub volume: f32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            focus_min: 25,
            short_break_min: 5,
            long_break_min: 15,
            long_break_every: 4,
            auto_start_breaks: true,
            auto_start_focus: false,
            strict_mode: false,
            appearance: Appearance::default(),
            block: Block::default(),
            alerts: Alerts::default(),
        }
    }
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            focus_color: "#ff6b5e".into(),
            short_break_color: "#3fd8a0".into(),
            long_break_color: "#62a8ff".into(),
            background_opacity: 0.86,
            idle_opacity: 0.94,
            above_fullscreen: true,
            tray_icon: true,
        }
    }
}

impl Default for Block {
    fn default() -> Self {
        Block { enabled: true, sites: SOCIAL.iter().map(|s| s.to_string()).collect(), kill_connections: true }
    }
}

impl Default for Alerts {
    fn default() -> Self {
        Alerts { notifications: true, sound: true, volume: 0.8 }
    }
}

impl Config {
    /// Lê o arquivo; se não existir grava os padrões. Arquivo quebrado não é sobrescrito.
    pub fn load() -> Config {
        let path = config_path();
        match read_json::<Config>(&path) {
            Ok(Some(cfg)) => cfg.sanitized(),
            Ok(None) => {
                let cfg = Config::default();
                if let Err(e) = write_json(&path, &cfg) {
                    eprintln!("foco: não consegui criar {}: {e}", path.display());
                }
                cfg
            }
            Err(e) => {
                eprintln!("foco: {} inválido ({e}); usando padrões", path.display());
                Config::default()
            }
        }
    }

    fn sanitized(mut self) -> Config {
        self.focus_min = self.focus_min.clamp(1, 600);
        self.short_break_min = self.short_break_min.clamp(1, 240);
        self.long_break_min = self.long_break_min.clamp(1, 240);
        self.long_break_every = self.long_break_every.clamp(1, 12);
        let a = &mut self.appearance;
        a.background_opacity = a.background_opacity.clamp(0.0, 1.0);
        a.idle_opacity = a.idle_opacity.clamp(0.15, 1.0);
        self.alerts.volume = self.alerts.volume.clamp(0.0, 1.0);
        self.block.sites = normalize_sites(&self.block.sites);
        self
    }
}

/// Deixa só domínios: tira esquema, caminho, porta, "www." e repetidos.
pub fn normalize_sites(sites: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in sites {
        let mut s = raw.trim().to_ascii_lowercase();
        if let Some(i) = s.find("://") {
            s = s[i + 3..].to_string();
        }
        if let Some(i) = s.find(['/', '?', '#']) {
            s.truncate(i);
        }
        if let Some(i) = s.rfind(':') {
            s.truncate(i);
        }
        let s = s.trim_start_matches("*.").trim_start_matches("www.").trim_end_matches('.');
        let valid = s.contains('.')
            && s.len() <= 253
            && s.split('.').all(|l| {
                !l.is_empty()
                    && l.len() <= 63
                    && !l.starts_with('-')
                    && !l.ends_with('-')
                    && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            });
        if valid && !out.iter().any(|o| o == s) {
            out.push(s.to_string());
        }
    }
    out
}

/// Estado que só o daemon escreve: timer e posição/tamanho do overlay.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Persist {
    pub timer: Timer,
    pub ui: Ui,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Ui {
    pub visible: bool,
    /// Clique atravessa o overlay (não dá para arrastar).
    pub locked: bool,
    pub size: u32,
    pub x: i32,
    pub y: i32,
    /// Falso até o overlay ganhar uma posição (padrão: canto superior direito).
    pub positioned: bool,
    pub output: String,
}

impl Default for Ui {
    fn default() -> Self {
        Ui { visible: true, locked: false, size: 190, x: 0, y: 0, positioned: false, output: String::new() }
    }
}

pub const MIN_SIZE: u32 = 90;
pub const MAX_SIZE: u32 = 640;

impl Persist {
    pub fn load() -> Persist {
        match read_json::<Persist>(&state_path()) {
            Ok(Some(mut p)) => {
                p.ui.size = p.ui.size.clamp(MIN_SIZE, MAX_SIZE);
                p
            }
            Ok(None) => Persist::default(),
            Err(e) => {
                eprintln!("foco: estado inválido ({e}); recomeçando");
                Persist::default()
            }
        }
    }

    pub fn save(&self) {
        if let Err(e) = write_json(&state_path(), self) {
            eprintln!("foco: não consegui salvar o estado: {e}");
        }
    }
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, text + "\n")?;
    std::fs::rename(&tmp, path)
}

pub fn parse_color(hex: &str) -> [u8; 3] {
    let h = hex.trim().trim_start_matches('#');
    if h.len() == 6 {
        if let Ok(v) = u32::from_str_radix(h, 16) {
            return [(v >> 16) as u8, (v >> 8) as u8, v as u8];
        }
    }
    [255, 107, 94]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites_are_normalized() {
        let raw: Vec<String> = [
            "https://www.Instagram.com/explore/",
            "youtube.com:443",
            "*.tiktok.com",
            "instagram.com",
            "localhost",
            "bad_domain.com",
            "-x.com",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(normalize_sites(&raw), vec!["instagram.com", "youtube.com", "tiktok.com"]);
    }

    #[test]
    fn partial_config_uses_defaults() {
        let cfg: Config = serde_json::from_str(r#"{"focus_min": 50, "block": {"enabled": false}}"#).unwrap();
        assert_eq!(cfg.focus_min, 50);
        assert_eq!(cfg.short_break_min, 5);
        assert!(!cfg.block.enabled);
        assert!(cfg.block.kill_connections);
    }
}
