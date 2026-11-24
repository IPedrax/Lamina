//! The layer heights strip beside the 3D view (View, Layer heights by hand): the plate's layer heights (`layer_heights`) drawn
//! against the height on the plate, and set by dragging in it.

use egui::{Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;

use crate::App;
use crate::lang::{t, tf};

/// The strip's state: whether it is shown, and where a drag in it started.
#[derive(Default)]
pub struct Strip {
    pub open: bool,
    /// Where a drag in the plot started, mm up the plate, and where the pointer last was during it.
    from: Option<f64>,
    last: Option<Pos2>,
    /// Where the plot was last drawn, for a test to drag in.
    pub(crate) plot: Option<Rect>,
}

/// A click in the plot sets this much either side of it, mm.
const CLICK_BAND: f64 = 0.5;

impl App {
    pub(crate) fn heights_ui(&mut self, ui: &mut Ui) {
        ui.label(RichText::new(t("Layer heights")).strong());
        let curve = self.session.scene.layer_heights.clone();
        ui.horizontal_wrapped(|ui| {
            if ui.button(t("Adaptive")).on_hover_text(t("Start from adaptive layers for the objects on the plate")).clicked() {
                self.run("layer_heights", json!({ "action": "adaptive" }));
            }
            if ui
                .add_enabled(curve.is_some(), egui::Button::new(t("Smooth")))
                .on_hover_text(t("Soften the steps between heights"))
                .clicked()
            {
                self.run("layer_heights", json!({ "action": "smooth" }));
            }
            if ui
                .add_enabled(curve.is_some(), egui::Button::new(t("Reset")))
                .on_hover_text(t("Back to the layer height in the settings"))
                .clicked()
            {
                self.run("layer_heights", json!({ "action": "clear" }));
            }
        });
        let top = self
            .session
            .scene
            .objects
            .iter()
            .filter(|o| o.modifier.is_none())
            .filter_map(|o| o.bounds())
            .map(|(_, hi)| hi[2])
            .fold(0.0, f64::max);
        if top <= 0.0 {
            ui.label(t("Put an object on the plate to set its layer heights."));
            return;
        }
        let p = &self.profile;
        let (lo, hi) = (lamina_core::scene::LAYER_HEIGHTS.0, (0.8 * p.nozzle).min(lamina_core::scene::LAYER_HEIGHTS.1));
        let tops: Vec<f64> = match curve.as_deref() {
            Some(c) => lamina_slicer::heights::tops(c, p.first_layer_height, top, lo, hi).unwrap_or_default(),
            None => {
                let n = if top <= p.first_layer_height { 1 } else { 1 + ((top - p.first_layer_height) / p.layer_height).ceil() as usize };
                (0..n).map(|k| p.first_layer_height + p.layer_height * k as f64).collect()
            }
        };
        ui.label(tf("{} layers", &[&tops.len().to_string()]));
        ui.label(RichText::new(t("Drag up the strip over the heights to change, and across to the layer height wanted.")).small());
        let size = vec2(ui.available_width(), (ui.available_height() - 8.0).max(80.0));
        let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
        self.strip.plot = Some(rect);
        let y_of = |z: f64| rect.bottom() - (z / top).clamp(0.0, 1.0) as f32 * rect.height();
        let x_of = |h: f64| rect.left() + (h / hi).clamp(0.0, 1.0) as f32 * rect.width();
        let z_at = |p: Pos2| f64::from((rect.bottom() - p.y) / rect.height()).clamp(0.0, 1.0) * top;
        // What the pointer asks for: a height to a hundredth of a millimetre.
        let h_at = |p: Pos2| ((f64::from((p.x - rect.left()) / rect.width()) * hi).clamp(lo, hi) * 100.0).round() / 100.0;
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals();
        painter.rect_filled(rect, 2.0, visuals.extreme_bg_color);
        // The layer height in the settings, and each layer at its middle.
        let base = x_of(p.layer_height);
        painter.line_segment([pos2(base, rect.top()), pos2(base, rect.bottom())], Stroke::new(1.0, visuals.weak_text_color()));
        let line: Vec<Pos2> = std::iter::once(0.0)
            .chain(tops.iter().copied())
            .collect::<Vec<f64>>()
            .windows(2)
            .map(|w| pos2(x_of(w[1] - w[0]), y_of((w[0] + w[1]) / 2.0)))
            .collect();
        painter.add(egui::Shape::line(line, Stroke::new(1.5, Color32::from_rgb(110, 165, 225))));
        let press = ui.input(|i| i.pointer.press_origin());
        if response.drag_started()
            && let Some(at) = press
        {
            self.strip.from = Some(z_at(at));
        }
        let pointer = response.interact_pointer_pos().or(response.hover_pos());
        if let Some(from) = self.strip.from {
            // The pointer may be gone on the frame the button comes up: the drag ends where it last was.
            self.strip.last = pointer.or(self.strip.last);
            if let Some(now) = self.strip.last {
                let (z, h) = (z_at(now), h_at(now));
                let band = Rect::from_x_y_ranges(rect.left()..=x_of(h), y_of(from.max(z))..=y_of(from.min(z)));
                painter.rect_filled(band, 0.0, Color32::from_rgba_unmultiplied(245, 165, 60, 90));
                if response.drag_stopped() || !response.dragged() {
                    (self.strip.from, self.strip.last) = (None, None);
                    let (a, b) = (from.min(z), from.max(z));
                    if b - a > 0.05 {
                        self.run("layer_heights", json!({ "action": "set", "from": a, "to": b, "height": h }));
                    }
                }
            }
        } else if response.clicked()
            && let Some(at) = pointer
        {
            let (z, h) = (z_at(at), h_at(at));
            self.run("layer_heights", json!({ "action": "set", "from": (z - CLICK_BAND).max(0.0), "to": z + CLICK_BAND, "height": h }));
        }
        if let Some(at) = response.hover_pos() {
            let text =
                format!("{} mm: {} mm", crate::lang::mark(format!("{:.1}", z_at(at))), crate::lang::mark(format!("{:.2}", h_at(at))));
            painter.text(rect.left_top() + vec2(4.0, 4.0), egui::Align2::LEFT_TOP, text, FontId::proportional(12.0), visuals.text_color());
        }
    }
}
