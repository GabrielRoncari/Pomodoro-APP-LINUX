//! Laço principal: overlay em layer-shell (Wayland), timer, bandeja, socket e bloqueio.

use std::os::unix::net::UnixListener;
use std::time::{Duration, Instant, SystemTime};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData, Region},
    delegate_registry,
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            EventLoop, LoopHandle, RegistrationToken,
            channel,
            signals::{Signal, Signals},
            timer::{TimeoutAction, Timer},
        },
        calloop_wayland_source::WaylandSource,
        client::{
            Connection, QueueHandle,
            globals::registry_queue_init,
            protocol::{wl_callback, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{BTN_LEFT, BTN_RIGHT, CursorIcon, PointerEvent, PointerEventKind, PointerHandler, ThemeSpec, ThemedPointer},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
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

const HOVER_SECS: f32 = 0.16;
const FLASH_SECS: f32 = 1.8;

struct Press {
    button: u32,
    pos: (f64, f64),
    target: Hit,
    dragging: bool,
    size0: u32,
}

struct Overlay {
    layer: LayerSurface,
    serial: u64,
    configured: bool,
    /// Tamanho (lógico) do buffer atual.
    size: u32,
    /// Tamanho pedido mais recente; vira `size` no próximo configure.
    want_size: u32,
    size_pending: bool,
    scale: i32,
    output_name: String,
    /// Margens (esquerda, topo) já enviadas.
    margin: (i32, i32),
    want_margin: (i32, i32),
    /// Margens que o compositor já aplicou (confirmado por wl_display.sync).
    effective: (i32, i32),
    margin_inflight: bool,
    frame_pending: bool,
    dirty: bool,
    hover: bool,
    hot: Hit,
    press: Option<Press>,
    region_for: Option<(u32, bool)>,
    pix: Option<Pixmap>,
    cursor: Option<CursorIcon>,
}

/// Confirma que o compositor aplicou uma margem: a partir daí as coordenadas do mouse
/// são relativas à nova posição. É isso que deixa o arrasto sem tremedeira.
struct MarginSync {
    serial: u64,
    margin: (i32, i32),
}

impl Dispatch2<wl_callback::WlCallback, App> for MarginSync {
    fn event(
        &self,
        app: &mut App,
        _: &wl_callback::WlCallback,
        event: wl_callback::Event,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        if let wl_callback::Event::Done { .. } = event {
            if let Some(ov) = app.ov.as_mut().filter(|ov| ov.serial == self.serial) {
                ov.effective = self.margin;
                ov.margin_inflight = false;
            }
            app.flush_margin();
        }
    }
}

pub struct App {
    conn: Connection,
    qh: QueueHandle<App>,
    lh: LoopHandle<'static, App>,
    registry: RegistryState,
    seats: SeatState,
    outputs: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    pool: Option<SlotPool>,
    pointer: Option<ThemedPointer>,
    ov: Option<Overlay>,
    ov_serial: u64,

    cfg: Config,
    cfg_mtime: Option<SystemTime>,
    p: Persist,
    stats: Stats,
    blocker: Blocker,
    warned_block_missing: bool,
    tray: Option<ksni::blocking::Handle<FocoTray>>,
    tray_snap: Snap,
    tick: Option<RegistrationToken>,

    hover_t: f32,
    flash_t: f32,
    last_anim: Option<Instant>,
    exit: bool,
}

fn mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

pub fn run(listener: UnixListener) -> Result<(), Box<dyn std::error::Error>> {
    let conn = Connection::connect_to_env().map_err(|e| format!("sem conexão Wayland: {e}"))?;
    let (globals, mut queue) = registry_queue_init::<App>(&conn)?;
    let qh = queue.handle();
    let mut event_loop: EventLoop<'static, App> = EventLoop::try_new()?;
    let lh = event_loop.handle();

    let compositor = CompositorState::bind(&globals, &qh)?;
    let layer_shell = LayerShell::bind(&globals, &qh).map_err(|_| "o compositor não suporta layer-shell")?;
    let shm = Shm::bind(&globals, &qh)?;

    let (tx, rx) = channel::channel::<Msg>();
    lh.insert_source(rx, |ev, _, app| {
        if let channel::Event::Msg(msg) = ev {
            app.on_msg(msg);
        }
    })?;
    lh.insert_source(Signals::new(&[Signal::SIGTERM, Signal::SIGINT, Signal::SIGHUP])?, |_, _, app| {
        app.exit = true;
    })?;

    ipc::serve(listener, tx.clone());

    let btx = tx.clone();
    let blocker = Blocker::new(move |e| {
        let _ = btx.send(Msg { cmd: Cmd::BlockError(e), reply: None });
    });

    let cfg = Config::load();
    let mut app = App {
        conn: conn.clone(),
        qh: qh.clone(),
        lh: lh.clone(),
        registry: RegistryState::new(&globals),
        seats: SeatState::new(&globals, &qh),
        outputs: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        pool: None,
        pointer: None,
        ov: None,
        ov_serial: 0,
        cfg_mtime: mtime(&config::config_path()),
        cfg,
        p: Persist::load(),
        stats: Stats::load(),
        blocker,
        warned_block_missing: false,
        tray: None,
        tray_snap: Snap::default(),
        tick: None,
        hover_t: 0.0,
        flash_t: 0.0,
        last_anim: None,
        exit: false,
    };

    // nomes e tamanhos dos monitores chegam em eventos: espera por eles antes do overlay
    queue.roundtrip(&mut app)?;
    queue.roundtrip(&mut app)?;
    WaylandSource::new(conn, queue).insert(lh)?;

    use ksni::blocking::TrayMethods;
    let snap = app.snapshot();
    match (FocoTray { snap: snap.clone(), tx }).spawn() {
        Ok(handle) => {
            app.tray = Some(handle);
            app.tray_snap = snap;
        }
        Err(e) => eprintln!("foco: sem ícone na bandeja ({e})"),
    }

    app.startup();
    while !app.exit {
        event_loop.dispatch(None, &mut app)?;
    }
    app.shutdown();
    Ok(())
}

impl App {
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
                if self.p.ui.visible { self.show_overlay() } else { self.ov = None }
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
                    self.ov = None;
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
            "today": { "count": count, "minutes": minutes },
        })
    }

    fn reload_config(&mut self) {
        self.cfg_mtime = mtime(&config::config_path());
        let old = std::mem::replace(&mut self.cfg, Config::load());
        if old.appearance.above_fullscreen != self.cfg.appearance.above_fullscreen
            && let Some(ov) = &self.ov
        {
            ov.layer.set_layer(self.layer_kind());
            ov.layer.commit();
        }
    }

    fn open_settings(&self) {
        notify::open_settings();
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
                    "Abra as configurações do Foco → Bloqueio e clique em Instalar (pede a senha uma vez).",
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

    fn refresh_tray(&mut self) {
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

    fn layer_kind(&self) -> Layer {
        if self.cfg.appearance.above_fullscreen { Layer::Overlay } else { Layer::Top }
    }

    fn output_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.outputs.outputs().filter_map(|o| self.outputs.info(&o)?.name).collect();
        v.sort();
        v
    }

    fn output_by_name(&self, name: &str) -> Option<(wl_output::WlOutput, (i32, i32))> {
        if name.is_empty() {
            return None;
        }
        self.outputs.outputs().find_map(|o| {
            let info = self.outputs.info(&o)?;
            (info.name.as_deref() == Some(name)).then(|| (o.clone(), info.logical_size.unwrap_or((1920, 1080))))
        })
    }

    fn output_size(&self) -> Option<(i32, i32)> {
        let name = self.ov.as_ref().map(|o| o.output_name.as_str()).unwrap_or(&self.p.ui.output);
        self.output_by_name(name).map(|(_, size)| size)
    }

    fn clamp_margin(&self, (x, y): (i32, i32), size: u32) -> (i32, i32) {
        match self.output_size() {
            Some((w, h)) => {
                let s = size as i32;
                (x.clamp(0, (w - s).max(0)), y.clamp(0, (h - s).max(0)))
            }
            None => (x.max(0), y.max(0)),
        }
    }

    fn default_position(out: (i32, i32), size: u32) -> (i32, i32) {
        (out.0 - size as i32 - 28, 64)
    }

    fn show_overlay(&mut self) {
        if self.ov.is_some() {
            return;
        }
        let size = self.p.ui.size.clamp(MIN_SIZE, MAX_SIZE);
        let output = self.output_by_name(&self.p.ui.output);
        let output_name = if output.is_some() { self.p.ui.output.clone() } else { String::new() };
        if let Some((_, out)) = &output
            && !self.p.ui.positioned
        {
            (self.p.ui.x, self.p.ui.y) = Self::default_position(*out, size);
            self.p.ui.positioned = true;
        }
        let mut margin = (self.p.ui.x, self.p.ui.y);
        if let Some((_, (w, h))) = &output {
            margin = (margin.0.clamp(0, (w - size as i32).max(0)), margin.1.clamp(0, (h - size as i32).max(0)));
        } else if !self.p.ui.positioned {
            margin = (80, 80);
        }

        let surface = self.compositor.create_surface(&self.qh);
        let layer = self.layer_shell.create_layer_surface(
            &self.qh,
            surface,
            self.layer_kind(),
            Some("foco"),
            output.as_ref().map(|(o, _)| o),
        );
        layer.set_anchor(Anchor::TOP | Anchor::LEFT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(size, size);
        layer.set_margin(margin.1, 0, 0, margin.0);
        layer.commit();

        self.ov_serial += 1;
        self.ov = Some(Overlay {
            layer,
            serial: self.ov_serial,
            configured: false,
            size,
            want_size: size,
            size_pending: false,
            scale: 1,
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
    }

    fn request_redraw(&mut self) {
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
            pressed: ov.and_then(|o| o.press.as_ref()).map(|p| p.target).unwrap_or_default(),
            flash,
            strict: t.strict_locked(&self.cfg),
            blocking: self.blocker.active(),
        }
    }

    fn draw(&mut self) {
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

        let scale = ov.scale.max(1);
        let px = ov.size as i32 * scale;
        let pool = match &mut self.pool {
            Some(p) => p,
            None => match SlotPool::new((px * px * 4) as usize, &self.shm) {
                Ok(p) => self.pool.insert(p),
                Err(e) => {
                    eprintln!("foco: sem memória compartilhada: {e}");
                    return;
                }
            },
        };
        let (buffer, canvas) = match pool.create_buffer(px, px, px * 4, wl_shm::Format::Argb8888) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("foco: buffer: {e}");
                return;
            }
        };
        if ov.pix.as_ref().is_none_or(|p| p.width() != px as u32) {
            ov.pix = Pixmap::new(px as u32, px as u32);
        }
        let Some(pix) = ov.pix.as_mut() else { return };
        render::draw_overlay(pix, &view);
        render::blit_argb(pix, canvas, opacity);

        let surface = ov.layer.wl_surface();
        if ov.region_for != Some((ov.size, locked)) {
            if let Ok(region) = Region::new(&self.compositor) {
                if !locked {
                    for (x, y, w, h) in Geo::new(ov.size as f32).input_rects(ov.size as i32) {
                        region.add(x, y, w, h);
                    }
                }
                surface.set_input_region(Some(region.wl_region()));
                ov.region_for = Some((ov.size, locked));
            }
        }
        surface.set_buffer_scale(scale);
        surface.damage_buffer(0, 0, px, px);
        surface.frame(&self.qh, FrameCallbackData(surface.clone()));
        ov.frame_pending = true;
        ov.dirty = false;
        if let Err(e) = buffer.attach_to(surface) {
            eprintln!("foco: attach: {e}");
        }
        ov.layer.commit();
    }

    /// Envia a margem desejada se não houver outra esperando confirmação.
    fn flush_margin(&mut self) {
        let Some(ov) = self.ov.as_mut() else { return };
        if ov.margin_inflight || ov.want_margin == ov.margin {
            return;
        }
        ov.margin = ov.want_margin;
        ov.layer.set_margin(ov.margin.1, 0, 0, ov.margin.0);
        ov.layer.commit();
        ov.margin_inflight = true;
        self.conn.display().sync(&self.qh, MarginSync { serial: ov.serial, margin: ov.margin });
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

    fn flush_size(&mut self) {
        let Some(ov) = self.ov.as_mut() else { return };
        if ov.size_pending || ov.want_size == ov.size {
            return;
        }
        ov.layer.set_size(ov.want_size, ov.want_size);
        ov.size_pending = true;
        if !ov.margin_inflight && ov.want_margin != ov.margin {
            ov.margin = ov.want_margin;
            ov.layer.set_margin(ov.margin.1, 0, 0, ov.margin.0);
            ov.margin_inflight = true;
            ov.layer.commit();
            self.conn.display().sync(&self.qh, MarginSync { serial: ov.serial, margin: ov.margin });
        } else {
            ov.layer.commit();
        }
    }

    fn set_cursor(&mut self, icon: CursorIcon) {
        let Some(ov) = self.ov.as_mut() else { return };
        if ov.cursor == Some(icon) {
            return;
        }
        ov.cursor = Some(icon);
        if let Some(p) = &self.pointer {
            let _ = p.set_cursor(&self.conn, icon);
        }
    }

    fn cursor_for(hit: Hit, dragging: bool) -> CursorIcon {
        match hit {
            _ if dragging => CursorIcon::Grabbing,
            Hit::Resize => CursorIcon::NwseResize,
            Hit::Toggle | Hit::Reset | Hit::Skip => CursorIcon::Pointer,
            _ => CursorIcon::Grab,
        }
    }

    fn on_pointer(&mut self, ev: &PointerEvent) {
        let strict = self.p.timer.strict_locked(&self.cfg);
        let Some(ov) = self.ov.as_mut() else { return };
        if &ev.surface != ov.layer.wl_surface() {
            return;
        }
        let (x, y) = ev.position;
        let geo = Geo::new(ov.size as f32);
        match ev.kind {
            PointerEventKind::Enter { .. } => {
                ov.hover = true;
                ov.cursor = None;
                ov.hot = geo.hit(x as f32, y as f32, true, strict);
                let icon = Self::cursor_for(ov.hot, false);
                self.set_cursor(icon);
                self.request_redraw();
            }
            PointerEventKind::Leave { .. } => {
                if ov.press.is_none() {
                    ov.hover = false;
                    ov.hot = Hit::None;
                    self.request_redraw();
                }
            }
            PointerEventKind::Motion { .. } => {
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
                    let hot = geo.hit(x as f32, y as f32, true, strict);
                    if hot != ov.hot {
                        ov.hot = hot;
                        self.set_cursor(Self::cursor_for(hot, false));
                        self.request_redraw();
                    }
                }
            }
            PointerEventKind::Press { button, .. } => {
                if ov.press.is_none() {
                    let target = geo.hit(x as f32, y as f32, true, strict);
                    ov.press = Some(Press { button, pos: (x, y), target, dragging: false, size0: ov.want_size });
                    self.request_redraw();
                }
            }
            PointerEventKind::Release { button, .. } => {
                let Some(press) = ov.press.take_if(|p| p.button == button) else { return };
                let inside = geo.hit(x as f32, y as f32, true, strict);
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
                if button == BTN_RIGHT {
                    self.open_settings();
                } else if button == BTN_LEFT && inside == press.target {
                    let cmd = match inside {
                        Hit::Toggle | Hit::Body => Some(Cmd::Toggle),
                        Hit::Reset => Some(Cmd::Reset),
                        Hit::Skip => Some(Cmd::Skip),
                        _ => None,
                    };
                    if let Some(cmd) = cmd {
                        if strict {
                            self.flash_t = 0.35;
                            self.request_redraw();
                        } else {
                            self.on_msg(Msg { cmd, reply: None });
                        }
                    }
                }
            }
            PointerEventKind::Axis { vertical, .. } => {
                let steps = if vertical.value120 != 0 {
                    vertical.value120 as f64 / 120.0
                } else if vertical.discrete != 0 {
                    vertical.discrete as f64
                } else {
                    vertical.absolute / 12.0
                };
                if steps.abs() > f64::EPSILON {
                    let size = (ov.want_size as f64 * 1.07f64.powf(-steps)).round() as u32;
                    self.resize(size, true);
                    self.p.save();
                }
            }
        }
    }
}

