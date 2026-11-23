//! The slice, read back from its G-code as layers of moves, and the view of one layer: extrusions bright, travels faint. Reading the
//! G-code (not the slicer's own structures) shows exactly what the printer will be told.

use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, pos2};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Move {
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub extrude: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layer {
    pub z: f32,
    pub moves: Vec<Move>,
}

/// The layers of `gcode`, each started by the `;LAYER:` comment the slicer writes. Moves before the first layer (the start G-code) and
/// lines that are not `G0`/`G1` are skipped.
pub fn parse(gcode: &str) -> Vec<Layer> {
    let mut layers: Vec<Layer> = Vec::new();
    let (mut x, mut y, mut z) = (0f32, 0f32, 0f32);
    for line in gcode.lines() {
        if line.starts_with(";LAYER:") {
            layers.push(Layer { z, moves: Vec::new() });
            continue;
        }
        let mut words = line.split(';').next().unwrap_or("").split_whitespace();
        let Some(cmd) = words.next() else { continue };
        if cmd != "G0" && cmd != "G1" {
            continue;
        }
        let (mut nx, mut ny, mut nz, mut e) = (x, y, z, 0f32);
        for w in words {
            let Some((k, v)) = w.split_at_checked(1) else { continue };
            let Ok(v) = v.parse::<f32>() else { continue };
            match k {
                "X" => nx = v,
                "Y" => ny = v,
                "Z" => nz = v,
                "E" => e = v,
                _ => {}
            }
        }
        if let Some(layer) = layers.last_mut() {
            // The layer's height is where the head goes before it prints anything on it; the end G-code's lift is not a layer.
            if nz != z && layer.moves.is_empty() {
                layer.z = nz;
            }
            if [nx, ny] != [x, y] {
                layer.moves.push(Move { from: [x, y], to: [nx, ny], extrude: cmd == "G1" && e > 0.0 });
            }
        }
        (x, y, z) = (nx, ny, nz);
    }
    layers
}

const EXTRUDE: Color32 = Color32::from_rgb(245, 165, 60);
const BELOW: Color32 = Color32::from_rgb(90, 80, 70);
const TRAVEL: Color32 = Color32::from_rgb(110, 125, 150);

/// Draws `layers[at]` (and the one below it, dim) on the plate, zoomed to what the slice prints across all its layers. `origin` is where the machine's zero is
/// on the plate: the front left corner, or the middle.
pub fn show(ui: &mut Ui, layers: &[Layer], at: usize, bed: [f64; 2], centred: bool, travels: bool) {
    let rect = ui.available_rect_before_wrap();
    ui.allocate_rect(rect, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(38, 41, 46));
    let (bw, bh) = (bed[0] as f32, bed[1] as f32);
    let (ox, oy) = if centred { (bw / 2.0, bh / 2.0) } else { (0.0, 0.0) };
    // Fit what is printed, not the whole plate, so a small part is not a speck; the plate fills the rest.
    let (lo, hi) = layers
        .iter()
        .flat_map(|l| &l.moves)
        .filter(|m| m.extrude)
        .flat_map(|m| [m.from, m.to])
        .fold(([f32::MAX, f32::MAX], [f32::MIN, f32::MIN]), |(lo, hi), p| {
            ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])])
        });
    let (lo, hi) = if lo[0] <= hi[0] { (lo, hi) } else { ([-ox, -oy], [bw - ox, bh - oy]) };
    let span = [(hi[0] - lo[0]).max(20.0) * 1.15, (hi[1] - lo[1]).max(20.0) * 1.15];
    let k = ((rect.width() - 24.0) / span[0]).min((rect.height() - 24.0) / span[1]).max(0.01);
    let (mx, my, centre) = ((lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, rect.center());
    let to = |p: [f32; 2]| -> Pos2 { pos2(centre.x + (p[0] - mx) * k, centre.y - (p[1] - my) * k) };
    let plate = Rect::from_two_pos(to([-ox, -oy]), to([bw - ox, bh - oy]));
    painter.rect_filled(plate, 2.0, Color32::from_rgb(62, 66, 72));
    if let Some(below) = at.checked_sub(1).and_then(|i| layers.get(i)) {
        for m in below.moves.iter().filter(|m| m.extrude) {
            painter.line_segment([to(m.from), to(m.to)], Stroke::new(1.0, BELOW));
        }
    }
    let Some(layer) = layers.get(at) else { return };
    let width = (0.45 * k).clamp(1.0, 4.0);
    for m in &layer.moves {
        if m.extrude {
            painter.line_segment([to(m.from), to(m.to)], Stroke::new(width, EXTRUDE));
        } else if travels {
            painter.line_segment([to(m.from), to(m.to)], Stroke::new(0.5, TRAVEL));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_are_read_into_their_layers() {
        let g = "G28\nG1 X5 Y5\n;LAYER:0\nG1 Z0.2 F1200\nG0 X10 Y10 F9000\nG1 X20 Y10 E0.5 F1200 ; a comment\nG1 E-0.8\n\
                 ;LAYER:1\nG1 Z0.4\nG1 X20 Y20 E0.4\nG0 X0 Y0\nG91\nG1 Z10\n";
        let layers = parse(g);
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[0].z, 0.2);
        assert_eq!(
            layers[0].moves,
            vec![Move { from: [5.0, 5.0], to: [10.0, 10.0], extrude: false }, Move { from: [10.0, 10.0], to: [20.0, 10.0], extrude: true },]
        );
        assert_eq!(layers[1].z, 0.4, "the end G-code's lift is not the layer's height");
        assert_eq!(layers[1].moves.iter().filter(|m| m.extrude).count(), 1);
    }

    #[test]
    fn junk_is_skipped_not_trusted() {
        assert!(parse("").is_empty());
        let layers = parse(";LAYER:0\nG1 Xé Y\u{2603} E1\nG1 X1e40 Y2\nM104 S200\n");
        assert_eq!(layers.len(), 1);
    }
}
