//! Laço principal: timer, overlay, bandeja, socket e bloqueio.
//!
//! O overlay sai por um de dois caminhos: layer-shell no Wayland (`wl.rs`: COSMIC, KDE, Hyprland,
//! Sway…) ou uma janela X11 sempre por cima (`x11.rs`: GNOME pelo Xwayland e sessões X11). O resto
//! (timer, mouse, desenho) é o mesmo para os dois.

use std::os::unix::net::UnixListener;
use std::panic::AssertUnwindSafe;
use std::time::{Duration, Instant, SystemTime};

use smithay_client_toolkit::{
    reexports::calloop::{
        EventLoop, LoopHandle, RegistrationToken,
        channel,
        signals::{Signal, Signals},
        timer::{TimeoutAction, Timer},
    },
    seat::pointer::CursorIcon,
    shell::{
        WaylandSurface,
        wlr_layer::{Layer, LayerSurface},
    },
};
use tiny_skia::Pixmap;

use crate::blocker::{self, Blocker};
use crate::config::{self, Config, MAX_SIZE, MIN_SIZE, Persist};
use crate::ipc::{self, Cmd, Msg};
use crate::notify;
use crate::render::{self, Geo, Hit, View};
use crate::stats::Stats;
use crate::timer::{Phase, Run, RunKind, format_clock, now_ms};
use crate::tray::{FRAC_STEPS, FocoTray, Snap};
use crate::{wl, x11};

const HOVER_SECS: f32 = 0.16;
const FLASH_SECS: f32 = 1.8;
/// Botões com os códigos do evdev (os do Wayland); os do X11 são traduzidos para eles.
pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;

pub struct Press {
    button: u32,
    pos: (f64, f64),
    target: Hit,
    dragging: bool,
    size0: u32,
}

pub enum Surf {
    Wl(LayerSurface),
    X(x11::Win),
}

pub struct Overlay {
    pub surf: Surf,
    pub serial: u64,
    pub configured: bool,
    /// Tamanho (lógico) do buffer atual.
    pub size: u32,
    /// Tamanho pedido mais recente; vira `size` no próximo configure.
    pub want_size: u32,
    pub size_pending: bool,
    pub scale: i32,
    pub output_name: String,
    /// Margens (esquerda, topo) já enviadas.
    pub margin: (i32, i32),
    pub want_margin: (i32, i32),
    /// Margens que a tela já aplicou: as coordenadas do mouse são relativas a elas.
    pub effective: (i32, i32),
    pub margin_inflight: bool,
    pub frame_pending: bool,
    pub dirty: bool,
    pub hover: bool,
    pub hot: Hit,
    pub press: Option<Press>,
    pub region_for: Option<(u32, bool)>,
    pub pix: Option<Pixmap>,
    pub cursor: Option<CursorIcon>,
}

/// Um monitor, em pixels lógicos.
#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub name: String,
    pub pos: (i32, i32),
    pub size: (i32, i32),
    pub primary: bool,
}

/// Evento do mouse sobre o overlay, já sem o sotaque de cada protocolo.
pub enum Ptr {
    Enter,
    Leave,
    Motion,
    Press(u32),
    /// Botão e serial (Wayland) ou horário (X11) do clique, para a ativação da central.
    Release(u32, u32),
    /// Passos da roda; positivo = para baixo.
    Scroll(f64),
}

pub enum Disp {
    Wl(Box<wl::Wl>),
    X(Box<x11::X11>),
    /// Sem onde desenhar (nem layer-shell nem X11): timer, bandeja, central e bloqueio seguem.
    Nenhum,
}

pub struct App {
    pub lh: LoopHandle<'static, App>,
    pub disp: Disp,
    pub ov: Option<Overlay>,
    ov_serial: u64,

    cfg: Config,
    cfg_mtime: Option<SystemTime>,
    pub p: Persist,
    stats: Stats,
    blocker: Blocker,
    warned_block_missing: bool,
    /// O modo rígido foi desligado durante um foco travado; a config é relida quando ele acabar.
    strict_hold: bool,
    tx: channel::Sender<Msg>,
    tray: Option<ksni::blocking::Handle<FocoTray>>,
    tray_snap: Snap,
    tick: Option<RegistrationToken>,
    /// X11 não tem frame callback: as animações andam num timer.
    anim_tick: Option<RegistrationToken>,

    hover_t: f32,
    flash_t: f32,
    last_anim: Option<Instant>,
    pub exit: bool,
}

fn mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

