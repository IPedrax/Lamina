//! The slice, read back from its G-code as layers of moves, each extrusion with its kind from the `;TYPE:` comments; and two views
//! of it: every layer up to the chosen one in 3D, coloured by kind (drawn by `lamina_core::render::draw_paths`), and the chosen layer
//! alone from above, extrusions bright, travels faint. Reading the G-code (not the slicer's own structures) shows exactly what the
//! printer will be told.

use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, TextureHandle, TextureOptions, Ui, pos2};
use lamina_core::render::{Camera, Path, draw_paths};
use lamina_slicer::print::Feature;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Move {
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub extrude: bool,
    /// What it prints: an index into `Feature::ALL` plus one, 0 when the G-code did not say.
    pub kind: u8,
}

/// The kind a `;TYPE:` comment names, as `Move::kind` keeps it.
fn kind_of(name: &str) -> u8 {
    Feature::ALL.iter().position(|f| f.name() == name.trim()).map_or(0, |i| i as u8 + 1)
}

/// The colour each kind is drawn in, and its name for the legend.
pub fn colour(kind: u8) -> (Color32, &'static str) {
    let feature = kind.checked_sub(1).and_then(|i| Feature::ALL.get(usize::from(i))).copied();
    match feature {
        Some(Feature::OuterWall) => (Color32::from_rgb(245, 150, 50), "outer wall"),
        Some(Feature::InnerWall) => (Color32::from_rgb(235, 215, 95), "inner wall"),
        Some(Feature::SolidInfill) => (Color32::from_rgb(190, 90, 210), "solid infill"),
        Some(Feature::SparseInfill) => (Color32::from_rgb(200, 70, 70), "sparse infill"),
        Some(Feature::Skirt) => (Color32::from_rgb(110, 200, 200), "skirt"),
        Some(Feature::Brim) => (Color32::from_rgb(80, 165, 210), "brim"),
        Some(Feature::Support) => (Color32::from_rgb(120, 195, 110), "support"),
        Some(Feature::SupportInterface) => (Color32::from_rgb(60, 140, 75), "support interface"),
        Some(Feature::Raft) => (Color32::from_rgb(160, 160, 160), "raft"),
        Some(Feature::Bridge) => (Color32::from_rgb(70, 125, 235), "bridge"),
        Some(Feature::GapFill) => (Color32::from_rgb(240, 240, 240), "gap fill"),
        Some(Feature::Ironing) => (Color32::from_rgb(150, 230, 120), "ironing"),
        Some(Feature::Tower) => (Color32::from_rgb(175, 140, 110), "purge tower"),
        Some(Feature::Purge) => (Color32::from_rgb(220, 120, 170), "purge"),
        None => (EXTRUDE, "extrusion"),
    }
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
    let mut kind = 0;
    for line in gcode.lines() {
        if line.starts_with(";LAYER:") {
            layers.push(Layer { z, moves: Vec::new() });
            continue;
        }
        if let Some(name) = line.strip_prefix(";TYPE:") {
            kind = kind_of(name);
            continue;
        }
        let mut words = line.split(';').next().unwrap_or("").split_whitespace();
        let Some(cmd) = words.next() else { continue };
        let arc = match cmd {
            "G0" | "G1" => None,
            "G2" => Some(false),
            "G3" => Some(true),
            _ => continue,
        };
        let (mut nx, mut ny, mut nz, mut e, mut ci, mut cj) = (x, y, z, 0f32, 0f32, 0f32);
        for w in words {
            let Some((k, v)) = w.split_at_checked(1) else { continue };
            let Ok(v) = v.parse::<f32>() else { continue };
            match k {
                "X" => nx = v,
                "Y" => ny = v,
                "Z" => nz = v,
                "E" => e = v,
                "I" => ci = v,
                "J" => cj = v,
                _ => {}
            }
        }
        // An arc, drawn as short straight pieces round its centre (I and J from where it starts).
        if let (Some(ccw), Some(layer)) = (arc, layers.last_mut()) {
            let c = [x + ci, y + cj];
            let r = (ci * ci + cj * cj).sqrt();
            let (a0, a1) = ((y - c[1]).atan2(x - c[0]), (ny - c[1]).atan2(nx - c[0]));
            let mut sweep = a1 - a0;
            if ccw && sweep <= 0.0 {
                sweep += std::f32::consts::TAU;
            } else if !ccw && sweep >= 0.0 {
                sweep -= std::f32::consts::TAU;
            }
            if r.is_finite() && r > 0.0 && r < 10_000.0 {
                let steps = ((sweep.abs() / 0.15).ceil() as usize).clamp(1, 256);
                let mut from = [x, y];
                for k in 1..=steps {
                    let t = a0 + sweep * k as f32 / steps as f32;
                    let to = if k == steps { [nx, ny] } else { [c[0] + r * t.cos(), c[1] + r * t.sin()] };
                    layer.moves.push(Move { from, to, extrude: e > 0.0, kind });
                    from = to;
                }
            }
            (x, y, z) = (nx, ny, nz);
            continue;
        }
        if let Some(layer) = layers.last_mut() {
            // The layer's height is where the head goes before it prints anything on it; the end G-code's lift is not a layer.
            if nz != z && layer.moves.is_empty() {
                layer.z = nz;
            }
            if [nx, ny] != [x, y] {
                layer.moves.push(Move { from: [x, y], to: [nx, ny], extrude: cmd == "G1" && e > 0.0, kind });
            }
        }
        (x, y, z) = (nx, ny, nz);
    }
    layers
}

