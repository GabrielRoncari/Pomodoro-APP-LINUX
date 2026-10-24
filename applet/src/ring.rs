//! Anel de progresso e bolinhas do ciclo, gerados como SVG (nítidos em qualquer escala).
//! Mesma leitura do overlay: o arco é o tempo que falta e some no sentido horário a partir do topo.

use std::f32::consts::TAU;
use std::fmt::Write;

pub struct Ring<'a> {
    /// Fração que falta da etapa (1 = cheia).
    pub left: f32,
    pub color: &'a str,
    pub track: &'a str,
    /// Espessura do traço, com o anel ocupando uma caixa de 100.
    pub width: f32,
    /// Duas barras no meio (pausado). Só no ícone do painel, o popup tem o relógio ali.
    pub pause_bars: bool,
    /// Etapa parada: o arco fica apagado.
    pub dim: bool,
}

impl Ring<'_> {
    pub fn svg(&self) -> Vec<u8> {
        let r = 50.0 - self.width / 2.0 - 1.0;
        let w = self.width;
        let mut s = String::with_capacity(600);
        let _ = write!(
            s,
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><circle cx="50" cy="50" r="{r:.2}" fill="none" stroke="{}" stroke-opacity="0.22" stroke-width="{w}"/>"#,
            self.track
        );
        let left = self.left.clamp(0.0, 1.0);
        let opacity = if self.dim { 0.55 } else { 1.0 };
        if left >= 0.999 {
            let _ = write!(
                s,
                r#"<circle cx="50" cy="50" r="{r:.2}" fill="none" stroke="{}" stroke-opacity="{opacity}" stroke-width="{w}"/>"#,
                self.color
            );
        } else if left > 0.002 {
            let a = (1.0 - left) * TAU;
            let (x, y) = (50.0 + r * a.sin(), 50.0 - r * a.cos());
            let large = u8::from(left > 0.5);
            let _ = write!(
                s,
                r#"<path d="M{x:.2} {y:.2}A{r:.2} {r:.2} 0 {large} 1 50 {top:.2}" fill="none" stroke="{}" stroke-opacity="{opacity}" stroke-width="{w}" stroke-linecap="round"/>"#,
                self.color,
                top = 50.0 - r
            );
        }
        if self.pause_bars {
            let _ = write!(
                s,
                r#"<rect x="35" y="33" width="10" height="34" rx="3" fill="{c}"/><rect x="55" y="33" width="10" height="34" rx="3" fill="{c}"/>"#,
                c = self.track
            );
        }
        s.push_str("</svg>");
        s.into_bytes()
    }
}

/// Bolinhas do ciclo até a pausa longa: as feitas cheias, as que faltam só no contorno.
pub fn dots(done: u32, every: u32, color: &str, track: &str) -> (Vec<u8>, f32) {
    let n = every.clamp(1, 12);
    let step = 14.0;
    let width = step * n as f32;
    let mut s = String::with_capacity(120 + 90 * n as usize);
    let _ = write!(s, r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} 14">"#);
    for i in 0..n {
        let cx = step * i as f32 + step / 2.0;
        if i < done {
            let _ = write!(s, r#"<circle cx="{cx}" cy="7" r="4.5" fill="{color}"/>"#);
        } else {
            let _ = write!(
                s,
                r#"<circle cx="{cx}" cy="7" r="4" fill="none" stroke="{track}" stroke-opacity="0.45" stroke-width="1.5"/>"#
            );
        }
    }
    s.push_str("</svg>");
    (s.into_bytes(), width)
}