pub fn run(listener: UnixListener) -> Result<(), Box<dyn std::error::Error>> {
    let mut event_loop: EventLoop<'static, App> = EventLoop::try_new()?;
    let lh = event_loop.handle();

    // layer-shell quando o compositor tem; senão uma janela X11, que no GNOME vai pelo Xwayland
    let (disp, wl_queue) = match wl::Wl::connect() {
        Ok((w, queue)) => (Disp::Wl(Box::new(w)), Some(queue)),
        Err(why_wl) => match x11::X11::connect() {
            Ok(x) => {
                config::log(&format!("overlay pelo X11 ({why_wl})"));
                (Disp::X(Box::new(x)), None)
            }
            Err(why_x) => {
                config::log(&format!("sem overlay: {why_wl}; X11: {why_x}"));
                notify::notify(
                    "Foco sem o círculo na tela",
                    "Esta sessão não deixa desenhar o overlay. O timer, a bandeja e a central funcionam.",
                );
                (Disp::Nenhum, None)
            }
        },
    };

    let (tx, rx) = channel::channel::<Msg>();
    lh.insert_source(rx, |ev, _, app| {
        if let channel::Event::Msg(msg) = ev {
            app.on_msg(msg);
        }
    })?;
    lh.insert_source(Signals::new(&[Signal::SIGTERM, Signal::SIGINT, Signal::SIGHUP])?, |ev, _, app| {
        config::log(&format!("encerrando: sinal {:?}", ev.signal()));
        app.exit = true;
    })?;

    ipc::serve(listener, tx.clone());

    let btx = tx.clone();
    let blocker = Blocker::new(move |e| {
        let _ = btx.send(Msg { cmd: Cmd::BlockError(e), reply: None });
    });

    let cfg = Config::load();
    let mut app = App {
        lh: lh.clone(),
        disp,
        ov: None,
        ov_serial: 0,
        cfg_mtime: mtime(&config::config_path()),
        cfg,
        p: Persist::load(),
        stats: Stats::load(),
        blocker,
        warned_block_missing: false,
        strict_hold: false,
        tx,
        tray: None,
        tray_snap: Snap::default(),
        tick: None,
        anim_tick: None,
        hover_t: 0.0,
        flash_t: 0.0,
        last_anim: None,
        exit: false,
    };

    match wl_queue {
        Some(mut queue) => {
            // nomes e tamanhos dos monitores chegam em eventos: espera por eles antes do overlay
            queue.roundtrip(&mut app)?;
            queue.roundtrip(&mut app)?;
            let conn = app.wl().conn.clone();
            wl::insert_source(conn, queue, &lh)?;
        }
        None => x11::insert_source(&app, &lh)?,
    }

    app.sync_tray();
    app.startup();
    // um erro ou pânico no laço não pode deixar sites bloqueados nem o socket para trás
    let result: Result<(), Box<dyn std::error::Error>> = loop {
        if app.exit {
            break Ok(());
        }
        let step = std::panic::catch_unwind(AssertUnwindSafe(|| {
            event_loop.dispatch(None, &mut app)?;
            x11::pump(&mut app);
            Ok::<(), Box<dyn std::error::Error>>(())
        }));
        match step {
            Ok(Ok(())) => {}
            Ok(Err(e)) => break Err(e),
            Err(_) => break Err("pânico no laço principal".into()),
        }
    };
    app.shutdown();
    result
}

impl App {
    pub fn wl(&mut self) -> &mut wl::Wl {
        match &mut self.disp {
            Disp::Wl(w) => w,
            _ => unreachable!("evento Wayland sem conexão Wayland"),
        }
    }

    fn startup(&mut self) {
        let now = now_ms();
        // o timer acabou enquanto o Foco estava fechado: volta ao início da etapa
        if let Run::Running { ends_at_ms } = self.p.timer.run
            && ends_at_ms <= now
        {
            self.p.timer.run = Run::Idle;
        }
        self.sync_block();
        if self.p.ui.visible {
            self.show_overlay();
        }
        self.schedule_tick();
        self.refresh_tray();
    }

    fn shutdown(&mut self) {
        self.p.save();
        if !self.p.timer.strict_locked(&self.cfg) {
            self.blocker.set(None, false);
        }
        self.blocker.flush(Duration::from_secs(6));
        let _ = std::fs::remove_file(config::socket_path());
        if let Some(t) = self.tray.take() {
            t.shutdown().wait();
        }
    }

    // ---------- comandos ----------

    fn on_msg(&mut self, msg: Msg) {
        let resp = match self.handle(msg.cmd) {
            Ok(v) => v,
            Err(e) => serde_json::json!({ "ok": false, "error": e }),
        };
        if let Some(reply) = msg.reply {
            let _ = reply.send(resp.to_string());
        }
    }