const EXTRUDE: Color32 = Color32::from_rgb(245, 165, 60);
const BELOW: Color32 = Color32::from_rgb(90, 80, 70);
const TRAVEL: Color32 = Color32::from_rgb(110, 125, 150);

/// The 3D preview's camera and its last picture, drawn again only when what it shows changes.
#[derive(Default)]
pub struct Preview3d {
    pub camera: Camera,
    texture: Option<TextureHandle>,
    drawn: Option<(Camera, usize, [usize; 2], usize, bool)>,
    /// The kinds the last picture showed, for the legend.
    pub kinds: Vec<u8>,
    /// Which slice it last framed, so a new slice is framed once.
    framed: usize,
}

/// Where the G-code's lines stand: the plate's size, whether the machine's zero is its middle (else its front left corner), and how
/// wide a line prints, mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Machine {
    pub bed: [f64; 2],
    pub centred: bool,
    pub width: f64,
}

/// The extrusions of `layers` up to `at` (and that layer's travels, with `travels`) as lines on the plate in the scene's frame: the
/// G-code is in the machine's, whose zero is the plate's front left corner unless `centred`. Lower layers are drawn a little darker,
/// so the chosen one stands out.
pub fn paths(layers: &[Layer], at: usize, m: Machine, travels: bool) -> Vec<Path> {
    let Machine { bed, centred, width } = m;
    let (ox, oy) = if centred { (0.0, 0.0) } else { (bed[0] / 2.0, bed[1] / 2.0) };
    let mut out = Vec::new();
    for (i, layer) in layers.iter().enumerate().take(at.saturating_add(1)) {
        let z = f64::from(layer.z);
        let dim = if i == at { 1.0 } else { 0.72 };
        let at3 = |p: [f32; 2]| [f64::from(p[0]) - ox, f64::from(p[1]) - oy, z];
        for m in &layer.moves {
            if m.extrude {
                let c = colour(m.kind).0;
                let color = [c.r(), c.g(), c.b()].map(|v| (f64::from(v) * dim) as u8);
                out.push(Path { from: at3(m.from), to: at3(m.to), color: [color[0], color[1], color[2], 255], width });
            } else if travels && i == at {
                let c = TRAVEL;
                out.push(Path { from: at3(m.from), to: at3(m.to), color: [c.r(), c.g(), c.b(), 255], width: width / 4.0 });
            }
        }
    }
    out
}

