//! Overlay no Wayland com layer-shell (COSMIC, KDE, Hyprland, Sway, niri, labwc…).

use smithay_client_toolkit::{
    activation::{ActivationHandler, ActivationState, RequestData},
    compositor::{CompositorHandler, CompositorState, FrameCallbackData, Region},
    delegate_registry,
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::LoopHandle,
        calloop_wayland_source::WaylandSource,
        client::{
            Connection, EventQueue, QueueHandle,
            globals::registry_queue_init,
            protocol::{wl_callback, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{CursorIcon, PointerEvent, PointerEventKind, PointerHandler, ThemeSpec, ThemedPointer},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};

use crate::app::{App, Output, Overlay, Ptr, Surf};
use crate::notify;
use crate::render::{self, Geo};

pub struct Wl {
    pub conn: Connection,
    qh: QueueHandle<App>,
    registry: RegistryState,
    seats: SeatState,
    outputs: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    pool: Option<SlotPool>,
    pointer: Option<ThemedPointer>,
    seat: Option<wl_seat::WlSeat>,
    activation: Option<ActivationState>,
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

pub fn insert_source(conn: Connection, queue: EventQueue<App>, lh: &LoopHandle<'static, App>) -> Result<(), String> {
    WaylandSource::new(conn, queue).insert(lh.clone()).map(|_| ()).map_err(|e| e.to_string())
}

/// O layer-surface de um overlay (só existe no caminho Wayland).
fn layer_of(ov: &Overlay) -> &LayerSurface {
    match &ov.surf {
        Surf::Wl(layer) => layer,
        Surf::X(_) => unreachable!("overlay X11 no caminho Wayland"),
    }
}

impl Wl {
    /// Conecta e confere se dá para usar: sem layer-shell (GNOME) o overlay vai pelo X11.
    pub fn connect() -> Result<(Wl, EventQueue<App>), String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("sem Wayland: {e}"))?;
        let (globals, queue) = registry_queue_init::<App>(&conn).map_err(|e| e.to_string())?;
        let qh = queue.handle();
        let compositor = CompositorState::bind(&globals, &qh).map_err(|e| e.to_string())?;
        let layer_shell = LayerShell::bind(&globals, &qh).map_err(|_| "o compositor não tem layer-shell".to_string())?;
        let shm = Shm::bind(&globals, &qh).map_err(|e| e.to_string())?;
        let wl = Wl {
            registry: RegistryState::new(&globals),
            seats: SeatState::new(&globals, &qh),
            outputs: OutputState::new(&globals, &qh),
            activation: ActivationState::bind(&globals, &qh).ok(),
            conn,
            qh,
            compositor,
            layer_shell,
            shm,
            pool: None,
            pointer: None,
            seat: None,
        };
        Ok((wl, queue))
    }

    pub fn outputs(&self) -> Vec<Output> {
        self.outputs
            .outputs()
            .filter_map(|o| {
                let info = self.outputs.info(&o)?;
                Some(Output {
                    name: info.name?,
                    pos: info.logical_position.unwrap_or((0, 0)),
                    size: info.logical_size.unwrap_or((1920, 1080)),
                    primary: false,
                })
            })
            .collect()
    }

    fn output_by_name(&self, name: &str) -> Option<wl_output::WlOutput> {
        if name.is_empty() {
            return None;
        }
        self.outputs.outputs().find(|o| self.outputs.info(o).and_then(|i| i.name).as_deref() == Some(name))
    }

    pub fn create_layer(&mut self, size: u32, margin: (i32, i32), output: &str, kind: Layer) -> LayerSurface {
        let surface = self.compositor.create_surface(&self.qh);
        let wl_out = self.output_by_name(output);
        let layer = self.layer_shell.create_layer_surface(&self.qh, surface, kind, Some("foco"), wl_out.as_ref());
        layer.set_anchor(Anchor::TOP | Anchor::LEFT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(size, size);
        layer.set_margin(margin.1, 0, 0, margin.0);
        layer.commit();
        layer
    }

    pub fn present(&mut self, ov: &mut Overlay, opacity: f32, locked: bool) {
        let Some(pix) = ov.pix.as_ref() else { return };
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
        render::blit_argb(pix, canvas, opacity);

        let layer = layer_of(ov).clone();
        let surface = layer.wl_surface();
        if ov.region_for != Some((ov.size, locked))
            && let Ok(region) = Region::new(&self.compositor)
        {
            if !locked {
                for (x, y, w, h) in Geo::new(ov.size as f32).input_rects(ov.size as i32) {
                    region.add(x, y, w, h);
                }
            }
            surface.set_input_region(Some(region.wl_region()));
            ov.region_for = Some((ov.size, locked));
        }
        surface.set_buffer_scale(scale);
        surface.damage_buffer(0, 0, px, px);
        surface.frame(&self.qh, FrameCallbackData(surface.clone()));
        if let Err(e) = buffer.attach_to(surface) {
            eprintln!("foco: attach: {e}");
        }
        layer.commit();
        ov.frame_pending = true;
    }

    pub fn send_margin(&mut self, ov: &mut Overlay) {
        let layer = layer_of(ov);
        layer.set_margin(ov.margin.1, 0, 0, ov.margin.0);
        layer.commit();
        ov.margin_inflight = true;
        self.conn.display().sync(&self.qh, MarginSync { serial: ov.serial, margin: ov.margin });
    }

    pub fn send_size(&mut self, ov: &mut Overlay) {
        let layer = layer_of(ov).clone();
        layer.set_size(ov.want_size, ov.want_size);
        ov.size_pending = true;
        if !ov.margin_inflight && ov.want_margin != ov.margin {
            ov.margin = ov.want_margin;
            self.send_margin(ov);
        } else {
            layer.commit();
        }
    }

    pub fn set_cursor(&self, icon: CursorIcon) {
        if let Some(p) = &self.pointer {
            let _ = p.set_cursor(&self.conn, icon);
        }
    }

    /// Com um token de ativação a central ganha o foco mesmo se já estava aberta atrás de outras.
    pub fn open_central(&self, layer: &LayerSurface, serial: u32) {
        let (Some(act), Some(seat)) = (&self.activation, &self.seat) else {
            notify::open_settings(None, &["toggle"]);
            return;
        };
        act.request_token(
            &self.qh,
            RequestData {
                app_id: Some("dev.foco.Settings".into()),
                seat_and_serial: Some((seat.clone(), serial)),
                surface: Some(layer.wl_surface().clone()),
                udata: (),
            },
        );
    }
}

fn is_ours(app: &App, surface: &wl_surface::WlSurface) -> bool {
    app.ov.as_ref().is_some_and(|o| layer_of(o).wl_surface() == surface)
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, factor: i32) {
        if is_ours(self, surface)
            && let Some(ov) = self.ov.as_mut()
        {
            ov.scale = factor.max(1);
            ov.dirty = true;
        }
        self.draw();
    }

    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _: u32) {
        if is_ours(self, surface)
            && let Some(ov) = self.ov.as_mut()
        {
            ov.frame_pending = false;
        }
        self.draw();
    }

    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, output: &wl_output::WlOutput) {
        let Some(name) = self.wl().outputs.info(output).and_then(|i| i.name) else { return };
        if !is_ours(self, surface) {
            return;
        }
        let Some(ov) = self.ov.as_mut() else { return };
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
            let m = App::default_position(out, size);
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
        &mut self.wl().outputs
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
        if is_ours(self, layer.wl_surface()) {
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
        if !is_ours(self, layer.wl_surface()) {
            return;
        }
        let Some(ov) = self.ov.as_mut() else { return };
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
        &mut self.wl().seats
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(&mut self, _: &Connection, qh: &QueueHandle<Self>, seat: wl_seat::WlSeat, capability: Capability) {
        let wl = self.wl();
        if capability == Capability::Pointer && wl.pointer.is_none() {
            wl.seat = Some(seat.clone());
            let surface = wl.compositor.create_surface(qh);
            match wl.seats.get_pointer_with_theme::<_, ()>(qh, &seat, wl.shm.wl_shm(), surface, ThemeSpec::default()) {
                Ok(p) => wl.pointer = Some(p),
                Err(e) => eprintln!("foco: sem ponteiro: {e}"),
            }
        }
    }

    fn remove_capability(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat, capability: Capability) {
        if capability == Capability::Pointer {
            self.wl().pointer = None;
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_pointer::WlPointer, events: &[PointerEvent]) {
        for ev in events {
            if !is_ours(self, &ev.surface) {
                continue;
            }
            let (x, y) = ev.position;
            let ptr = match ev.kind {
                PointerEventKind::Enter { .. } => Ptr::Enter,
                PointerEventKind::Leave { .. } => Ptr::Leave,
                PointerEventKind::Motion { .. } => Ptr::Motion,
                PointerEventKind::Press { button, .. } => Ptr::Press(button),
                PointerEventKind::Release { button, serial, .. } => Ptr::Release(button, serial),
                PointerEventKind::Axis { vertical, .. } => Ptr::Scroll(if vertical.value120 != 0 {
                    vertical.value120 as f64 / 120.0
                } else if vertical.discrete != 0 {
                    vertical.discrete as f64
                } else {
                    vertical.absolute / 12.0
                }),
            };
            self.on_pointer(ptr, x, y);
        }
    }
}

impl ActivationHandler for App {
    type RequestUdata = ();

    fn new_token(&mut self, token: String, _: &RequestData<()>) {
        notify::open_settings(Some(&token), &["toggle"]);
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.wl().shm
    }
}

delegate_registry!(App);

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.wl().registry
    }
    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(App);