    fn handle(&mut self, cmd: Cmd) -> Result<serde_json::Value, String> {
        let now = now_ms();
        let strict = self.p.timer.strict_locked(&self.cfg);
        let ok = serde_json::json!({ "ok": true });
        let timer_cmd = matches!(
            cmd,
            Cmd::Toggle | Cmd::Pause | Cmd::Skip | Cmd::Reset | Cmd::ResetCycle | Cmd::Phase(_)
        );
        if strict && timer_cmd {
            return Err("modo rígido: termine o foco primeiro".into());
        }
        match cmd {
            Cmd::Toggle => self.p.timer.toggle(now, &self.cfg),
            Cmd::Start => self.p.timer.start(now, &self.cfg),
            Cmd::Pause => self.p.timer.pause(now),
            Cmd::Skip => {
                let was_active = self.p.timer.run != Run::Idle;
                self.p.timer.advance(&self.cfg, false);
                if was_active {
                    self.p.timer.start(now, &self.cfg);
                }
            }
            Cmd::Reset => self.p.timer.reset(),
            Cmd::ResetCycle => {
                self.p.timer.done_in_cycle = 0;
                self.p.timer.go_to(Phase::Focus);
            }
            Cmd::Phase(ph) => self.p.timer.go_to(ph),
            Cmd::Overlay(v) => {
                self.p.ui.visible = v.unwrap_or(!self.p.ui.visible);
                if self.p.ui.visible { self.show_overlay() } else { self.hide_overlay() }
            }
            Cmd::Lock(v) => {
                self.p.ui.locked = v.unwrap_or(!self.p.ui.locked);
                if let Some(ov) = self.ov.as_mut() {
                    ov.hover = false;
                    ov.hot = Hit::None;
                }
                self.request_redraw();
            }
            Cmd::Size(n) => self.resize(n, true),
            Cmd::Output(name) => {
                if !name.is_empty() && !self.output_names().contains(&name) {
                    return Err(format!("monitor desconhecido: {name}"));
                }
                self.p.ui.output = name;
                self.p.ui.positioned = false;
                if self.p.ui.visible {
                    self.hide_overlay();
                    self.show_overlay();
                }
            }
            Cmd::Reload => self.reload_config(),
            Cmd::Settings => {
                self.open_settings();
                return Ok(ok);
            }
            Cmd::Status => return Ok(self.status_json()),
            Cmd::Quit => {
                config::log("encerrando: comando quit");
                self.exit = true;
                return Ok(ok);
            }
            Cmd::BlockError(e) => {
                self.blocker.resync();
                notify::notify("Foco: bloqueio de sites falhou", &e);
                return Ok(ok);
            }
        }
        self.after_change();
        Ok(ok)
    }

    /// Depois de qualquer mudança de estado: salva, bloqueia/libera, redesenha.
    fn after_change(&mut self) {
        if self.strict_hold && !self.p.timer.strict_locked(&self.cfg) {
            self.reload_config();
        }
        self.p.save();
        self.sync_block();
        self.schedule_tick();
        self.request_redraw();
        self.refresh_tray();
    }

    fn status_json(&self) -> serde_json::Value {
        let now = now_ms();
        let t = &self.p.timer;
        let rem = t.remaining_ms(now, &self.cfg);
        let (count, minutes) = self.stats.today();
        let [r, g, b] = self.phase_color(t.phase);
        serde_json::json!({
            "ok": true,
            "phase": t.phase.key(),
            "phase_name": t.phase.name(),
            "state": match t.kind() { RunKind::Idle => "idle", RunKind::Running => "running", RunKind::Paused => "paused" },
            "remaining_ms": rem,
            "total_ms": t.duration_ms(&self.cfg),
            "clock": format_clock(rem),
            "done_in_cycle": t.done_in_cycle,
            "every": self.cfg.long_break_every,
            "strict_locked": t.strict_locked(&self.cfg),
            "color": format!("#{r:02x}{g:02x}{b:02x}"),
            "blocking": self.blocker.active(),
            "blocked_sites": self.blocker.applied(),
            "block_installed": blocker::installed(),
            "overlay": {
                "visible": self.p.ui.visible,
                "locked": self.p.ui.locked,
                "size": self.p.ui.size,
                "output": self.p.ui.output,
            },
            "outputs": self.output_names(),
            "backend": match self.disp { Disp::Wl(_) => "wayland", Disp::X(_) => "x11", Disp::Nenhum => "nenhum" },
            "today": { "count": count, "minutes": minutes },
        })
    }

