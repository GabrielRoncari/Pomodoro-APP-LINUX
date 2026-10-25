//! Overlay numa janela X11 sempre por cima. Serve nas sessões X11 (Cinnamon, XFCE, MATE, KDE X11…)
//! e no GNOME, que não tem layer-shell, pelo Xwayland.
//!
//! A janela é override-redirect: o gerenciador de janelas não põe moldura, não dá foco e não a
//! mostra na barra de tarefas. Por isso o Foco mesmo a mantém no topo e escolhe o monitor.

use std::borrow::Cow;
use std::os::fd::{AsFd, OwnedFd};
use std::time::{Duration, Instant};

use smithay_client_toolkit::reexports::calloop::{Interest, LoopHandle, Mode, PostAction, generic::Generic};
use smithay_client_toolkit::seat::pointer::CursorIcon;
use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::cursor::Handle as CursorHandle;
use x11rb::image::{BitsPerPixel, Image, ImageOrder, ScanlinePad};
use x11rb::protocol::Event;
use x11rb::protocol::randr::{self, ConnectionExt as _};
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::*;
use x11rb::resource_manager;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::app::{App, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, Disp, Output, Overlay, Ptr, Surf};
use crate::config;
use crate::notify;
use crate::render::{self, Geo};

type Error = Box<dyn std::error::Error>;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        UTF8_STRING,
        _NET_WM_NAME,
        _NET_WM_PID,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_UTILITY,
        _NET_WM_STATE,
        _NET_WM_STATE_ABOVE,
        _NET_WM_STATE_STICKY,
        _NET_WM_STATE_SKIP_TASKBAR,
        _NET_WM_STATE_SKIP_PAGER,
        _NET_WM_STATE_FULLSCREEN,
        _NET_ACTIVE_WINDOW,
    }
}

struct Cursors {
    default: Cursor,
    pointer: Cursor,
    grabbing: Cursor,
    resize: Cursor,
}

pub struct Win {
    id: Window,
    gc: Gcontext,
}

pub struct X11 {
    conn: RustConnection,
    screen: usize,
    root: Window,
    visual: Visualid,
    depth: u8,
    colormap: Colormap,
    /// Há um compositor: a transparência funciona. Sem ele o círculo é recortado com Shape.
    composited: bool,
    has_shape: bool,
    has_randr: bool,
    /// Pixels físicos por pixel lógico (do Xft.dpi).
    pub scale: i32,
    /// Monitores em pixels lógicos.
    pub outputs: Vec<Output>,
    cursors: Cursors,
    atoms: Atoms,
    last_raise: Instant,
}

impl X11 {
    pub fn connect() -> Result<X11, Error> {
        let (conn, screen) = RustConnection::connect(None).map_err(|e| format!("sem X11: {e}"))?;
        let scr = conn.setup().roots[screen].clone();
        let root = scr.root;

        // visual de 32 bits para a transparência
        let argb = scr
            .allowed_depths
            .iter()
            .filter(|d| d.depth == 32)
            .flat_map(|d| d.visuals.iter())
            .find(|v| v.class == VisualClass::TRUE_COLOR)
            .map(|v| v.visual_id);
        let (visual, depth, colormap) = match argb {
            Some(v) => {
                let cm = conn.generate_id()?;
                conn.create_colormap(ColormapAlloc::NONE, cm, root, v)?;
                (v, 32, cm)
            }
            None => (scr.root_visual, scr.root_depth, scr.default_colormap),
        };

        let atoms = Atoms::new(&conn)?.reply()?;
        let cm_owner = conn.intern_atom(false, format!("_NET_WM_CM_S{screen}").as_bytes())?.reply()?.atom;
        let composited = argb.is_some() && conn.get_selection_owner(cm_owner)?.reply()?.owner != x11rb::NONE;
        let has_shape = conn.extension_information(shape::X11_EXTENSION_NAME)?.is_some();
        let has_randr = conn.extension_information(randr::X11_EXTENSION_NAME)?.is_some();

        let db = resource_manager::new_from_default(&conn)?;
        let dpi: f64 = db.get_value("Xft.dpi", "").ok().flatten().unwrap_or(96.0);
        let scale = ((dpi / 96.0).round() as i32).clamp(1, 4);

        let handle = CursorHandle::new(&conn, screen, &db)?.reply()?;
        let load = |names: &[&str]| {
            names
                .iter()
                .find_map(|n| handle.load_cursor(&conn, n).ok().filter(|c| *c != x11rb::NONE))
                .unwrap_or(x11rb::NONE)
        };
        let cursors = Cursors {
            default: load(&["default", "left_ptr"]),
            pointer: load(&["pointer", "hand2", "hand1"]),
            grabbing: load(&["grabbing", "closedhand", "fleur"]),
            resize: load(&["nwse-resize", "bottom_right_corner", "size_fdiag"]),
        };

        if has_randr {
            conn.randr_select_input(
                root,
                randr::NotifyMask::SCREEN_CHANGE | randr::NotifyMask::CRTC_CHANGE | randr::NotifyMask::OUTPUT_CHANGE,
            )?;
        }

        let mut x = X11 {
            conn,
            screen,
            root,
            visual,
            depth,
            colormap,
            composited,
            has_shape,
            has_randr,
            scale,
            outputs: Vec::new(),
            cursors,
            atoms,
            last_raise: Instant::now() - Duration::from_secs(1),
        };
        x.outputs = x.query_outputs();
        x.conn.flush()?;
        Ok(x)
    }

