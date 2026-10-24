//! O miniaplicativo: anel e relógio no painel, controles num popup no estilo dos applets do COSMIC.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use cosmic::applet::token::subscription::{TokenRequest, TokenUpdate, activation_token_subscription};
use cosmic::applet::{menu_button, padded_control};
use cosmic::cctk::sctk::reexports::calloop;
use cosmic::cosmic_theme::Spacing;
use cosmic::iced::platform_specific::shell::wayland::commands::popup::{destroy_popup, get_popup};
use cosmic::iced::widget::{column, row, stack};
use cosmic::iced::{Alignment, Color, Length, Limits, Subscription, window};
use cosmic::widget::{
    self, autosize, button, container, divider, icon, segmented_button, segmented_control, space, text, toggler,
};
use cosmic::{Action, Element, Task, theme};

use crate::daemon::{self, Reply, Status};
use crate::ring::{self, Ring};

static AUTOSIZE_MAIN_ID: LazyLock<widget::Id> = LazyLock::new(|| widget::Id::new("foco-autosize-main"));

const DIAL: f32 = 184.0;

pub struct Foco {
    core: cosmic::Core,
    popup: Option<window::Id>,
    /// `None`: o daemon não está rodando.
    status: Option<Status>,
    /// Quando o estado chegou, para o relógio andar entre uma consulta e outra.
    at: Instant,
    /// Só a cadeia de consultas mais recente continua; respostas antigas são descartadas.
    serial: u64,
    error: Option<String>,
    phases: segmented_button::SingleSelectModel,
    token_tx: Option<calloop::channel::Sender<TokenRequest>>,
}

#[derive(Debug, Clone)]
pub enum Message {
    TogglePopup,
    PopupClosed(window::Id),
    Poll(u64),
    Reply(u64, Reply),
    Send(String),
    Phase(segmented_button::Entity),
    StartDaemon,
    OpenSettings,
    Token(TokenUpdate),
}

fn clock(ms: u64) -> String {
    let secs = ms.div_ceil(1000);
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

fn hex(c: impl Into<Color>) -> String {
    let c: Color = c.into();
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c.r), b(c.g), b(c.b))
}

fn on_off(v: bool) -> &'static str {
    if v { "on" } else { "off" }
}

impl Foco {
    fn remaining_ms(&self, s: &Status) -> u64 {
        if s.running() {
            s.remaining_ms.saturating_sub(self.at.elapsed().as_millis() as u64)
        } else {
            s.remaining_ms
        }
    }

    fn left_fraction(&self, s: &Status) -> f32 {
        if s.total_ms == 0 { 1.0 } else { self.remaining_ms(s) as f32 / s.total_ms as f32 }
    }

    /// Pergunta o estado ao daemon (mandando antes o comando, se houver).
    fn fetch(&mut self, cmd: Option<String>) -> Task<Action<Message>> {
        self.serial += 1;
        let serial = self.serial;
        cosmic::task::future(async move { Action::App(Message::Reply(serial, daemon::send(cmd).await)) })
    }

    /// Próxima consulta: logo depois de o relógio virar o segundo, ou a cada segundo se parado.
    fn schedule(&self, delay: Option<Duration>) -> Task<Action<Message>> {
        let delay = delay.unwrap_or_else(|| match &self.status {
            Some(s) if s.running() => {
                let rem = s.remaining_ms;
                let to_next = if rem % 1000 == 0 { 1000 } else { rem % 1000 };
                Duration::from_millis(to_next.min(rem.max(1)) + 20)
            }
            Some(_) => Duration::from_secs(1),
            None => Duration::from_secs(2),
        });
        let serial = self.serial;
        cosmic::task::future(async move {
            tokio::time::sleep(delay).await;
            Action::App(Message::Poll(serial))
        })
    }