    fn reload_config(&mut self) {
        self.cfg_mtime = mtime(&config::config_path());
        let strict = self.p.timer.strict_locked(&self.cfg);
        let old = std::mem::replace(&mut self.cfg, Config::load());
        // desligar o modo rígido no meio do foco só vale quando ele acabar
        self.strict_hold = strict && !self.cfg.strict_mode;
        if self.strict_hold {
            self.cfg.strict_mode = true;
        }
        if old.appearance.above_fullscreen != self.cfg.appearance.above_fullscreen
            && let Some(Overlay { surf: Surf::Wl(layer), .. }) = &self.ov
        {
            layer.set_layer(self.layer_kind());
            layer.commit();
        }
        self.sync_tray();
    }

    fn open_settings(&self) {
        notify::open_settings(None, &[]);
    }

    /// Clique no overlay: abre a central (ou fecha, se ela já está na frente).
    fn open_central(&self, serial: u32) {
        match (&self.disp, &self.ov) {
            (Disp::Wl(w), Some(Overlay { surf: Surf::Wl(layer), .. })) => w.open_central(layer, serial),
            (Disp::X(_), _) => x11::open_central(serial),
            _ => {
                notify::open_settings(None, &["toggle"]);
            }
        }
    }

    // ---------- timer ----------

    fn schedule_tick(&mut self) {
        if let Some(tok) = self.tick.take() {
            self.lh.remove(tok);
        }
        let delay = match self.p.timer.run {
            Run::Running { ends_at_ms } => {
                let rem = ends_at_ms.saturating_sub(now_ms());
                // acorda logo depois de o relógio mudar de segundo
                let to_next = if rem % 1000 == 0 { 1000 } else { rem % 1000 };
                Duration::from_millis(to_next.min(rem).max(1) + 3)
            }
            _ => Duration::from_secs(1),
        };
        self.tick = self
            .lh
            .insert_source(Timer::from_duration(delay), |_, _, app| {
                app.tick = None;
                app.on_tick();
                TimeoutAction::Drop
            })
            .ok();
    }

    fn on_tick(&mut self) {
        let now = now_ms();
        if let Run::Running { ends_at_ms } = self.p.timer.run
            && now >= ends_at_ms
        {
            self.complete_phase(now);
            self.after_change();
            return;
        }
        if mtime(&config::config_path()) != self.cfg_mtime {
            self.reload_config();
            self.sync_block();
        }
        self.keep_on_top();
        self.request_redraw();
        self.refresh_tray();
        self.schedule_tick();
    }

    fn complete_phase(&mut self, now: u64) {
        let finished = self.p.timer.phase;
        if finished == Phase::Focus {
            self.stats.record_focus((self.p.timer.total_ms / 60_000) as u32);
        }
        let next = self.p.timer.advance(&self.cfg, true);
        let auto = match next {
            Phase::Focus => self.cfg.auto_start_focus,
            _ => self.cfg.auto_start_breaks,
        };
        if auto {
            self.p.timer.start(now, &self.cfg);
        }
        let mins = next.minutes(&self.cfg);
        let (title, body) = match (finished, next) {
            (Phase::Focus, Phase::LongBreak) => (
                "Ciclo completo!".to_string(),
                format!(
                    "{} focos feitos. {} de {mins} min.",
                    self.cfg.long_break_every,
                    if auto { "Pausa longa começou" } else { "Hora da pausa longa" }
                ),
            ),
            (Phase::Focus, _) => (
                "Foco concluído".to_string(),
                if auto { format!("Pausa de {mins} min começou.") } else { format!("Hora de uma pausa de {mins} min.") },
            ),
            _ => (
                "Fim da pausa".to_string(),
                if auto { format!("Foco de {mins} min começou.") } else { format!("Bora voltar? Foco de {mins} min.") },
            ),
        };
        if self.cfg.alerts.notifications {
            notify::notify(&title, &body);
        }
        if self.cfg.alerts.sound {
            let name = if finished == Phase::Focus { "complete" } else { "bell" };
            notify::sound(name, self.cfg.alerts.volume);
        }
        self.flash_t = 1.0;
        self.last_anim = None;
    }

    fn sync_block(&mut self) {
        let t = &self.p.timer;
        let b = &self.cfg.block;
        let mut want = (b.enabled && t.phase == Phase::Focus && t.run != Run::Idle && !b.sites.is_empty())
            .then(|| b.sites.clone());
        // modo rígido: durante o foco a lista só cresce
        if t.strict_locked(&self.cfg)
            && let Some(cur) = self.blocker.applied()
        {
            let mut merged = want.unwrap_or_default();
            for s in cur {
                if !merged.contains(s) {
                    merged.push(s.clone());
                }
            }
            want = Some(merged);
        }
        if want.is_some() && !blocker::installed() {
            if !self.warned_block_missing {
                self.warned_block_missing = true;
                notify::notify(
                    "Bloqueio de sites não instalado",
                    "Abra a central do Foco → Sites bloqueados e clique em Instalar (pede a senha uma vez).",
                );
            }
            return;
        }
        self.blocker.set(want, b.kill_connections);
    }

