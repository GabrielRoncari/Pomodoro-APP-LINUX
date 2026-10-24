mod app;
mod blocker;
mod config;
mod ipc;
mod notify;
mod render;
mod stats;
mod timer;
mod tray;

use std::os::unix::net::{UnixListener, UnixStream};

use render::{Hit, View};
use timer::{Phase, RunKind};

fn preview(out: &str, size: u32) {
    let base = View {
        phase: Phase::Focus,
        run: RunKind::Running,
        clock: "17:42".into(),
        fraction: 0.71,
        done_in_cycle: 1,
        every: 4,
        color: config::parse_color("#ff6b5e"),
        bg_opacity: 0.86,
        hover: 0.0,
        hot: Hit::None,
        pressed: Hit::None,
        flash: 0.0,
        strict: false,
        blocking: true,
    };
    let variants = vec![
        View { run: RunKind::Idle, clock: "25:00".into(), fraction: 1.0, done_in_cycle: 0, blocking: false, ..base.clone() },
        base.clone(),
        View { hover: 1.0, hot: Hit::Toggle, ..base.clone() },
        View { run: RunKind::Paused, hover: 1.0, hot: Hit::Resize, ..base.clone() },
        View { phase: Phase::ShortBreak, clock: "03:12".into(), fraction: 0.64, color: config::parse_color("#3fd8a0"), blocking: false, done_in_cycle: 2, ..base.clone() },
        View { phase: Phase::LongBreak, clock: "11:05".into(), fraction: 0.2, color: config::parse_color("#62a8ff"), blocking: false, done_in_cycle: 4, ..base.clone() },
        View { strict: true, hover: 1.0, fraction: 0.05, clock: "01:15".into(), ..base.clone() },
        View { flash: 0.8, ..base.clone() },
    ];
    let cols = 4u32;
    let rows = (variants.len() as u32).div_ceil(cols);
    let gap = 16u32;
    let mut sheet = tiny_skia::Pixmap::new(cols * (size + gap) + gap, rows * (size + gap) + gap + 80).unwrap();
    sheet.fill(tiny_skia::Color::from_rgba8(110, 118, 135, 255));
    for (i, v) in variants.iter().enumerate() {
        let mut p = tiny_skia::Pixmap::new(size, size).unwrap();
        render::draw_overlay(&mut p, v);
        let x = gap + (i as u32 % cols) * (size + gap);
        let y = gap + (i as u32 / cols) * (size + gap);
        sheet.draw_pixmap(x as i32, y as i32, p.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
    }
    // ícones da bandeja
    let mut x = gap as i32;
    for (px, run, frac, label) in [(22, RunKind::Running, 0.7, "18"), (32, RunKind::Running, 0.7, "18"), (48, RunKind::Paused, 0.4, "10"), (64, RunKind::Idle, 1.0, "25"), (64, RunKind::Running, 0.9, "120")] {
        let argb = render::tray_icon(px, config::parse_color("#ff6b5e"), frac, run, label);
        let mut p = tiny_skia::Pixmap::new(px, px).unwrap();
        for (d, s) in p.pixels_mut().iter_mut().zip(argb.chunks_exact(4)) {
            *d = tiny_skia::ColorU8::from_rgba(s[1], s[2], s[3], s[0]).premultiply();
        }
        sheet.draw_pixmap(x, (sheet.height() - 72) as i32, p.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
        x += px as i32 + 16;
    }
    sheet.save_png(out).unwrap();
    println!("{out}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => std::process::exit(daemon()),
        Some("-h" | "--help" | "help") => println!("{}", ipc::HELP),
        Some("-V" | "--version") => println!("foco {}", env!("CARGO_PKG_VERSION")),
        // abre direto, mesmo com o daemon parado
        Some("settings") => std::process::exit(if notify::open_settings() { 0 } else { 1 }),
        Some("preview") => {
            let out = args.get(1).cloned().unwrap_or_else(|| "preview.png".into());
            let size = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(220);
            preview(&out, size);
        }
        Some(_) => std::process::exit(ipc::client(&args)),
    }
}

fn daemon() -> i32 {
    let path = config::socket_path();
    if UnixStream::connect(&path).is_ok() {
        // já está rodando: só traz o overlay de volta
        let _ = ipc::request("show");
        eprintln!("o Foco já está rodando");
        return 0;
    }
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("foco: não consegui abrir {}: {e}", path.display());
            return 1;
        }
    };
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        config::log(&format!("pânico: {info}"));
        default_hook(info);
    }));
    config::log(concat!("iniciado (v", env!("CARGO_PKG_VERSION"), ")"));
    match app::run(listener) {
        Ok(()) => {
            config::log("encerrado");
            0
        }
        Err(e) => {
            config::log(&format!("encerrado por erro: {e}"));
            1
        }
    }
}