    fn sync_phases(&mut self) {
        let Some(s) = &self.status else { return };
        let target = self
            .phases
            .iter()
            .find(|&e| self.phases.data::<&'static str>(e).is_some_and(|k| *k == s.phase));
        if let Some(e) = target
            && self.phases.active() != e
        {
            self.phases.activate(e);
        }
    }

    fn colors(&self) -> (String, String) {
        let cosmic = theme::active();
        let track = hex(cosmic.cosmic().on_bg_color());
        let color = match &self.status {
            Some(s) if !s.color.is_empty() => s.color.clone(),
            _ => track.clone(),
        };
        (color, track)
    }

    fn panel_ring(&self) -> icon::Handle {
        let (color, track) = self.colors();
        let ring = match &self.status {
            Some(s) => Ring {
                // degraus de 1/120: o anel do painel é pequeno e o SVG fica em cache
                left: (self.left_fraction(s) * 120.0).ceil() / 120.0,
                color: &color,
                track: &track,
                width: 15.0,
                pause_bars: s.paused(),
                dim: s.idle(),
            },
            None => Ring { left: 0.0, color: &color, track: &track, width: 15.0, pause_bars: false, dim: true },
        };
        icon::from_svg_bytes(ring.svg())
    }

    fn dial(&self, s: Option<&Status>) -> Element<'_, Message> {
        let (color, track) = self.colors();
        let ring = Ring {
            left: s.map_or(0.0, |s| (self.left_fraction(s) * 720.0).ceil() / 720.0),
            color: &color,
            track: &track,
            width: 6.5,
            pause_bars: false,
            dim: s.is_none_or(Status::idle),
        };
        let center: Element<'_, Message> = match s {
            Some(s) => {
                let state = if s.running() {
                    "em andamento"
                } else if s.paused() {
                    "pausado"
                } else {
                    "pronto"
                };
                column![
                    text::caption_heading(s.phase_name.to_uppercase()),
                    text(clock(self.remaining_ms(s))).size(42).font(cosmic::font::semibold()),
                    text::caption(state),
                ]
                .align_x(Alignment::Center)
                .into()
            }
            None => text::title3("Foco").into(),
        };
        container(
            stack![
                widget::icon(icon::from_svg_bytes(ring.svg()))
                    .width(Length::Fixed(DIAL))
                    .height(Length::Fixed(DIAL)),
                container(center).center(Length::Fixed(DIAL)),
            ]
            .width(Length::Fixed(DIAL))
            .height(Length::Fixed(DIAL)),
        )
        .center_x(Length::Fill)
        .into()
    }

    fn offline_view(&self) -> Element<'_, Message> {
        let Spacing { space_xxs, space_s, .. } = theme::active().cosmic().spacing;
        column![
            padded_control(self.dial(None)).padding([space_s, 0]),
            padded_control(
                column![
                    text::heading("O Foco não está rodando"),
                    text::caption("O timer, o overlay e o bloqueio de sites ficam no programa do Foco.")
                        .align_x(Alignment::Center),
                ]
                .spacing(space_xxs)
                .align_x(Alignment::Center)
                .width(Length::Fill),
            ),
            padded_control(
                container(button::suggested("Iniciar o Foco").on_press(Message::StartDaemon)).center_x(Length::Fill)
            ),
            padded_control(divider::horizontal::default()).padding([space_xxs, space_s]),
            menu_button(text::body("Configurações…")).on_press(Message::OpenSettings),
        ]
        .padding([8, 0])
        .into()
    }

    fn status_view<'a>(&'a self, s: &'a Status) -> Element<'a, Message> {
        let Spacing { space_xxs, space_xs, space_s, .. } = theme::active().cosmic().spacing;
        let locked = s.strict_locked;
        let (color, track) = self.colors();

        let (toggle_icon, toggle_label) = if s.running() {
            ("media-playback-pause-symbolic", "Pausar")
        } else if s.paused() {
            ("media-playback-start-symbolic", "Continuar")
        } else {
            ("media-playback-start-symbolic", "Iniciar")
        };
        let cmd = |c: &str| (!locked).then(|| Message::Send(c.to_string()));
        let controls = row![
            button::icon(icon::from_name("view-refresh-symbolic"))
                .tooltip("Reiniciar etapa")
                .on_press_maybe(cmd("reset")),
            space::horizontal(),
            button::suggested(toggle_label)
                .leading_icon(icon::from_name(toggle_icon))
                .width(Length::Fixed(168.0))
                .on_press_maybe(cmd("toggle")),
            space::horizontal(),
            button::icon(icon::from_name("media-skip-forward-symbolic"))
                .tooltip("Pular etapa")
                .on_press_maybe(cmd("skip")),
        ]
        .align_y(Alignment::Center);

        let (dots_svg, dots_w) = ring::dots(s.done_in_cycle, s.every, &color, &track);
        let cycle = row![
            widget::icon(icon::from_svg_bytes(dots_svg))
                .width(Length::Fixed(dots_w))
                .height(Length::Fixed(14.0)),
            text::caption(format!("ciclo {}/{} até a pausa longa", s.done_in_cycle, s.every)),
        ]
        .spacing(space_xs)
        .align_y(Alignment::Center);

        let mut phases = segmented_control::horizontal(&self.phases);
        if !locked {
            phases = phases.on_activate(Message::Phase);
        }
        let mut top = column![self.dial(Some(s)), container(cycle).center_x(Length::Fill), controls, phases]
            .spacing(space_s);
        if locked {
            top = top.push(text::caption("Modo rígido: termine o foco para pausar, pular ou trocar de etapa."));
        } else if let Some(e) = &self.error {
            top = top.push(text::caption(e.clone()));
        }

        let today = format!(
            "{} {} · {} min",
            s.today.count,
            if s.today.count == 1 { "foco" } else { "focos" },
            s.today.minutes
        );
        let mut info = column![row![text::body("Hoje").width(Length::Fill), text::body(today)]].spacing(space_xxs);
        if s.blocking {
            info = info.push(
                row![
                    icon::from_name("changes-prevent-symbolic").size(16).icon(),
                    text::body("Sites bloqueados durante este foco"),
                ]
                .spacing(space_xs)
                .align_y(Alignment::Center),
            );
        }

        column![
            padded_control(top).padding([space_xs, space_s]),
            padded_control(divider::horizontal::default()).padding([space_xxs, space_s]),
            padded_control(info),
            padded_control(divider::horizontal::default()).padding([space_xxs, space_s]),
            padded_control(
                toggler(s.overlay.visible)
                    .label("Mostrar overlay".to_string())
                    .text_size(14)
                    .width(Length::Fill)
                    .on_toggle(|v| Message::Send(format!("overlay {}", on_off(v)))),
            ),
            padded_control(
                toggler(s.overlay.locked)
                    .label("Travar overlay (cliques atravessam)".to_string())
                    .text_size(14)
                    .width(Length::Fill)
                    .on_toggle(|v| Message::Send(format!("lock {}", on_off(v)))),
            ),
            padded_control(divider::horizontal::default()).padding([space_xxs, space_s]),
            menu_button(text::body("Configurações…")).on_press(Message::OpenSettings),
        ]
        .padding([8, 0])
        .into()
    }
}

