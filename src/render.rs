//! Desenho do overlay e do ícone da bandeja (tiny-skia + contornos de fonte via ttf-parser).

use std::f32::consts::{FRAC_1_SQRT_2, PI, TAU};
use std::sync::OnceLock;

use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LinearGradient, Paint, Path, PathBuilder, Pixmap, Point,
    RadialGradient, Shader, SpreadMode, Stroke, Transform,
};
use ttf_parser::{Face, OutlineBuilder};

use crate::timer::{Phase, RunKind};

static SEMIBOLD: &[u8] = include_bytes!("../assets/FiraSans-SemiBold.otf");
static MEDIUM: &[u8] = include_bytes!("../assets/FiraSans-Medium.otf");

fn semibold() -> &'static Face<'static> {
    static F: OnceLock<Face<'static>> = OnceLock::new();
    F.get_or_init(|| Face::parse(SEMIBOLD, 0).expect("fonte embutida"))
}

fn medium() -> &'static Face<'static> {
    static F: OnceLock<Face<'static>> = OnceLock::new();
    F.get_or_init(|| Face::parse(MEDIUM, 0).expect("fonte embutida"))
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Hit {
    #[default]
    None,
    Body,
    Resize,
}

/// Tudo que o overlay precisa para se desenhar.
#[derive(Clone, Debug)]
pub struct View {
    pub phase: Phase,
    pub run: RunKind,
    pub clock: String,
    pub fraction: f32,
    pub done_in_cycle: u32,
    pub every: u32,
    pub color: [u8; 3],
    pub bg_opacity: f32,
    /// 0..1, animado: a alça e o realce aparecem conforme sobe.
    pub hover: f32,
    pub hot: Hit,
    /// 0..1, pulso quando uma etapa termina.
    pub flash: f32,
    pub blocking: bool,
}

/// Geometria do overlay, proporcional ao tamanho (serve tanto em pixels lógicos quanto de buffer).
pub struct Geo {
    pub c: f32,
    pub r: f32,
    pub ring_r: f32,
    pub ring_w: f32,
    pub row_y: f32,
    pub grip: (f32, f32),
    pub grip_r: f32,
    /// Distância (em cada eixo) do canto superior esquerdo até a alça, como fração do tamanho.
    pub grip_frac: f32,
}

impl Geo {
    pub fn new(size: f32) -> Geo {
        let c = size / 2.0;
        let r = size * 0.45;
        let ring_w = size * 0.056;
        let ring_r = r - size * 0.048 - ring_w / 2.0;
        let g = (r + size * 0.004) * FRAC_1_SQRT_2;
        Geo {
            c,
            r,
            ring_r,
            ring_w,
            row_y: c + size * 0.19,
            grip: (c + g, c + g),
            grip_r: size * 0.043,
            grip_frac: (c + g) / size,
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> Hit {
        let d = |px: f32, py: f32| ((x - px).powi(2) + (y - py).powi(2)).sqrt();
        if d(self.grip.0, self.grip.1) <= self.grip_r * 1.6 {
            return Hit::Resize;
        }
        if d(self.c, self.c) > self.r {
            return Hit::None;
        }
        Hit::Body
    }

    /// Retângulos (x, y, w, h) que cobrem o disco e a alça: a região que recebe cliques.
    pub fn input_rects(&self, size: i32) -> Vec<(i32, i32, i32, i32)> {
        let mut out = Vec::new();
        let step = 2;
        let mut y = 0;
        while y < size {
            // ponto da faixa mais próximo do centro: cobre o disco inteiro
            let yc = (self.c).clamp(y as f32, (y + step) as f32);
            let dy = yc - self.c;
            if dy.abs() < self.r {
                let half = (self.r * self.r - dy * dy).sqrt();
                let x0 = (self.c - half).floor() as i32;
                let x1 = (self.c + half).ceil() as i32;
                out.push((x0, y, x1 - x0, step));
            }
            y += step;
        }
        let gr = self.grip_r * 1.6;
        out.push((
            (self.grip.0 - gr).floor() as i32,
            (self.grip.1 - gr).floor() as i32,
            (gr * 2.0).ceil() as i32,
            (gr * 2.0).ceil() as i32,
        ));
        out
    }
}

fn rgba(c: [u8; 3], a: f32) -> Color {
    Color::from_rgba8(c[0], c[1], c[2], (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn mix(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2])]
}

const WHITE: [u8; 3] = [255, 255, 255];
const BLACK: [u8; 3] = [0, 0, 0];

fn pt(x: f32, y: f32) -> Point {
    Point::from_xy(x, y)
}

fn solid(c: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(c);
    p.anti_alias = true;
    p
}

fn shaded(shader: Shader<'static>) -> Paint<'static> {
    Paint { shader, anti_alias: true, ..Paint::default() }
}

fn fill(pix: &mut Pixmap, path: &Path, paint: &Paint) {
    pix.fill_path(path, paint, FillRule::Winding, Transform::identity(), None);
}

fn stroke(pix: &mut Pixmap, path: &Path, paint: &Paint, width: f32, round: bool) {
    let st = Stroke {
        width,
        line_cap: if round { LineCap::Round } else { LineCap::Butt },
        ..Stroke::default()
    };
    pix.stroke_path(path, paint, &st, Transform::identity(), None);
}

fn circle(cx: f32, cy: f32, r: f32) -> Option<Path> {
    PathBuilder::from_circle(cx, cy, r.max(0.01))
}

/// Arco por Béziers cúbicas (≤ 90° cada). Ângulo 0 = direita, sentido horário (y para baixo).
fn push_arc(pb: &mut PathBuilder, cx: f32, cy: f32, r: f32, start: f32, sweep: f32, move_first: bool) {
    let segs = (sweep.abs() / (PI / 2.0)).ceil().max(1.0) as usize;
    let step = sweep / segs as f32;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let mut a = start;
    if move_first {
        pb.move_to(cx + r * a.cos(), cy + r * a.sin());
    }
    for _ in 0..segs {
        let b = a + step;
        let (ca, sa, cb, sb) = (a.cos(), a.sin(), b.cos(), b.sin());
        pb.cubic_to(
            cx + r * (ca - k * sa),
            cy + r * (sa + k * ca),
            cx + r * (cb + k * sb),
            cy + r * (sb - k * cb),
            cx + r * cb,
            cy + r * sb,
        );
        a = b;
    }
}

fn arc(cx: f32, cy: f32, r: f32, start: f32, sweep: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    push_arc(&mut pb, cx, cy, r, start, sweep, true);
    pb.finish()
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    push_arc(&mut pb, x + w - r, y + r, r, -PI / 2.0, PI / 2.0, false);
    pb.line_to(x + w, y + h - r);
    push_arc(&mut pb, x + w - r, y + h - r, r, 0.0, PI / 2.0, false);
    pb.line_to(x + r, y + h);
    push_arc(&mut pb, x + r, y + h - r, r, PI / 2.0, PI / 2.0, false);
    pb.line_to(x, y + r);
    push_arc(&mut pb, x + r, y + r, r, PI, PI / 2.0, false);
    pb.close();
    pb.finish()
}

struct Sink<'a> {
    pb: &'a mut PathBuilder,
    x: f32,
    y: f32,
    k: f32,
}

impl OutlineBuilder for Sink<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.pb.move_to(self.x + x * self.k, self.y - y * self.k);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.pb.line_to(self.x + x * self.k, self.y - y * self.k);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.pb.quad_to(self.x + x1 * self.k, self.y - y1 * self.k, self.x + x * self.k, self.y - y * self.k);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.pb.cubic_to(
            self.x + x1 * self.k,
            self.y - y1 * self.k,
            self.x + x2 * self.k,
            self.y - y2 * self.k,
            self.x + x * self.k,
            self.y - y * self.k,
        );
    }
    fn close(&mut self) {
        self.pb.close();
    }
}

struct Text {
    path: Option<Path>,
    width: f32,
}

/// Texto centrado em `cx` com a linha de base em `baseline`. Com `tabular`, todo dígito
/// ocupa a largura do dígito mais largo (o relógio não "dança").
fn text(face: &Face, s: &str, px: f32, cx: f32, baseline: f32, tracking_em: f32, tabular: bool) -> Text {
    let upem = face.units_per_em() as f32;
    let k = px / upem;
    let digit = ('0'..='9')
        .filter_map(|c| face.glyph_index(c))
        .filter_map(|g| face.glyph_hor_advance(g))
        .max()
        .unwrap_or(0) as f32;
    let glyphs: Vec<_> = s
        .chars()
        .map(|ch| {
            let gid = face.glyph_index(ch);
            let own = gid.and_then(|g| face.glyph_hor_advance(g)).map(f32::from).unwrap_or(upem * 0.3);
            let cell = if tabular && ch.is_ascii_digit() { digit } else { own };
            (gid, own, cell)
        })
        .collect();
    let track = tracking_em * upem;
    let total = glyphs.iter().map(|g| g.2).sum::<f32>() + track * glyphs.len().saturating_sub(1) as f32;
    let mut x = cx - total * k / 2.0;
    let mut pb = PathBuilder::new();
    for (gid, own, cell) in glyphs {
        if let Some(g) = gid {
            let mut sink = Sink { pb: &mut pb, x: x + (cell - own) / 2.0 * k, y: baseline, k };
            face.outline_glyph(g, &mut sink);
        }
        x += (cell + track) * k;
    }
    Text { path: pb.finish(), width: total * k }
}

fn cap_height(face: &Face, px: f32) -> f32 {
    let upem = face.units_per_em() as f32;
    face.capital_height().map(f32::from).unwrap_or(upem * 0.7) * px / upem
}

fn draw_text(pix: &mut Pixmap, t: &Text, color: Color) {
    if let Some(p) = &t.path {
        fill(pix, p, &solid(color));
    }
}

fn lock_icon(pix: &mut Pixmap, cx: f32, cy: f32, h: f32, paint: &Paint) {
    let w = h * 0.8;
    let body_top = cy - h * 0.08;
    if let Some(p) = rounded_rect(cx - w / 2.0, body_top, w, h * 0.58, h * 0.1) {
        fill(pix, &p, paint);
    }
    let sw = w * 0.3;
    let mut pb = PathBuilder::new();
    pb.move_to(cx - sw, body_top + h * 0.02);
    pb.line_to(cx - sw, cy - h * 0.2);
    push_arc(&mut pb, cx, cy - h * 0.2, sw, PI, PI, false);
    pb.line_to(cx + sw, body_top + h * 0.02);
    if let Some(p) = pb.finish() {
        stroke(pix, &p, paint, h * 0.12, true);
    }
}

fn grip_icon(pix: &mut Pixmap, cx: f32, cy: f32, r: f32, paint: &Paint) {
    // seta diagonal dupla ↖↘
    let d = r * 0.42;
    let mut pb = PathBuilder::new();
    pb.move_to(cx - d, cy - d);
    pb.line_to(cx + d, cy + d);
    for (x, y, sx) in [(cx - d, cy - d, 1.0f32), (cx + d, cy + d, -1.0)] {
        pb.move_to(x + sx * d * 0.75, y);
        pb.line_to(x, y);
        pb.line_to(x, y + sx * d * 0.75);
    }
    if let Some(p) = pb.finish() {
        let st = Stroke {
            width: r * 0.16,
            line_cap: LineCap::Round,
            line_join: tiny_skia::LineJoin::Round,
            ..Stroke::default()
        };
        pix.stroke_path(&p, paint, &st, Transform::identity(), None);
    }
}

/// Desenha o overlay inteiro num pixmap quadrado (lado = tamanho em pixels do buffer).
pub fn draw_overlay(pix: &mut Pixmap, v: &View) {
    pix.fill(Color::TRANSPARENT);
    let s = pix.width() as f32;
    let g = Geo::new(s);
    let c = g.c;
    let col = v.color;
    let bg = v.bg_opacity;

    // sombra suave em volta do disco
    let sy = c + s * 0.012;
    if let Some(sh) = RadialGradient::new(
        pt(c, sy),
        0.0,
        pt(c, sy),
        s * 0.5,
        vec![
            GradientStop::new(0.84, rgba(BLACK, 0.34 * bg)),
            GradientStop::new(0.92, rgba(BLACK, 0.12 * bg)),
            GradientStop::new(1.0, rgba(BLACK, 0.0)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    ) && let Some(p) = circle(c, sy, s * 0.5)
    {
        fill(pix, &p, &shaded(sh));
    }

    // disco de fundo, com um leve tom da cor da etapa no alto
    if let Some(disc) = circle(c, c, g.r) {
        let top = mix([34, 36, 46], col, 0.07);
        if let Some(sh) = LinearGradient::new(
            pt(c, c - g.r),
            pt(c, c + g.r),
            vec![GradientStop::new(0.0, rgba(top, bg)), GradientStop::new(1.0, rgba([14, 15, 20], bg))],
            SpreadMode::Pad,
            Transform::identity(),
        ) {
            fill(pix, &disc, &shaded(sh));
        }
        if v.flash > 0.0 {
            fill(pix, &disc, &solid(rgba(col, 0.32 * v.flash)));
        }
    }
    if let Some(p) = circle(c, c, g.r - s * 0.003) {
        stroke(pix, &p, &solid(rgba(WHITE, 0.075)), (s * 0.006).max(1.0), false);
    }

    // trilho
    if let Some(p) = circle(c, c, g.ring_r) {
        stroke(pix, &p, &solid(rgba(WHITE, 0.075)), g.ring_w, false);
    }

    // progresso: a parte que já passou some no sentido horário a partir do topo
    let strength = match v.run {
        RunKind::Running => 1.0,
        RunKind::Paused => 0.55,
        RunKind::Idle => 0.42,
    };
    let frac = if v.run == RunKind::Idle { 1.0 } else { v.fraction };
    if frac > 0.0005 {
        let start = -PI / 2.0 + (1.0 - frac) * TAU;
        let sweep = frac * TAU;
        if let Some(p) = arc(c, c, g.ring_r, start, sweep) {
            if v.run != RunKind::Idle {
                stroke(pix, &p, &solid(rgba(col, 0.14 * strength)), g.ring_w * 1.75, true);
            }
            if let Some(sh) = LinearGradient::new(
                pt(c - g.r, c - g.r),
                pt(c + g.r, c + g.r),
                vec![
                    GradientStop::new(0.0, rgba(mix(col, WHITE, 0.32), strength)),
                    GradientStop::new(0.55, rgba(col, strength)),
                    GradientStop::new(1.0, rgba(mix(col, BLACK, 0.1), strength)),
                ],
                SpreadMode::Pad,
                Transform::identity(),
            ) {
                stroke(pix, &p, &shaded(sh), g.ring_w, true);
            }
        }
        if v.run != RunKind::Idle {
            let (hx, hy) = (c + g.ring_r * start.cos(), c + g.ring_r * start.sin());
            if let Some(p) = circle(hx, hy, g.ring_w * 0.24) {
                fill(pix, &p, &solid(rgba(WHITE, 0.92 * strength.max(0.7))));
            }
        }
    }

    // bolinhas do ciclo
    let n = v.every.clamp(1, 12);
    let spacing = s * 0.043;
    let dot_r = s * 0.0125;
    let dots_y = c - s * 0.178;
    for i in 0..n {
        let x = c + (i as f32 - (n as f32 - 1.0) / 2.0) * spacing;
        let done = v.phase == Phase::LongBreak || i < v.done_in_cycle;
        let current = v.phase == Phase::Focus && i == v.done_in_cycle && v.run != RunKind::Idle;
        let color = if done {
            rgba(col, 0.95)
        } else if current {
            rgba(col, 0.45)
        } else {
            rgba(WHITE, 0.16)
        };
        if let Some(p) = circle(x, dots_y, dot_r) {
            fill(pix, &p, &solid(color));
        }
    }

    // relógio
    let face = semibold();
    let px = s * if v.clock.len() > 5 { 0.19 } else { 0.218 };
    let baseline = c + s * 0.012 + cap_height(face, px) / 2.0;
    let clock = text(face, &v.clock, px, c, baseline, 0.0, true);
    let clock_alpha = if v.run == RunKind::Paused { 0.62 } else { 0.97 };
    draw_text(pix, &clock, rgba(WHITE, clock_alpha));

    // linha de baixo: rótulo da etapa
    let label = match v.run {
        RunKind::Paused => "PAUSADO",
        _ => v.phase.label(),
    };
    let lf = medium();
    let lpx = s * 0.058;
    let lbase = g.row_y + cap_height(lf, lpx) / 2.0;
    let lc = mix(col, WHITE, 0.18);
    let a = if v.run == RunKind::Paused { 0.7 } else { 0.95 };
    let t = text(lf, label, lpx, c, lbase, 0.16, false);
    draw_text(pix, &t, rgba(lc, a));
    if v.blocking {
        let lh = lpx * 1.05;
        lock_icon(pix, c - t.width / 2.0 - lh * 0.9, g.row_y, lh, &solid(rgba(lc, a * 0.85)));
    }

    // com o mouse em cima: borda acesa (clique abre a central) e alça de redimensionar
    let ca = v.hover;
    if ca > 0.01 {
        if let Some(p) = circle(c, c, g.r - s * 0.004) {
            stroke(pix, &p, &solid(rgba(mix(col, WHITE, 0.25), ca * 0.32)), (s * 0.008).max(1.0), false);
        }
        let (gx, gy) = g.grip;
        let hot = v.hot == Hit::Resize;
        if let Some(p) = circle(gx, gy, g.grip_r) {
            fill(pix, &p, &solid(rgba(if hot { [52, 55, 68] } else { [30, 32, 41] }, ca * 0.96)));
            stroke(pix, &p, &solid(rgba(WHITE, ca * 0.14)), (s * 0.005).max(1.0), false);
        }
        grip_icon(pix, gx, gy, g.grip_r, &solid(rgba(WHITE, ca * if hot { 0.95 } else { 0.7 })));
    }
}

/// Ícone da bandeja em ARGB32 (ordem de rede, sem pré-multiplicar), como o SNI espera.
pub fn tray_icon(px: u32, color: [u8; 3], frac: f32, run: RunKind, label: &str) -> Vec<u8> {
    let Some(mut pix) = Pixmap::new(px, px) else { return Vec::new() };
    let s = px as f32;
    let c = s / 2.0;
    let ring_w = s * 0.12;
    let ring_r = c - ring_w / 2.0 - s * 0.03;

    if let Some(p) = circle(c, c, c - s * 0.02) {
        fill(&mut pix, &p, &solid(rgba([20, 21, 27], 0.78)));
    }
    if let Some(p) = circle(c, c, ring_r) {
        stroke(&mut pix, &p, &solid(rgba(WHITE, 0.2)), ring_w, false);
    }
    let strength = match run {
        RunKind::Running => 1.0,
        RunKind::Paused => 0.55,
        RunKind::Idle => 0.6,
    };
    let frac = if run == RunKind::Idle { 1.0 } else { frac };
    if frac > 0.001 {
        let start = -PI / 2.0 + (1.0 - frac) * TAU;
        if let Some(p) = arc(c, c, ring_r, start, frac * TAU) {
            stroke(&mut pix, &p, &solid(rgba(mix(color, WHITE, 0.1), strength)), ring_w, true);
        }
    }
    let face = semibold();
    let tpx = s * if label.chars().count() > 2 { 0.32 } else { 0.4 };
    let baseline = c + cap_height(face, tpx) / 2.0;
    let t = text(face, label, tpx, c, baseline, 0.0, true);
    draw_text(&mut pix, &t, rgba(WHITE, if run == RunKind::Paused { 0.7 } else { 1.0 }));

    let mut out = Vec::with_capacity((px * px * 4) as usize);
    for p in pix.pixels() {
        let c = p.demultiply();
        out.extend_from_slice(&[c.alpha(), c.red(), c.green(), c.blue()]);
    }
    out
}

/// Copia o pixmap (RGBA pré-multiplicado) para um buffer wl_shm ARGB8888 (BGRA na memória),
/// aplicando a opacidade global.
pub fn blit_argb(pix: &Pixmap, dst: &mut [u8], opacity: f32) {
    let o = (opacity.clamp(0.0, 1.0) * 256.0) as u32;
    for (s, d) in pix.data().chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
        let m = |x: u8| ((x as u32 * o) >> 8) as u8;
        d[0] = m(s[2]);
        d[1] = m(s[1]);
        d[2] = m(s[0]);
        d[3] = m(s[3]);
    }
}
