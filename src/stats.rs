//! Histórico de focos concluídos (um JSON por linha), usado para "hoje: N focos".

use serde::{Deserialize, Serialize};
use std::io::Write;

use crate::config::history_path;

#[derive(Serialize, Deserialize)]
struct Entry {
    ts: u64,
    phase: String,
    minutes: u32,
}

#[derive(Default)]
pub struct Stats {
    focus: Vec<(u64, u32)>,
}

impl Stats {
    pub fn load() -> Stats {
        let text = std::fs::read_to_string(history_path()).unwrap_or_default();
        let focus = text
            .lines()
            .filter_map(|l| serde_json::from_str::<Entry>(l).ok())
            .filter(|e| e.phase == "focus")
            .map(|e| (e.ts, e.minutes))
            .collect();
        Stats { focus }
    }

    pub fn record_focus(&mut self, minutes: u32) {
        let ts = crate::timer::now_ms() / 1000;
        self.focus.push((ts, minutes));
        let line = serde_json::to_string(&Entry { ts, phase: "focus".into(), minutes }).unwrap_or_default();
        let path = history_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let res = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut f| writeln!(f, "{line}"));
        if let Err(e) = res {
            eprintln!("foco: não consegui gravar o histórico: {e}");
        }
    }

    /// (focos, minutos) desde a meia-noite local.
    pub fn today(&self) -> (u32, u32) {
        let start = local_midnight(crate::timer::now_ms() / 1000);
        self.focus
            .iter()
            .filter(|(ts, _)| *ts >= start)
            .fold((0, 0), |(n, m), (_, min)| (n + 1, m + min))
    }
}

fn local_midnight(now: u64) -> u64 {
    // SAFETY: localtime_r/mktime só leem e escrevem nas structs locais passadas.
    unsafe {
        let t = now as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        tm.tm_hour = 0;
        tm.tm_min = 0;
        tm.tm_sec = 0;
        tm.tm_isdst = -1;
        libc::mktime(&mut tm).max(0) as u64
    }
}