impl cosmic::Application for Foco {
    type Executor = cosmic::SingleThreadExecutor;
    type Flags = ();
    type Message = Message;

    const APP_ID: &'static str = "dev.foco.Applet";

    fn core(&self) -> &cosmic::Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut cosmic::Core {
        &mut self.core
    }

    fn init(core: cosmic::Core, _flags: ()) -> (Self, Task<Action<Message>>) {
        let phases = segmented_button::ModelBuilder::default()
            .insert(|b| b.text("Foco").data("focus").activate())
            .insert(|b| b.text("Pausa curta").data("short_break"))
            .insert(|b| b.text("Pausa longa").data("long_break"))
            .build();
        let mut app = Foco {
            core,
            popup: None,
            status: None,
            at: Instant::now(),
            serial: 0,
            error: None,
            phases,
            token_tx: None,
        };
        let first = app.fetch(None);
        (app, first)
    }

    fn on_close_requested(&self, id: window::Id) -> Option<Message> {
        Some(Message::PopupClosed(id))
    }

    fn subscription(&self) -> Subscription<Message> {
        activation_token_subscription(0).map(Message::Token)
    }

    fn update(&mut self, message: Message) -> Task<Action<Message>> {
        match message {
            Message::Poll(serial) => {
                if serial == self.serial {
                    return self.fetch(None);
                }
            }
            Message::Reply(serial, reply) => {
                if serial != self.serial {
                    return Task::none();
                }
                self.status = reply.status;
                self.at = Instant::now();
                self.error = reply.error;
                self.sync_phases();
                return self.schedule(None);
            }
            Message::Send(cmd) => {
                return self.fetch(Some(cmd));
            }
            Message::Phase(entity) => {
                if let Some(key) = self.phases.data::<&'static str>(entity).copied() {
                    self.phases.activate(entity);
                    return self.fetch(Some(format!("phase {key}")));
                }
            }
            Message::StartDaemon => {
                if daemon::start_daemon() {
                    self.serial += 1;
                    return self.schedule(Some(Duration::from_millis(700)));
                }
                self.error = Some("Não achei o programa foco. Rode o install.sh de novo.".into());
            }
            Message::OpenSettings => {
                if let Some(tx) = &self.token_tx {
                    let _ = tx.send(TokenRequest { app_id: Self::APP_ID.to_string(), exec: "foco-settings".into() });
                } else {
                    daemon::open_settings(None);
                }
            }
            Message::Token(update) => match update {
                TokenUpdate::Init(tx) => self.token_tx = Some(tx),
                TokenUpdate::Finished => self.token_tx = None,
                TokenUpdate::ActivationToken { token, .. } => {
                    daemon::open_settings(token);
                }
            },
            Message::TogglePopup => {
                if let Some(p) = self.popup.take() {
                    return destroy_popup(p);
                }
                let id = window::Id::unique();
                self.popup = Some(id);
                let mut settings =
                    self.core.applet.get_popup_settings(self.core.main_window_id().unwrap(), id, None, None, None);
                settings.positioner.size_limits =
                    Limits::NONE.min_width(360.0).max_width(360.0).min_height(200.0).max_height(1080.0);
                // o estado pode ter mudado por fora (overlay, bandeja, atalho): atualiza ao abrir
                return Task::batch([get_popup(settings), self.fetch(None)]);
            }
            Message::PopupClosed(id) => {
                if self.popup == Some(id) {
                    self.popup = None;
                }
            }
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let applet = &self.core.applet;
        let horizontal = applet.is_horizontal();
        let (size, _) = applet.suggested_size(true);
        let (pad_major, pad_minor) = applet.suggested_padding(true);
        let ring = widget::icon(self.panel_ring())
            .width(Length::Fixed(f32::from(size)))
            .height(Length::Fixed(f32::from(size)));

        let content: Element<'_, Message> = match &self.status {
            Some(s) if horizontal && !s.idle() => row![ring, applet.text(clock(self.remaining_ms(s)))]
                .spacing(6)
                .align_y(Alignment::Center)
                .into(),
            _ => ring.into(),
        };
        let thickness = Length::Fixed(f32::from(size + 2 * pad_minor));
        let content = if horizontal {
            container(content).center_y(thickness)
        } else {
            container(content).center_x(thickness)
        };
        let button = button::custom(content)
            .padding(if horizontal { [0, pad_major] } else { [pad_major, 0] })
            .on_press_down(Message::TogglePopup)
            .class(theme::Button::AppletIcon);

        autosize::autosize(button, AUTOSIZE_MAIN_ID.clone()).into()
    }

    fn view_window(&self, _id: window::Id) -> Element<'_, Message> {
        let content = match &self.status {
            Some(s) => self.status_view(s),
            None => self.offline_view(),
        };
        self.core.applet.popup_container(content).into()
    }

    fn style(&self) -> Option<cosmic::iced::theme::Style> {
        Some(cosmic::applet::style())
    }
}
