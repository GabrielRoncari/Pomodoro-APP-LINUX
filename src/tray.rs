//! Ícone na bandeja (StatusNotifierItem): anel de progresso com os minutos e um menu.

use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{Category, Icon, MenuItem, ToolTip, Tray};
use smithay_client_toolkit::reexports::calloop::channel::Sender;

use crate::ipc::{Cmd, Msg};
use crate::render;
use crate::timer::{Phase, RunKind};

/// Passos do anel no ícone: menos passos, menos atualizações pela D-Bus.
pub const FRAC_STEPS: f32 = 96.0;

/// O que a bandeja mostra. Só atualizamos o ksni quando isto muda.
#[derive(Clone, PartialEq, Default, Debug)]
pub struct Snap {
    pub phase: Phase,
    pub run: RunKind,
    /// "faltam 24 min" (granularidade de minuto para o menu não piscar)
    pub left: String,
    pub icon_label: String,
    pub frac_q: u16,
    pub color: [u8; 3],
    pub today_count: u32,
    pub today_min: u32,
    pub overlay_visible: bool,
    pub locked: bool,
    pub strict: bool,
    pub outputs: Vec<String>,
    pub output: String,
    pub blocking: bool,
}

pub struct FocoTray {
    pub snap: Snap,
    pub tx: Sender<Msg>,
}

impl FocoTray {
    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(Msg { cmd, reply: None });
    }

    fn status_line(&self) -> String {
        let s = &self.snap;
        match s.run {
            RunKind::Idle => format!("{} · pronto para começar", s.phase.name()),
            RunKind::Paused => format!("{} · pausado, {}", s.phase.name(), s.left),
            RunKind::Running => format!("{} · {}", s.phase.name(), s.left),
        }
    }
}

fn item(label: impl Into<String>, enabled: bool, cmd: Cmd) -> MenuItem<FocoTray> {
    StandardItem {
        label: label.into(),
        enabled,
        activate: Box::new(move |t: &mut FocoTray| t.send(cmd.clone())),
        ..Default::default()
    }
    .into()
}

fn info(label: impl Into<String>) -> MenuItem<FocoTray> {
    StandardItem { label: label.into(), enabled: false, ..Default::default() }.into()
}

pub fn fmt_minutes(min: u32) -> String {
    if min >= 60 { format!("{}h{:02}", min / 60, min % 60) } else { format!("{min} min") }
}

impl Tray for FocoTray {
    fn id(&self) -> String {
        "foco".into()
    }

    fn title(&self) -> String {
        "Foco".into()
    }

    fn category(&self) -> Category {
        Category::ApplicationStatus
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        let s = &self.snap;
        [22u32, 32, 48, 64]
            .iter()
            .map(|&px| Icon {
                width: px as i32,
                height: px as i32,
                data: render::tray_icon(px, s.color, s.frac_q as f32 / FRAC_STEPS, s.run, &s.icon_label),
            })
            .collect()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip { title: "Foco".into(), description: self.status_line(), ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(Cmd::Toggle);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let s = &self.snap;
        let free = !s.strict;
        let toggle_label = match s.run {
            RunKind::Running => "Pausar",
            RunKind::Paused => "Continuar",
            RunKind::Idle => "Iniciar",
        };
        let mut status = self.status_line();
        if s.strict {
            status.push_str(" · modo rígido");
        }
        let today = if s.today_count == 0 {
            "Hoje: nenhum foco ainda".to_string()
        } else {
            format!(
                "Hoje: {} {} · {}",
                s.today_count,
                if s.today_count == 1 { "foco" } else { "focos" },
                fmt_minutes(s.today_min)
            )
        };

        let mut menu = vec![
            info(status),
            info(today),
            MenuItem::Separator,
            item(toggle_label, free, Cmd::Toggle),
            item("Pular etapa", free, Cmd::Skip),
            item("Reiniciar etapa", free && s.run != RunKind::Idle, Cmd::Reset),
            SubMenu {
                label: "Ir para".into(),
                enabled: free,
                submenu: vec![
                    item("Foco", true, Cmd::Phase(Phase::Focus)),
                    item("Pausa curta", true, Cmd::Phase(Phase::ShortBreak)),
                    item("Pausa longa", true, Cmd::Phase(Phase::LongBreak)),
                    MenuItem::Separator,
                    item("Zerar ciclo", true, Cmd::ResetCycle),
                ],
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            CheckmarkItem {
                label: "Mostrar overlay".into(),
                checked: s.overlay_visible,
                activate: Box::new(|t: &mut FocoTray| t.send(Cmd::Overlay(None))),
                ..Default::default()
            }
            .into(),
            CheckmarkItem {
                label: "Travar overlay (cliques atravessam)".into(),
                checked: s.locked,
                enabled: s.overlay_visible,
                activate: Box::new(|t: &mut FocoTray| t.send(Cmd::Lock(None))),
                ..Default::default()
            }
            .into(),
        ];
        if s.outputs.len() > 1 {
            let outputs = s.outputs.clone();
            menu.push(
                SubMenu {
                    label: "Monitor do overlay".into(),
                    enabled: s.overlay_visible,
                    submenu: vec![
                        RadioGroup {
                            selected: s.outputs.iter().position(|o| *o == s.output).unwrap_or(usize::MAX),
                            select: Box::new(move |t: &mut FocoTray, i| {
                                if let Some(name) = outputs.get(i) {
                                    t.send(Cmd::Output(name.clone()));
                                }
                            }),
                            options: s
                                .outputs
                                .iter()
                                .map(|o| RadioItem { label: o.clone(), ..Default::default() })
                                .collect(),
                        }
                        .into(),
                    ],
                    ..Default::default()
                }
                .into(),
            );
        }
        if s.blocking {
            menu.push(MenuItem::Separator);
            menu.push(info("Sites bloqueados durante este foco"));
        }
        menu.extend([
            MenuItem::Separator,
            item("Configurações…", true, Cmd::Settings),
            item("Sair", true, Cmd::Quit),
        ]);
        menu
    }
}
