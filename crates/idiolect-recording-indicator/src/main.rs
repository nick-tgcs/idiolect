//! A small, sleek "voice is live" overlay: a microphone badge with a soft
//! pulsing ring, shown just to the right of the **text caret** while dictation
//! is recording, tracking it as it moves.
//!
//! It is a tiny transparent, click-through, always-on-top borderless window.
//! The engine launches it (with the caret's screen position as `x y` args) when
//! recording starts, streams updated caret positions on stdin (one `"x y"` line
//! each), and kills the process when recording stops. Kept behind the engine's
//! `RecordingIndicator` trait so it is swappable.

use std::io::BufRead;
use std::sync::{Arc, Mutex};

use eframe::egui;

const ACCENT: egui::Color32 = egui::Color32::from_rgb(124, 131, 253);
const GLYPH: egui::Color32 = egui::Color32::from_rgb(240, 241, 250);
const WIN: f32 = 48.0;
/// The mic centre sits this far right of the caret; `y` is the caret's vertical
/// centre, so the badge lands right at the cursor, nudged to the right.
const MIC_RIGHT: f32 = 12.0;

fn caret_to_window(x: f32, y: f32) -> egui::Pos2 {
    // Window is WIN×WIN with the mic at its centre (WIN/2, WIN/2).
    egui::pos2((x + MIC_RIGHT - WIN / 2.0).max(0.0), (y - WIN / 2.0).max(0.0))
}

fn main() -> eframe::Result<()> {
    let mut args = std::env::args().skip(1);
    let x: f32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(400.0);
    let y: f32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(400.0);

    // Latest caret position, updated by a stdin reader thread as the engine
    // streams "x y" lines.
    let caret = Arc::new(Mutex::new((x, y)));
    let reader_caret = Arc::clone(&caret);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            let mut parts = line.split_whitespace();
            if let (Some(nx), Some(ny)) = (
                parts.next().and_then(|s| s.parse::<f32>().ok()),
                parts.next().and_then(|s| s.parse::<f32>().ok()),
            ) {
                *reader_caret.lock().expect("caret mutex") = (nx, ny);
            }
        }
    });

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([WIN, WIN])
            .with_position(caret_to_window(x, y))
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_resizable(false)
            .with_mouse_passthrough(true)
            .with_taskbar(false)
            .with_title("idiolect-recording"),
        ..Default::default()
    };

    eframe::run_native(
        "idiolect-recording-indicator",
        options,
        Box::new(move |_cc| Ok(Box::new(Indicator { caret }))),
    )
}

struct Indicator {
    caret: Arc<Mutex<(f32, f32)>>,
}

impl eframe::App for Indicator {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0] // fully transparent window
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint(); // keep animating + tracking the caret

        let (cx, cy) = *self.caret.lock().expect("caret mutex");
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(caret_to_window(cx, cy)));

        let t = ctx.input(|i| i.time) as f32;

        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| {
                let center = ui.max_rect().center();
                let painter = ui.painter();

                // Soft pulsing ring (~1.4s period).
                let phase = (t / 1.4).fract();
                painter.circle_stroke(
                    center,
                    9.0 + phase * 9.0,
                    egui::Stroke::new(2.0, ACCENT.gamma_multiply((1.0 - phase) * 0.55)),
                );

                // Accent badge with a subtle breathing size.
                let badge_r = 9.0 * (1.0 + (t * 3.0).sin() * 0.05);
                painter.circle_filled(center, badge_r, ACCENT);

                // Microphone glyph.
                let body = egui::Rect::from_center_size(
                    center - egui::vec2(0.0, 1.5),
                    egui::vec2(5.0, 8.5),
                );
                painter.rect_filled(body, egui::Rounding::same(2.5), GLYPH);
                let stroke = egui::Stroke::new(1.3, GLYPH);
                painter.add(egui::Shape::CubicBezier(
                    egui::epaint::CubicBezierShape::from_points_stroke(
                        [
                            center + egui::vec2(-4.5, 1.5),
                            center + egui::vec2(-4.5, 6.0),
                            center + egui::vec2(4.5, 6.0),
                            center + egui::vec2(4.5, 1.5),
                        ],
                        false,
                        egui::Color32::TRANSPARENT,
                        stroke,
                    ),
                ));
                painter.line_segment(
                    [center + egui::vec2(0.0, 6.0), center + egui::vec2(0.0, 8.0)],
                    stroke,
                );
                painter.line_segment(
                    [center + egui::vec2(-2.5, 8.0), center + egui::vec2(2.5, 8.0)],
                    stroke,
                );
            });
    }
}
