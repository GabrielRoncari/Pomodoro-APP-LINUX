use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::Config;

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Focus,
    ShortBreak,
    LongBreak,
}

impl Phase {
    pub fn minutes(self, cfg: &Config) -> u32 {
        match self {
            Phase::Focus => cfg.focus_min,
            Phase::ShortBreak => cfg.short_break_min,
            Phase::LongBreak => cfg.long_break_min,
        }
    }

    /// Rótulo curto, em caixa alta, desenhado no overlay.
    pub fn label(self) -> &'static str {
        match self {
            Phase::Focus => "FOCO",
            Phase::ShortBreak => "PAUSA",
            Phase::LongBreak => "PAUSA LONGA",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Phase::Focus => "Foco",
            Phase::ShortBreak => "Pausa curta",
            Phase::LongBreak => "Pausa longa",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Phase::Focus => "focus",
            Phase::ShortBreak => "short_break",
            Phase::LongBreak => "long_break",
        }
    }

    pub fn parse(s: &str) -> Option<Phase> {
        match s {
            "focus" | "foco" => Some(Phase::Focus),
            "short" | "short_break" | "pausa" => Some(Phase::ShortBreak),
            "long" | "long_break" | "pausa-longa" => Some(Phase::LongBreak),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Run {
    #[default]
    Idle,
    Running {
        ends_at_ms: u64,
    },
    Paused {
        remaining_ms: u64,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub enum RunKind {
    #[default]
    Idle,
    Running,
    Paused,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Timer {
    pub phase: Phase,
    pub run: Run,
    /// Focos concluídos no ciclo atual (zera depois da pausa longa).
    pub done_in_cycle: u32,
    /// Duração da etapa em andamento, fixada no início dela.
    pub total_ms: u64,
}

impl Timer {
    pub fn kind(&self) -> RunKind {
        match self.run {
            Run::Idle => RunKind::Idle,
            Run::Running { .. } => RunKind::Running,
            Run::Paused { .. } => RunKind::Paused,
        }
    }

    pub fn duration_ms(&self, cfg: &Config) -> u64 {
        match self.run {
            Run::Idle => self.phase.minutes(cfg) as u64 * 60_000,
            _ => self.total_ms.max(1),
        }
    }

    pub fn remaining_ms(&self, now: u64, cfg: &Config) -> u64 {
        match self.run {
            Run::Idle => self.duration_ms(cfg),
            Run::Running { ends_at_ms } => ends_at_ms.saturating_sub(now),
            Run::Paused { remaining_ms } => remaining_ms,
        }
    }

    /// Fração restante (1.0 = cheio, 0.0 = acabou).
    pub fn fraction(&self, now: u64, cfg: &Config) -> f32 {
        let total = self.duration_ms(cfg) as f32;
        (self.remaining_ms(now, cfg) as f32 / total).clamp(0.0, 1.0)
    }

    /// Foco rodando com modo rígido: nada de pausar, pular ou reiniciar.
    pub fn strict_locked(&self, cfg: &Config) -> bool {
        cfg.strict_mode && self.phase == Phase::Focus && matches!(self.run, Run::Running { .. })
    }

    pub fn start(&mut self, now: u64, cfg: &Config) {
        match self.run {
            Run::Idle => {
                self.total_ms = self.phase.minutes(cfg) as u64 * 60_000;
                self.run = Run::Running { ends_at_ms: now + self.total_ms };
            }
            Run::Paused { remaining_ms } => {
                self.run = Run::Running { ends_at_ms: now + remaining_ms };
            }
            Run::Running { .. } => {}
        }
    }

    pub fn pause(&mut self, now: u64) {
        if let Run::Running { ends_at_ms } = self.run {
            self.run = Run::Paused { remaining_ms: ends_at_ms.saturating_sub(now) };
        }
    }

    pub fn toggle(&mut self, now: u64, cfg: &Config) {
        match self.run {
            Run::Running { .. } => self.pause(now),
            _ => self.start(now, cfg),
        }
    }

    pub fn reset(&mut self) {
        self.run = Run::Idle;
    }

    pub fn go_to(&mut self, phase: Phase) {
        self.phase = phase;
        self.run = Run::Idle;
    }

    /// Vai para a próxima etapa. `completed` diz se o foco atual conta no ciclo.
    pub fn advance(&mut self, cfg: &Config, completed: bool) -> Phase {
        let every = cfg.long_break_every.max(1);
        let next = match self.phase {
            Phase::Focus => {
                if completed {
                    self.done_in_cycle += 1;
                }
                if self.done_in_cycle >= every { Phase::LongBreak } else { Phase::ShortBreak }
            }
            Phase::ShortBreak => Phase::Focus,
            Phase::LongBreak => {
                self.done_in_cycle = 0;
                Phase::Focus
            }
        };
        self.phase = next;
        self.run = Run::Idle;
        next
    }
}

/// Contagem regressiva em MM:SS, arredondando para cima (25:00 no primeiro segundo).
pub fn format_clock(ms: u64) -> String {
    let secs = ms.div_ceil(1000);
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_goes_to_long_break() {
        let cfg = Config::default();
        let mut t = Timer::default();
        for _ in 0..cfg.long_break_every - 1 {
            assert_eq!(t.advance(&cfg, true), Phase::ShortBreak);
            assert_eq!(t.advance(&cfg, true), Phase::Focus);
        }
        assert_eq!(t.advance(&cfg, true), Phase::LongBreak);
        assert_eq!(t.advance(&cfg, true), Phase::Focus);
        assert_eq!(t.done_in_cycle, 0);
    }

    #[test]
    fn skipped_focus_does_not_count() {
        let cfg = Config::default();
        let mut t = Timer::default();
        t.advance(&cfg, false);
        assert_eq!(t.done_in_cycle, 0);
    }

    #[test]
    fn pause_resume_keeps_remaining() {
        let cfg = Config::default();
        let mut t = Timer::default();
        t.start(1_000, &cfg);
        t.pause(61_000);
        assert_eq!(t.remaining_ms(999_999, &cfg), 25 * 60_000 - 60_000);
        t.start(100_000, &cfg);
        assert_eq!(t.remaining_ms(100_000, &cfg), 25 * 60_000 - 60_000);
    }

    #[test]
    fn clock_rounds_up() {
        assert_eq!(format_clock(25 * 60_000), "25:00");
        assert_eq!(format_clock(25 * 60_000 - 1), "25:00");
        assert_eq!(format_clock(25 * 60_000 - 1000), "24:59");
        assert_eq!(format_clock(0), "00:00");
    }
}