/// Draws the slice in 3D up to `layers[at]` in the space left in `ui`, steered as the 3D view is, with a legend of the kinds shown.
pub fn show_3d(ui: &mut Ui, state: &mut Preview3d, layers: &[Layer], at: usize, m: Machine, travels: bool) {
    let slice = layers.as_ptr() as usize;
    let rect = ui.available_rect_before_wrap();
    let legend_height = 22.0;
    let view = Rect::from_min_max(rect.min, pos2(rect.max.x, (rect.max.y - legend_height).max(rect.min.y)));
    let response = ui.allocate_rect(view, Sense::click_and_drag());
    if state.framed != slice {
        // A new slice: look at what it prints.
        let mut lo = [f64::MAX; 3];
        let mut hi = [f64::MIN; 3];
        for p in paths(layers, usize::MAX, m, false).iter().flat_map(|p| [p.from, p.to]) {
            for k in 0..3 {
                (lo[k], hi[k]) = (lo[k].min(p[k]), hi[k].max(p[k]));
            }
        }
        if lo[0] <= hi[0] {
            state.camera.frame_all((lo, hi));
        }
        state.framed = slice;
    }
    crate::view3d::steer(&mut state.camera, &response, ui, true);
    let ppp = ui.ctx().pixels_per_point();
    let size = [(view.width() * ppp).round().max(1.0) as usize, (view.height() * ppp).round().max(1.0) as usize];
    let key = (state.camera, at, size, slice, travels);
    if state.drawn != Some(key) {
        let lines = paths(layers, at, m, travels);
        let mut kinds: Vec<u8> =
            layers.iter().take(at.saturating_add(1)).flat_map(|l| &l.moves).filter(|m| m.extrude).map(|m| m.kind).collect();
        kinds.sort_unstable();
        kinds.dedup();
        state.kinds = kinds;
        let picture = draw_paths(&lines, &state.camera, m.bed, size);
        let image = crate::view3d::image(&picture);
        match &mut state.texture {
            Some(t) => t.set(image, TextureOptions::LINEAR),
            None => state.texture = Some(ui.ctx().load_texture("lamina-preview-3d", image, TextureOptions::LINEAR)),
        }
        state.drawn = Some(key);
    }
    if let Some(t) = &state.texture {
        ui.painter_at(view).image(t.id(), view, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    ui.horizontal_wrapped(|ui| {
        for &k in &state.kinds {
            let (c, name) = colour(k);
            ui.label(RichText::new("\u{25a0}").color(c));
            ui.label(name);
        }
    });
}

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
            vec![
                Move { from: [5.0, 5.0], to: [10.0, 10.0], extrude: false, kind: 0 },
                Move { from: [10.0, 10.0], to: [20.0, 10.0], extrude: true, kind: 0 },
            ]
        );
        assert_eq!(layers[1].z, 0.4, "the end G-code's lift is not the layer's height");
        assert_eq!(layers[1].moves.iter().filter(|m| m.extrude).count(), 1);
    }

    #[test]
    fn each_extrusion_keeps_its_kind_and_the_3d_lines_stand_on_the_plate() {
        let g = ";LAYER:0\nG1 Z0.2\n;TYPE:outer-wall\nG0 X10 Y10\nG1 X20 Y10 E0.5\n;TYPE:sparse-infill\nG1 X20 Y20 E0.5\n\
                 ;TYPE:nonsense\nG1 X10 Y20 E0.5\n;LAYER:1\nG1 Z0.4\n;TYPE:bridge\nG1 X10 Y10 E0.5\n";
        let layers = parse(g);
        let kinds: Vec<&str> = layers.iter().flat_map(|l| &l.moves).filter(|m| m.extrude).map(|m| colour(m.kind).1).collect();
        assert_eq!(kinds, ["outer wall", "sparse infill", "extrusion", "bridge"]);
        // The machine's zero at the front left of a 220 mm plate: the scene's is in the middle, 110 mm in from it.
        let lines = paths(&layers, 0, Machine { bed: [220.0, 220.0], centred: false, width: 0.45 }, false);
        assert_eq!(lines.len(), 3, "the layer above is not drawn");
        let z = f64::from(0.2f32);
        assert_eq!((lines[0].from, lines[0].to), ([-100.0, -100.0, z], [-90.0, -100.0, z]));
        assert_eq!(lines[0].color, [245, 150, 50, 255], "the chosen layer at full colour");
        let both = paths(&layers, 1, Machine { bed: [220.0, 220.0], centred: true, width: 0.45 }, true);
        assert_eq!(both.len(), 4);
        assert!(both[0].color[0] < 245, "a lower layer is darker");
        assert_eq!(both[3].from[2], f64::from(0.4f32));
    }

    #[test]
    fn an_arc_is_drawn_round_its_centre() {
        // A half circle of radius 10 about (10, 0), counter-clockwise from (0, 0) to (20, 0): through (10, -10).
        let layers = parse(";LAYER:0\nG1 Z0.2\nG0 X0 Y0\nG3 X20 Y0 I10 J0 E1\n");
        let arc: Vec<&Move> = layers[0].moves.iter().filter(|m| m.extrude).collect();
        assert!(arc.len() > 10);
        assert!(arc.iter().all(|m| ((m.to[0] - 10.0).powi(2) + m.to[1].powi(2)).sqrt() - 10.0 < 0.01));
        assert!(arc.iter().any(|m| m.to[1] < -9.9), "below, as counter-clockwise from the left goes");
        assert_eq!(arc.last().unwrap().to, [20.0, 0.0]);
        let cw = parse(";LAYER:0\nG1 Z0.2\nG0 X0 Y0\nG2 X20 Y0 I10 J0 E1\n");
        assert!(cw[0].moves.iter().any(|m| m.to[1] > 9.9), "clockwise goes over the top");
    }

    #[test]
    fn junk_is_skipped_not_trusted() {
        assert!(parse("").is_empty());
        let layers = parse(";LAYER:0\nG1 Xé Y\u{2603} E1\nG1 X1e40 Y2\nM104 S200\n");
        assert_eq!(layers.len(), 1);
    }

    #[test]
    fn hostile_gcode_never_panics() {
        use lamina_core::fuzz::Rng;
        let good =
            b";LAYER:0\nG1 Z0.200 F1200\nG0 X10.000 Y10.000 F9000\nG1 X20.000 Y10.000 E0.5 F1200\n;LAYER:1\nG1 Z0.400\nG1 X10 Y20 E1\n";
        let mut r = Rng::new(0x6c0);
        for _ in 0..3000 {
            let _ = parse(&String::from_utf8_lossy(&r.input(&[good])));
        }
    }
}