    // ---------- bandeja ----------

    fn snapshot(&self) -> Snap {
        let now = now_ms();
        let t = &self.p.timer;
        let rem = t.remaining_ms(now, &self.cfg);
        let mins = rem.div_ceil(60_000);
        let (left, icon_label) = if t.run != Run::Idle && rem < 60_000 {
            let secs = rem.div_ceil(1000);
            (format!("faltam {secs} s"), secs.to_string())
        } else {
            (format!("faltam {mins} min"), mins.to_string())
        };
        let (today_count, today_min) = self.stats.today();
        Snap {
            phase: t.phase,
            run: t.kind(),
            left,
            icon_label,
            frac_q: (t.fraction(now, &self.cfg) * FRAC_STEPS).round() as u16,
            color: self.phase_color(t.phase),
            today_count,
            today_min,
            overlay_visible: self.p.ui.visible,
            locked: self.p.ui.locked,
            strict: t.strict_locked(&self.cfg),
            outputs: self.output_names(),
            output: self.ov.as_ref().map(|o| o.output_name.clone()).unwrap_or_else(|| self.p.ui.output.clone()),
            blocking: self.blocker.active(),
        }
    }

    /// Cria ou tira o ícone da bandeja conforme a configuração.
    fn sync_tray(&mut self) {
        use ksni::blocking::TrayMethods;
        if !self.cfg.appearance.tray_icon {
            if let Some(t) = self.tray.take() {
                t.shutdown().wait();
            }
            return;
        }
        if self.tray.is_some() {
            return;
        }
        let snap = self.snapshot();
        match (FocoTray { snap: snap.clone(), tx: self.tx.clone() }).spawn() {
            Ok(handle) => {
                self.tray = Some(handle);
                self.tray_snap = snap;
            }
            Err(e) => eprintln!("foco: sem ícone na bandeja ({e})"),
        }
    }

    pub fn refresh_tray(&mut self) {
        let snap = self.snapshot();
        if snap == self.tray_snap {
            return;
        }
        if let Some(tray) = &self.tray {
            let s = snap.clone();
            tray.update(move |t| t.snap = s);
        }
        self.tray_snap = snap;
    }

    // ---------- overlay ----------

    fn phase_color(&self, phase: Phase) -> [u8; 3] {
        let a = &self.cfg.appearance;
        config::parse_color(match phase {
            Phase::Focus => &a.focus_color,
            Phase::ShortBreak => &a.short_break_color,
            Phase::LongBreak => &a.long_break_color,
        })
    }

    pub fn layer_kind(&self) -> Layer {
        if self.cfg.appearance.above_fullscreen { Layer::Overlay } else { Layer::Top }
    }

    pub fn outputs(&self) -> Vec<Output> {
        match &self.disp {
            Disp::Wl(w) => w.outputs(),
            Disp::X(x) => x.outputs.clone(),
            Disp::Nenhum => Vec::new(),
        }
    }