fn geo_frac() -> f32 {
    Geo::new(1.0).grip_frac
}

// ---------- Wayland ----------

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, factor: i32) {
        if let Some(ov) = self.ov.as_mut().filter(|o| o.layer.wl_surface() == surface) {
            ov.scale = factor.max(1);
            ov.dirty = true;
        }
        self.draw();
    }

    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _: u32) {
        if let Some(ov) = self.ov.as_mut().filter(|o| o.layer.wl_surface() == surface) {
            ov.frame_pending = false;
        }
        self.draw();
    }

    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, output: &wl_output::WlOutput) {
        let Some(name) = self.outputs.info(output).and_then(|i| i.name) else { return };
        let Some(ov) = self.ov.as_mut().filter(|o| o.layer.wl_surface() == surface) else { return };
        if !ov.output_name.is_empty() {
            return;
        }
        // overlay criado sem monitor definido: adota o que o compositor escolheu
        ov.output_name = name.clone();
        self.p.ui.output = name;
        if !self.p.ui.positioned
            && let Some(out) = self.output_size()
        {
            let size = self.ov.as_ref().map(|o| o.want_size).unwrap_or(self.p.ui.size);
            let m = Self::default_position(out, size);
            (self.p.ui.x, self.p.ui.y) = m;
            self.p.ui.positioned = true;
            if let Some(ov) = self.ov.as_mut() {
                ov.want_margin = m;
            }
            self.flush_margin();
        }
        self.p.save();
        self.refresh_tray();
    }

    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.refresh_tray();
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        // resolução/rotação mudou: mantém o círculo dentro da tela
        if let Some(ov) = self.ov.as_ref() {
            let m = self.clamp_margin(ov.want_margin, ov.want_size);
            if let Some(ov) = self.ov.as_mut() {
                ov.want_margin = m;
            }
            self.flush_margin();
        }
        self.refresh_tray();
    }

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.refresh_tray();
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        if self.ov.as_ref().is_some_and(|o| o.layer.wl_surface() == layer.wl_surface()) {
            // o monitor sumiu: recria no monitor que o compositor escolher
            self.ov = None;
            self.p.ui.output.clear();
            self.p.ui.positioned = false;
            if self.p.ui.visible {
                self.show_overlay();
            }
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(ov) = self.ov.as_mut().filter(|o| o.layer.wl_surface() == layer.wl_surface()) else { return };
        let w = configure.new_size.0;
        let new_size = if w > 0 { w } else { ov.want_size };
        if new_size != ov.size {
            ov.region_for = None;
        }
        ov.size = new_size;
        ov.size_pending = false;
        ov.configured = true;
        ov.dirty = true;
        // um configure novo pede um buffer novo já, sem esperar o frame
        ov.frame_pending = false;
        self.draw();
        self.flush_size();
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            let surface = self.compositor.create_surface(qh);
            match self.seats.get_pointer_with_theme::<_, ()>(qh, &seat, self.shm.wl_shm(), surface, ThemeSpec::default()) {
                Ok(p) => self.pointer = Some(p),
                Err(e) => eprintln!("foco: sem ponteiro: {e}"),
            }
        }
    }

    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Pointer {
            self.pointer = None;
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for ev in events {
            self.on_pointer(ev);
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_registry!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(App);