    fn query_outputs(&self) -> Vec<Output> {
        let s = self.scale;
        if self.has_randr
            && let Ok(reply) = self.conn.randr_get_monitors(self.root, true).map(|c| c.reply())
            && let Ok(reply) = reply
        {
            let mut v = Vec::new();
            for m in reply.monitors {
                let name = self
                    .conn
                    .get_atom_name(m.name)
                    .ok()
                    .and_then(|c| c.reply().ok())
                    .map(|r| String::from_utf8_lossy(&r.name).into_owned())
                    .unwrap_or_else(|| format!("monitor-{}", v.len() + 1));
                v.push(Output {
                    name,
                    pos: (m.x as i32 / s, m.y as i32 / s),
                    size: (m.width as i32 / s, m.height as i32 / s),
                    primary: m.primary,
                });
            }
            if !v.is_empty() {
                return v;
            }
        }
        let scr = &self.conn.setup().roots[self.screen];
        vec![Output {
            name: "X11".into(),
            pos: (0, 0),
            size: (scr.width_in_pixels as i32 / s, scr.height_in_pixels as i32 / s),
            primary: true,
        }]
    }

    /// Cria a janela já no lugar, em pixels lógicos absolutos.
    pub fn create(&mut self, size: u32, pos: (i32, i32)) -> Result<Win, Error> {
        let s = self.scale;
        let px = (size as i32 * s) as u16;
        let id = self.conn.generate_id()?;
        let aux = CreateWindowAux::new()
            .background_pixel(0)
            .border_pixel(0)
            .colormap(self.colormap)
            .override_redirect(1)
            .event_mask(
                EventMask::EXPOSURE
                    | EventMask::BUTTON_PRESS
                    | EventMask::BUTTON_RELEASE
                    | EventMask::POINTER_MOTION
                    | EventMask::ENTER_WINDOW
                    | EventMask::LEAVE_WINDOW
                    | EventMask::VISIBILITY_CHANGE,
            );
        self.conn.create_window(
            self.depth,
            id,
            self.root,
            (pos.0 * s) as i16,
            (pos.1 * s) as i16,
            px,
            px,
            0,
            WindowClass::INPUT_OUTPUT,
            self.visual,
            &aux,
        )?;
        // nome e tipo para o compositor e para quem lista janelas
        let a = &self.atoms;
        self.conn.change_property8(PropMode::REPLACE, id, AtomEnum::WM_NAME, AtomEnum::STRING, b"Foco")?;
        self.conn.change_property8(PropMode::REPLACE, id, a._NET_WM_NAME, a.UTF8_STRING, b"Foco")?;
        self.conn.change_property8(PropMode::REPLACE, id, AtomEnum::WM_CLASS, AtomEnum::STRING, b"foco\0Foco\0")?;
        self.conn.change_property32(PropMode::REPLACE, id, a._NET_WM_PID, AtomEnum::CARDINAL, &[std::process::id()])?;
        self.conn.change_property32(
            PropMode::REPLACE,
            id,
            a._NET_WM_WINDOW_TYPE,
            AtomEnum::ATOM,
            &[a._NET_WM_WINDOW_TYPE_UTILITY],
        )?;
        self.conn.change_property32(
            PropMode::REPLACE,
            id,
            a._NET_WM_STATE,
            AtomEnum::ATOM,
            &[a._NET_WM_STATE_ABOVE, a._NET_WM_STATE_STICKY, a._NET_WM_STATE_SKIP_TASKBAR, a._NET_WM_STATE_SKIP_PAGER],
        )?;
        let gc = self.conn.generate_id()?;
        self.conn.create_gc(gc, id, &CreateGCAux::new())?;
        self.conn.map_window(id)?;
        self.conn.configure_window(id, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
        Ok(Win { id, gc })
    }

    pub fn destroy(&self, win: &Win) {
        let _ = self.conn.free_gc(win.gc);
        let _ = self.conn.destroy_window(win.id);
    }

    pub fn present(&mut self, ov: &mut Overlay, opacity: f32, locked: bool) {
        let (Surf::X(win), Some(pix)) = (&ov.surf, ov.pix.as_ref()) else { return };
        let px = pix.width();
        if ov.region_for != Some((ov.size, locked)) {
            if self.has_shape {
                let disc: Vec<Rectangle> = Geo::new(px as f32)
                    .input_rects(px as i32)
                    .into_iter()
                    .map(|(x, y, w, h)| Rectangle { x: x as i16, y: y as i16, width: w as u16, height: h as u16 })
                    .collect();
                let input = if locked { Vec::new() } else { disc.clone() };
                let _ = self.conn.shape_rectangles(
                    shape::SO::SET,
                    shape::SK::INPUT,
                    ClipOrdering::UNSORTED,
                    win.id,
                    0,
                    0,
                    &input,
                );
                // sem compositor não há transparência: recorta o círculo
                if !self.composited {
                    let _ = self.conn.shape_rectangles(
                        shape::SO::SET,
                        shape::SK::BOUNDING,
                        ClipOrdering::UNSORTED,
                        win.id,
                        0,
                        0,
                        &disc,
                    );
                }
            }
            ov.region_for = Some((ov.size, locked));
        }
        let mut data = vec![0u8; (px * px * 4) as usize];
        render::blit_argb(pix, &mut data, opacity);
        let image = Image::new(
            px as u16,
            px as u16,
            ScanlinePad::Pad32,
            self.depth,
            BitsPerPixel::B32,
            ImageOrder::LsbFirst,
            Cow::Owned(data),
        );
        if let Err(e) = image.map_err(Error::from).and_then(|img| img.put(&self.conn, win.id, win.gc, 0, 0).map(|_| ()).map_err(Error::from)) {
            eprintln!("foco: desenho X11: {e}");
        }
    }

    /// Põe a janela no tamanho e na margem atuais do overlay.
    pub fn place(&mut self, ov: &mut Overlay) {
        let Surf::X(win) = &ov.surf else { return };
        let s = self.scale;
        let origin = self.outputs.iter().find(|o| o.name == ov.output_name).map(|o| o.pos).unwrap_or((0, 0));
        let px = (ov.size as i32 * s) as u32;
        let _ = self.conn.configure_window(
            win.id,
            &ConfigureWindowAux::new()
                .x((origin.0 + ov.margin.0) * s)
                .y((origin.1 + ov.margin.1) * s)
                .width(px)
                .height(px),
        );
        ov.effective = ov.margin;
    }

    pub fn set_cursor(&self, ov: &Overlay, icon: CursorIcon) {
        let Surf::X(win) = &ov.surf else { return };
        let c = match icon {
            CursorIcon::Pointer => self.cursors.pointer,
            CursorIcon::Grabbing => self.cursors.grabbing,
            CursorIcon::NwseResize => self.cursors.resize,
            _ => self.cursors.default,
        };
        let _ = self.conn.change_window_attributes(win.id, &ChangeWindowAttributesAux::new().cursor(c));
    }

    /// A janela ativa está em tela cheia (só enxerga janelas X11).
    pub fn fullscreen_active(&self) -> bool {
        let a = &self.atoms;
        let prop = |win, atom, kind| {
            self.conn
                .get_property(false, win, atom, kind, 0, 64)
                .ok()
                .and_then(|c| c.reply().ok())
                .and_then(|r| r.value32().map(|v| v.collect::<Vec<u32>>()))
                .unwrap_or_default()
        };
        let Some(&active) = prop(self.root, a._NET_ACTIVE_WINDOW, AtomEnum::WINDOW).first() else { return false };
        active != x11rb::NONE && prop(active, a._NET_WM_STATE, AtomEnum::ATOM).contains(&a._NET_WM_STATE_FULLSCREEN)
    }

    /// Volta para o topo (no máximo algumas vezes por segundo, para não brigar com ninguém).
    pub fn raise(&mut self, win: &Win) {
        if self.last_raise.elapsed() < Duration::from_millis(250) {
            return;
        }
        self.last_raise = Instant::now();
        let _ = self.conn.configure_window(win.id, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
    }
}

pub fn insert_source(app: &App, lh: &LoopHandle<'static, App>) -> Result<(), Error> {
    let Disp::X(x) = &app.disp else { return Ok(()) };
    let fd: OwnedFd = x.conn.stream().as_fd().try_clone_to_owned()?;
    lh.insert_source(Generic::new(fd, Interest::READ, Mode::Level), |_, _, app| {
        pump(app);
        Ok(PostAction::Continue)
    })
    .map_err(|e| e.error)?;
    Ok(())
}

/// Trata os eventos que chegaram (inclusive os que o x11rb já leu esperando uma resposta) e
/// envia os pedidos pendentes. Roda depois de cada volta do laço.
pub fn pump(app: &mut App) {
    loop {
        let ev = match &app.disp {
            Disp::X(x) => x.conn.poll_for_event(),
            _ => return,
        };
        match ev {
            Ok(Some(ev)) => handle(app, ev),
            Ok(None) => break,
            Err(e) => {
                config::log(&format!("encerrando: conexão X11 caiu ({e})"));
                app.exit = true;
                return;
            }
        }
    }
    if let Disp::X(x) = &app.disp
        && let Err(e) = x.conn.flush()
    {
        config::log(&format!("encerrando: conexão X11 caiu ({e})"));
        app.exit = true;
    }
}

fn handle(app: &mut App, ev: Event) {
    let Disp::X(x) = &app.disp else { return };
    let scale = x.scale as f64;
    let ours = app.ov.as_ref().and_then(|o| match &o.surf {
        Surf::X(w) => Some(w.id),
        Surf::Wl(_) => None,
    });
    let origin = app
        .ov
        .as_ref()
        .and_then(|o| x.outputs.iter().find(|out| out.name == o.output_name))
        .map(|o| o.pos)
        .unwrap_or((0, 0));
    let pointer = |app: &mut App, ptr: Ptr, ex: i16, ey: i16, rx: i16, ry: i16| {
        if let Some(ov) = app.ov.as_mut() {
            // onde a janela estava quando o evento aconteceu: o arrasto se guia por ela
            ov.effective = (
                ((rx - ex) as f64 / scale).round() as i32 - origin.0,
                ((ry - ey) as f64 / scale).round() as i32 - origin.1,
            );
        }
        app.on_pointer(ptr, ex as f64 / scale, ey as f64 / scale);
    };
    match ev {
        Event::Expose(e) if Some(e.window) == ours && e.count == 0 => app.request_redraw(),
        Event::EnterNotify(e) if Some(e.event) == ours => pointer(app, Ptr::Enter, e.event_x, e.event_y, e.root_x, e.root_y),
        Event::LeaveNotify(e) if Some(e.event) == ours => pointer(app, Ptr::Leave, e.event_x, e.event_y, e.root_x, e.root_y),
        Event::MotionNotify(e) if Some(e.event) == ours => pointer(app, Ptr::Motion, e.event_x, e.event_y, e.root_x, e.root_y),
        Event::ButtonPress(e) if Some(e.event) == ours => {
            let ptr = match e.detail {
                1 => Ptr::Press(BTN_LEFT),
                2 => Ptr::Press(BTN_MIDDLE),
                3 => Ptr::Press(BTN_RIGHT),
                // roda: 4 é para cima (cresce), 5 para baixo
                4 => Ptr::Scroll(-1.0),
                5 => Ptr::Scroll(1.0),
                _ => return,
            };
            pointer(app, ptr, e.event_x, e.event_y, e.root_x, e.root_y);
        }
        Event::ButtonRelease(e) if Some(e.event) == ours => {
            let button = match e.detail {
                1 => BTN_LEFT,
                2 => BTN_MIDDLE,
                3 => BTN_RIGHT,
                _ => return,
            };
            pointer(app, Ptr::Release(button, e.time), e.event_x, e.event_y, e.root_x, e.root_y);
        }
        Event::VisibilityNotify(e) if Some(e.window) == ours && e.state != Visibility::UNOBSCURED => app.keep_on_top(),
        Event::RandrScreenChangeNotify(_) | Event::RandrNotify(_) => {
            if let Disp::X(x) = &mut app.disp {
                let outputs = x.query_outputs();
                if outputs != x.outputs {
                    x.outputs = outputs;
                    app.outputs_changed();
                }
            }
        }
        Event::Error(e) => eprintln!("foco: erro X11: {e:?}"),
        _ => {}
    }
}

/// Clique no overlay. Numa sessão X11 a central também é X11, e o horário do clique deixa o
/// gerenciador de janelas dar o foco para ela.
pub fn open_central(time: u32) {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        let id = format!("foco-{}-overlay_TIME{time}", std::process::id());
        notify::open_settings_x11(&id, &["toggle"]);
    } else {
        notify::open_settings(None, &["toggle"]);
    }
}