    fn output_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.outputs().into_iter().map(|o| o.name).collect();
        v.sort();
        v
    }

    fn output_by_name(&self, name: &str) -> Option<Output> {
        if name.is_empty() {
            return None;
        }
        self.outputs().into_iter().find(|o| o.name == name)
    }

    pub fn output_size(&self) -> Option<(i32, i32)> {
        let name = self.ov.as_ref().map(|o| o.output_name.as_str()).unwrap_or(&self.p.ui.output);
        self.output_by_name(name).map(|o| o.size)
    }

    pub fn clamp_margin(&self, (x, y): (i32, i32), size: u32) -> (i32, i32) {
        match self.output_size() {
            Some((w, h)) => {
                let s = size as i32;
                (x.clamp(0, (w - s).max(0)), y.clamp(0, (h - s).max(0)))
            }
            None => (x.max(0), y.max(0)),
        }
    }

    pub fn default_position(out: (i32, i32), size: u32) -> (i32, i32) {
        (out.0 - size as i32 - 28, 64)
    }

    pub fn show_overlay(&mut self) {
        if self.ov.is_some() || matches!(self.disp, Disp::Nenhum) {
            return;
        }
        let size = self.p.ui.size.clamp(MIN_SIZE, MAX_SIZE);
        let mut output = self.output_by_name(&self.p.ui.output);
        if output.is_none() && matches!(self.disp, Disp::X(_)) {
            // no Wayland o compositor escolhe o monitor; no X11 a escolha é nossa: o principal
            let outputs = self.outputs();
            output = outputs.iter().find(|o| o.primary).or(outputs.first()).cloned();
            self.p.ui.output = output.as_ref().map(|o| o.name.clone()).unwrap_or_default();
        }
        let output_name = output.as_ref().map(|o| o.name.clone()).unwrap_or_default();
        if let Some(out) = &output
            && !self.p.ui.positioned
        {
            (self.p.ui.x, self.p.ui.y) = Self::default_position(out.size, size);
            self.p.ui.positioned = true;
        }
        let mut margin = (self.p.ui.x, self.p.ui.y);
        if let Some(Output { size: (w, h), .. }) = &output {
            margin = (margin.0.clamp(0, (w - size as i32).max(0)), margin.1.clamp(0, (h - size as i32).max(0)));
        } else if !self.p.ui.positioned {
            margin = (80, 80);
        }

        let layer = self.layer_kind();
        let (surf, scale) = match &mut self.disp {
            Disp::Wl(w) => (Surf::Wl(w.create_layer(size, margin, &output_name, layer)), 1),
            Disp::X(x) => {
                let origin = output.as_ref().map(|o| o.pos).unwrap_or((0, 0));
                match x.create(size, (origin.0 + margin.0, origin.1 + margin.1)) {
                    Ok(win) => (Surf::X(win), x.scale),
                    Err(e) => {
                        config::log(&format!("overlay X11: {e}"));
                        return;
                    }
                }
            }
            Disp::Nenhum => return,
        };
        // a janela X11 já nasce com tamanho e lugar; a layer espera o configure
        let configured = matches!(surf, Surf::X(_));

        self.ov_serial += 1;
        self.ov = Some(Overlay {
            surf,
            serial: self.ov_serial,
            configured,
            size,
            want_size: size,
            size_pending: false,
            scale,
            output_name,
            margin,
            want_margin: margin,
            effective: margin,
            margin_inflight: false,
            frame_pending: false,
            dirty: true,
            hover: false,
            hot: Hit::None,
            press: None,
            region_for: None,
            pix: None,
            cursor: None,
        });
        self.hover_t = 0.0;
        self.draw();
    }

    /// X11: volta o círculo para o topo se alguma janela o cobriu. Com "por cima de tela cheia"
    /// desligado, deixa uma janela em tela cheia ficar na frente.
    pub fn keep_on_top(&mut self) {
        let above_fullscreen = self.cfg.appearance.above_fullscreen;
        if let (Disp::X(x), Some(Overlay { surf: Surf::X(win), .. })) = (&mut self.disp, &self.ov)
            && (above_fullscreen || !x.fullscreen_active())
        {
            x.raise(win);
        }
    }

    pub fn hide_overlay(&mut self) {
        let Some(ov) = self.ov.take() else { return };
        if let (Surf::X(win), Disp::X(x)) = (&ov.surf, &self.disp) {
            x.destroy(win);
        }
    }

    pub fn request_redraw(&mut self) {
        if let Some(ov) = self.ov.as_mut() {
            ov.dirty = true;
        }
        self.draw();
    }

    fn animating(&self) -> bool {
        let target = if self.ov.as_ref().is_some_and(|o| o.hover) { 1.0 } else { 0.0 };
        self.flash_t > 0.0 || (self.hover_t - target).abs() > 0.001
    }

    fn step_animation(&mut self) {
        let now = Instant::now();
        let dt = self.last_anim.map(|t| (now - t).as_secs_f32()).unwrap_or(0.0).min(0.1);
        self.last_anim = Some(now);
        let target = if self.ov.as_ref().is_some_and(|o| o.hover) { 1.0 } else { 0.0 };
        let step = dt / HOVER_SECS;
        if self.hover_t < target {
            self.hover_t = (self.hover_t + step).min(target);
        } else {
            self.hover_t = (self.hover_t - step).max(target);
        }
        if self.flash_t > 0.0 {
            self.flash_t = (self.flash_t - dt / FLASH_SECS).max(0.0);
        }
        if !self.animating() {
            self.last_anim = None;
        }
    }

    fn view(&self) -> View {
        let now = now_ms();
        let t = &self.p.timer;
        let ov = self.ov.as_ref();
        let every = self.cfg.long_break_every;
        // dois pulsos que se apagam
        let f = 1.0 - self.flash_t;
        let flash = if self.flash_t > 0.0 {
            self.flash_t * (0.5 - 0.5 * (f * std::f32::consts::TAU * 2.0).cos())
        } else {
            0.0
        };
        let h = self.hover_t;
        View {
            phase: t.phase,
            run: t.kind(),
            clock: format_clock(t.remaining_ms(now, &self.cfg)),
            fraction: t.fraction(now, &self.cfg),
            done_in_cycle: t.done_in_cycle.min(every),
            every,
            color: self.phase_color(t.phase),
            bg_opacity: self.cfg.appearance.background_opacity,
            hover: h * h * (3.0 - 2.0 * h),
            hot: ov.map(|o| o.hot).unwrap_or_default(),
            flash,
            blocking: self.blocker.active(),
        }
    }

    pub fn draw(&mut self) {
        if self.ov.as_ref().is_none_or(|o| !o.configured || o.frame_pending) {
            return;
        }
        if self.animating() {
            self.step_animation();
        }
        let view = self.view();
        let idle = self.cfg.appearance.idle_opacity;
        let opacity = idle + (1.0 - idle) * self.hover_t;
        let locked = self.p.ui.locked;
        let animating = self.animating();
        let Some(ov) = self.ov.as_mut() else { return };
        if !ov.dirty && !animating {
            return;
        }

        let px = (ov.size as i32 * ov.scale.max(1)) as u32;
        if ov.pix.as_ref().is_none_or(|p| p.width() != px) {
            ov.pix = Pixmap::new(px, px);
        }
        let Some(pix) = ov.pix.as_mut() else { return };
        render::draw_overlay(pix, &view);
        match &mut self.disp {
            Disp::Wl(w) => w.present(ov, opacity, locked),
            Disp::X(x) => x.present(ov, opacity, locked),
            Disp::Nenhum => {}
        }
        ov.dirty = false;

        if animating && matches!(self.disp, Disp::X(_)) && self.anim_tick.is_none() {
            self.anim_tick = self
                .lh
                .insert_source(Timer::from_duration(Duration::from_millis(16)), |_, _, app| {
                    app.anim_tick = None;
                    app.draw();
                    TimeoutAction::Drop
                })
                .ok();
        }
    }

    /// Envia a margem desejada se não houver outra esperando confirmação.
    pub fn flush_margin(&mut self) {
        let Some(ov) = self.ov.as_mut() else { return };
        if ov.margin_inflight || ov.want_margin == ov.margin {
            return;
        }
        ov.margin = ov.want_margin;
        match &mut self.disp {
            Disp::Wl(w) => w.send_margin(ov),
            Disp::X(x) => x.place(ov),
            Disp::Nenhum => {}
        }
    }

    /// Muda o tamanho; com `keep_center` o círculo cresce em volta do próprio centro.
    fn resize(&mut self, size: u32, keep_center: bool) {
        let size = size.clamp(MIN_SIZE, MAX_SIZE);
        let Some(ov) = self.ov.as_ref() else {
            self.p.ui.size = size;
            return;
        };
        let old = ov.want_size;
        if size == old {
            return;
        }
        let mut m = ov.want_margin;
        if keep_center {
            let d = (old as i32 - size as i32) / 2;
            m = (m.0 + d, m.1 + d);
        }
        let m = self.clamp_margin(m, size);
        if let Some(ov) = self.ov.as_mut() {
            ov.want_size = size;
            ov.want_margin = m;
        }
        self.p.ui.size = size;
        (self.p.ui.x, self.p.ui.y) = m;
        self.flush_size();
    }

    pub fn flush_size(&mut self) {
        let Some(ov) = self.ov.as_mut() else { return };
        if ov.size_pending || ov.want_size == ov.size {
            return;
        }
        match &mut self.disp {
            Disp::Wl(w) => w.send_size(ov),
            Disp::X(x) => {
                // no X11 o tamanho vale na hora
                ov.size = ov.want_size;
                ov.margin = ov.want_margin;
                ov.region_for = None;
                ov.dirty = true;
                x.place(ov);
                self.draw();
            }
            Disp::Nenhum => {}
        }
    }

    fn set_cursor(&mut self, icon: CursorIcon) {
        let Some(ov) = self.ov.as_mut() else { return };
        if ov.cursor == Some(icon) {
            return;
        }
        ov.cursor = Some(icon);
        match &self.disp {
            Disp::Wl(w) => w.set_cursor(icon),
            Disp::X(x) => x.set_cursor(ov, icon),
            Disp::Nenhum => {}
        }
    }

    fn cursor_for(hit: Hit, dragging: bool) -> CursorIcon {
        match hit {
            _ if dragging => CursorIcon::Grabbing,
            Hit::Resize => CursorIcon::NwseResize,
            Hit::Body => CursorIcon::Pointer,
            Hit::None => CursorIcon::Default,
        }
    }

    /// Mouse sobre o overlay, em coordenadas lógicas da superfície.
    pub fn on_pointer(&mut self, ev: Ptr, x: f64, y: f64) {
        let Some(ov) = self.ov.as_mut() else { return };
        let geo = Geo::new(ov.size as f32);
        match ev {
            Ptr::Enter => {
                ov.hover = true;
                ov.cursor = None;
                ov.hot = geo.hit(x as f32, y as f32);
                let icon = Self::cursor_for(ov.hot, false);
                self.set_cursor(icon);
                self.request_redraw();
            }
            Ptr::Leave => {
                if ov.press.is_none() {
                    ov.hover = false;
                    ov.hot = Hit::None;
                    self.request_redraw();
                }
            }
            Ptr::Motion => {
                if let Some(press) = ov.press.as_mut() {
                    if press.button != BTN_LEFT {
                        return;
                    }
                    if press.target == Hit::Resize {
                        let d = ((x - press.pos.0) + (y - press.pos.1)) / 2.0;
                        let size = (press.size0 as f64 + d / geo_frac() as f64).round().max(0.0) as u32;
                        self.resize(size, false);
                        return;
                    }
                    if !press.dragging && (x - press.pos.0).hypot(y - press.pos.1) > 4.0 {
                        press.dragging = true;
                        self.set_cursor(CursorIcon::Grabbing);
                    }
                    let Some(ov) = self.ov.as_mut() else { return };
                    let Some(press) = ov.press.as_ref() else { return };
                    if press.dragging {
                        let want = (
                            (ov.effective.0 as f64 + x - press.pos.0).round() as i32,
                            (ov.effective.1 as f64 + y - press.pos.1).round() as i32,
                        );
                        let size = ov.want_size;
                        let m = self.clamp_margin(want, size);
                        if let Some(ov) = self.ov.as_mut() {
                            ov.want_margin = m;
                        }
                        self.flush_margin();
                    }
                } else {
                    let hot = geo.hit(x as f32, y as f32);
                    if hot != ov.hot {
                        ov.hot = hot;
                        self.set_cursor(Self::cursor_for(hot, false));
                        self.request_redraw();
                    }
                }
            }
            Ptr::Press(button) => {
                if ov.press.is_none() {
                    let target = geo.hit(x as f32, y as f32);
                    ov.press = Some(Press { button, pos: (x, y), target, dragging: false, size0: ov.want_size });
                    self.request_redraw();
                }
            }
            Ptr::Release(button, serial) => {
                let Some(press) = ov.press.take_if(|p| p.button == button) else { return };
                let inside = geo.hit(x as f32, y as f32);
                ov.hot = inside;
                if inside == Hit::None {
                    ov.hover = false;
                }
                let moved = press.dragging || press.target == Hit::Resize;
                if moved {
                    (self.p.ui.x, self.p.ui.y) = ov.want_margin;
                    self.p.ui.size = ov.want_size;
                    self.p.ui.positioned = true;
                    self.p.save();
                }
                let icon = Self::cursor_for(inside, false);
                self.set_cursor(icon);
                self.request_redraw();
                if moved {
                    return;
                }
                // qualquer botão abre a central; iniciar e pausar ficam lá
                if inside == Hit::Body && press.target == Hit::Body && matches!(button, BTN_LEFT | BTN_RIGHT) {
                    self.open_central(serial);
                }
            }
            Ptr::Scroll(steps) => {
                if steps.abs() > f64::EPSILON {
                    let size = (ov.want_size as f64 * 1.07f64.powf(-steps)).round() as u32;
                    self.resize(size, true);
                    self.p.save();
                }
            }
        }
    }

    /// Os monitores mudaram: mantém o círculo dentro da tela (ou recria, se o dele sumiu).
    pub fn outputs_changed(&mut self) {
        if let Some(ov) = self.ov.as_ref() {
            let gone = !ov.output_name.is_empty() && self.output_by_name(&ov.output_name).is_none();
            if gone && matches!(self.disp, Disp::X(_)) {
                self.hide_overlay();
                self.p.ui.output.clear();
                self.p.ui.positioned = false;
                if self.p.ui.visible {
                    self.show_overlay();
                }
            } else {
                let m = self.clamp_margin(ov.want_margin, ov.want_size);
                if let Some(ov) = self.ov.as_mut() {
                    ov.want_margin = m;
                    // reposiciona mesmo sem mudar a margem: a origem do monitor pode ter mudado
                    ov.margin = (i32::MIN, i32::MIN);
                }
                self.flush_margin();
            }
        }
        self.refresh_tray();
    }
}

fn geo_frac() -> f32 {
    Geo::new(1.0).grip_frac
}