/// The pictures in docs/USER-GUIDE.md, drawn by the same code as the window's 3D view and preview:
/// `cargo test -p lamina --lib -- --ignored docs_pictures` writes them again.
#[cfg(test)]
mod docs {
    use lamina_core::render::{Camera, draw, draw_paths, png};
    use lamina_core::{Mesh, Session, Transform};
    use serde_json::json;

    #[test]
    #[ignore]
    fn docs_pictures() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/images");
        std::fs::create_dir_all(&dir).unwrap();
        // The plate: a few parts, one picked.
        let mut s = Session::new();
        for cmd in [
            json!(["add_primitive", {"kind": "rounded_box", "size": [40, 30, 12], "corner": 4, "at": [-35, 0]}]),
            json!(["add_primitive", {"kind": "cylinder", "radius": 12, "height": 25, "at": [10, 15]}]),
            json!(["add_primitive", {"kind": "torus", "radius": 12, "tube": 4, "at": [40, -20]}]),
        ] {
            s.execute(cmd[0].as_str().unwrap(), &cmd[1]).unwrap();
        }
        let mut camera = Camera { azimuth: -60.0, elevation: 32.0, ..Camera::default() };
        camera.frame_all(([-60.0, -40.0, 0.0], [60.0, 40.0, 25.0]));
        std::fs::write(dir.join("plate.png"), png(&draw(&s.scene, &[2], &camera, [220.0, 220.0], [960, 600]))).unwrap();
        // The preview: a part with an overhang on supports, with a brim, three quarters printed.
        let p = lamina_slicer::profile::Profile {
            support: lamina_slicer::profile::Support::FromPlate,
            brim_width: 4.0,
            ..lamina_slicer::profile::Profile::default()
        };
        let post = Mesh::cuboid([10.0, 10.0, 20.0]).transformed(&Transform::translate([-12.0, 0.0, 0.0]));
        let arm = Mesh::cuboid([34.0, 10.0, 5.0]).transformed(&Transform::translate([0.0, 0.0, 15.0]));
        let out = lamina_slicer::print::gcode(&[(1, post), (2, arm)], &p).unwrap();
        let layers = super::parse(&out.gcode);
        let machine = super::Machine { bed: p.bed, centred: p.origin_center, width: p.line_width() };
        let lines = super::paths(&layers, layers.len() * 4 / 5, machine, false);
        let mut camera = Camera { azimuth: -55.0, elevation: 28.0, ..Camera::default() };
        camera.frame_all(([-25.0, -10.0, 0.0], [25.0, 10.0, 20.0]));
        std::fs::write(dir.join("preview.png"), png(&draw_paths(&lines, &camera, p.bed, [960, 600]))).unwrap();
    }
}
