//! Text as outlines, for lettering a part: each glyph's outline read from the bundled font (IBM Plex Mono Regular, under the SIL
//! Open Font License, `assets/fonts/ibm-plex-mono/OFL.txt`) by `ttf-parser`, its curves cut into short straight steps, laid out
//! letter by letter and line by line. `sketch::extrude_contours` lifts them into a solid; a boolean engraves or embosses it.

use ttf_parser::{Face, OutlineBuilder};

use crate::sketch::P2;

/// The font, compiled in.
const FONT: &[u8] = include_bytes!("../../../assets/fonts/ibm-plex-mono/IBMPlexMono-Regular.ttf");
/// The most characters a text may have.
pub const MAX_CHARS: usize = 500;
/// The straight steps a curve of a glyph is cut into.
const STEPS: usize = 8;

/// A glyph's contours, its curves cut into steps, in font units.
#[derive(Default)]
struct Pen {
    contours: Vec<Vec<P2>>,
    at: P2,
}

impl Pen {
    fn current(&mut self) -> &mut Vec<P2> {
        if self.contours.is_empty() {
            self.contours.push(Vec::new());
        }
        let last = self.contours.len() - 1;
        &mut self.contours[last]
    }
}

impl OutlineBuilder for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        self.at = [f64::from(x), f64::from(y)];
        self.contours.push(vec![self.at]);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.at = [f64::from(x), f64::from(y)];
        let at = self.at;
        self.current().push(at);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (p0, c, p) = (self.at, [f64::from(x1), f64::from(y1)], [f64::from(x), f64::from(y)]);
        for k in 1..=STEPS {
            let t = k as f64 / STEPS as f64;
            let u = 1.0 - t;
            let q = [u * u * p0[0] + 2.0 * u * t * c[0] + t * t * p[0], u * u * p0[1] + 2.0 * u * t * c[1] + t * t * p[1]];
            self.current().push(q);
        }
        self.at = p;
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (p0, c1, c2, p) = (self.at, [f64::from(x1), f64::from(y1)], [f64::from(x2), f64::from(y2)], [f64::from(x), f64::from(y)]);
        for k in 1..=STEPS {
            let t = k as f64 / STEPS as f64;
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            let q = [a * p0[0] + b * c1[0] + c * c2[0] + d * p[0], a * p0[1] + b * c1[1] + c * c2[1] + d * p[1]];
            self.current().push(q);
        }
        self.at = p;
    }

    fn close(&mut self) {}
}

/// The outlines of `text`, its capitals `height` mm tall, `spacing` times the font's own space between letters (1 for the font's),
/// lines one under another. Millimetres, the first line's baseline on y = 0, starting at x = 0. A character the font has no glyph
/// for is an error that names it.
pub fn contours(text: &str, height: f64, spacing: f64) -> Result<Vec<Vec<P2>>, String> {
    let chars = text.chars().count();
    if text.trim().is_empty() || chars > MAX_CHARS {
        return Err(format!("`text` is from 1 to {MAX_CHARS} characters, not all spaces"));
    }
    if !(height.is_finite() && height > 0.0 && height <= 1000.0) {
        return Err("`height` (of the capitals) is from 0 to 1000 mm".into());
    }
    if !(spacing.is_finite() && (0.5..=3.0).contains(&spacing)) {
        return Err("`spacing` is from 0.5 to 3 times the font's own".into());
    }
    let face = Face::parse(FONT, 0).map_err(|e| format!("internal: the font does not read ({e})"))?;
    let caps = face.capital_height().filter(|h| *h > 0).map_or(f64::from(face.ascender()) * 0.7, f64::from);
    let k = height / caps;
    let line = f64::from(face.ascender() - face.descender() + face.line_gap()) * k;
    let mut out = Vec::new();
    let missing: Vec<char> = text.chars().filter(|c| !c.is_whitespace() && face.glyph_index(*c).is_none()).collect();
    if !missing.is_empty() {
        let shown: String = missing.iter().take(10).collect();
        return Err(format!("the font has no letters for {shown:?}: it has Latin, Greek and Cyrillic letters, digits and punctuation"));
    }
    for (row, text_line) in text.lines().enumerate() {
        let mut x = 0.0;
        let y = -(row as f64) * line;
        for c in text_line.chars() {
            let Some(id) = face.glyph_index(c) else { continue }; // a space or a tab has none in some fonts
            let mut pen = Pen::default();
            let _ = face.outline_glyph(id, &mut pen);
            for contour in pen.contours {
                let placed: Vec<P2> = contour.iter().map(|p| [x + p[0] * k, y + p[1] * k]).collect();
                if placed.len() >= 3 {
                    out.push(placed);
                }
            }
            x += f64::from(face.glyph_hor_advance(id).unwrap_or(0)) * k * spacing;
        }
    }
    if out.is_empty() {
        return Err("the text has no letters to draw".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sketch::extrude_contours;

    #[test]
    fn letters_have_their_size_and_their_holes() {
        let h = contours("H", 10.0, 1.0).unwrap();
        let (lo, hi) = bounds(&h);
        assert!((hi[1] - lo[1] - 10.0).abs() < 0.2, "a capital is as tall as asked: {lo:?} {hi:?}");
        // An O is a ring: extruded, its volume is its outer area less its hole, and it is closed.
        let o = extrude_contours(&contours("O", 10.0, 1.0).unwrap(), 2.0).unwrap();
        assert!(o.is_watertight());
        let (a, b) = o.bounds().unwrap();
        let full = (b[0] - a[0]) * (b[1] - a[1]) * 2.0;
        assert!(o.volume() > 0.0 && o.volume() < 0.6 * full, "the hole is a hole: {} of {full}", o.volume());
        // Text runs along x, and a second line sits under the first.
        let two = contours("AB\nC", 5.0, 1.0).unwrap();
        let (lo, hi) = bounds(&two);
        assert!(hi[0] - lo[0] > 5.0 && lo[1] < -5.0, "{lo:?} {hi:?}");
        let solid = extrude_contours(&contours("Lamina 3D", 8.0, 1.0).unwrap(), 1.2).unwrap();
        assert!(solid.is_watertight() && solid.volume() > 0.0);
    }

    #[test]
    fn what_the_font_cannot_draw_is_said() {
        assert!(contours("\u{4e2d}\u{6587}", 5.0, 1.0).unwrap_err().contains("no letters"));
        assert!(contours("   ", 5.0, 1.0).is_err());
        assert!(contours("x", f64::NAN, 1.0).is_err());
        assert!(contours("x", 5.0, 9.0).is_err());
        assert!(contours(&"a".repeat(MAX_CHARS + 1), 5.0, 1.0).is_err());
    }

    fn bounds(c: &[Vec<P2>]) -> (P2, P2) {
        c.iter()
            .flatten()
            .fold(([f64::MAX; 2], [f64::MIN; 2]), |(lo, hi), p| ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])]))
    }
}
