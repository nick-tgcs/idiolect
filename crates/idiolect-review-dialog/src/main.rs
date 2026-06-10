//! Idiolect review dialog: a small, self-contained GUI that shows the dictated
//! text in an editable box we fully control, so the user's correction can be
//! captured no matter which application the text is destined for.
//!
//! Protocol (so the toolkit stays swappable behind the engine's `ReviewDialog`):
//!   stdin  : the raw transcript to review (UTF-8).
//!   stdout : on confirm, the final edited text; process exits 0.
//!   exit 1 : the user cancelled (nothing written).
//!
//! This is one interchangeable implementation; the engine only knows the
//! stdin/stdout contract, never egui.

use std::io::Read;
use std::sync::{Arc, Mutex};

use eframe::egui;

/// Result shared out of the egui app when the window closes.
#[derive(Default)]
struct Outcome {
    text: String,
    confirmed: bool,
}

fn main() -> eframe::Result<()> {
    let mut transcript = String::new();
    let _ = std::io::stdin().read_to_string(&mut transcript);
    let transcript = transcript.trim_end_matches('\n').to_owned();

    let outcome = Arc::new(Mutex::new(Outcome {
        text: transcript.clone(),
        confirmed: false,
    }));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([580.0, 300.0])
            .with_min_inner_size([440.0, 240.0])
            .with_resizable(true)
            .with_decorations(false)
            .with_always_on_top()
            .with_title("Idiolect — review"),
        ..Default::default()
    };

    let app_outcome = Arc::clone(&outcome);
    eframe::run_native(
        "idiolect-review",
        options,
        Box::new(move |cc| {
            install_theme(&cc.egui_ctx);
            Ok(Box::new(ReviewApp::new(transcript.clone(), app_outcome)))
        }),
    )?;

    let outcome = outcome.lock().expect("outcome mutex");
    if outcome.confirmed {
        print!("{}", outcome.text);
        Ok(())
    } else {
        std::process::exit(1);
    }
}

const ACCENT: egui::Color32 = egui::Color32::from_rgb(124, 131, 253);
const BG: egui::Color32 = egui::Color32::from_rgb(22, 23, 30);
const SURFACE: egui::Color32 = egui::Color32::from_rgb(31, 33, 43);
const FIELD: egui::Color32 = egui::Color32::from_rgb(16, 17, 23);
const TEXT: egui::Color32 = egui::Color32::from_rgb(229, 231, 240);
const MUTED: egui::Color32 = egui::Color32::from_rgb(140, 144, 161);

fn install_theme(ctx: &egui::Context) {
    use egui::{FontFamily, FontId, TextStyle};

    let mut style = (*ctx.style()).clone();
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(20.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(15.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(15.0, FontFamily::Monospace),
        ),
        (
            TextStyle::Small,
            FontId::new(12.5, FontFamily::Proportional),
        ),
    ]
    .into();

    let mut v = egui::Visuals::dark();
    let rounding = egui::Rounding::same(10.0);
    v.window_rounding = egui::Rounding::same(14.0);
    v.window_fill = BG;
    v.panel_fill = BG;
    v.extreme_bg_color = FIELD;
    v.override_text_color = Some(TEXT);
    v.widgets.noninteractive.rounding = rounding;
    v.widgets.inactive.rounding = rounding;
    v.widgets.hovered.rounding = rounding;
    v.widgets.active.rounding = rounding;
    v.widgets.open.rounding = rounding;
    v.widgets.inactive.bg_fill = SURFACE;
    v.widgets.inactive.weak_bg_fill = SURFACE;
    let surface_hover = egui::Color32::from_rgb(44, 47, 60);
    v.widgets.hovered.bg_fill = surface_hover;
    v.widgets.hovered.weak_bg_fill = surface_hover;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    v.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    v.window_shadow = egui::epaint::Shadow {
        offset: egui::vec2(0.0, 6.0),
        blur: 24.0,
        spread: 0.0,
        color: egui::Color32::from_black_alpha(120),
    };
    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(10.0, 12.0);
    style.spacing.button_padding = egui::vec2(16.0, 9.0);
    ctx.set_style(style);
}

struct ReviewApp {
    text: String,
    outcome: Arc<Mutex<Outcome>>,
    focused: bool,
    centered: bool,
}

impl ReviewApp {
    fn new(text: String, outcome: Arc<Mutex<Outcome>>) -> Self {
        Self {
            text,
            outcome,
            focused: false,
            centered: false,
        }
    }

    /// Place the window dead-center on the monitor (egui/winit don't do this for
    /// us). Runs on the first frame where the monitor + window sizes are known.
    fn center(&mut self, ctx: &egui::Context) {
        if self.centered {
            return;
        }
        let monitor = ctx.input(|i| i.viewport().monitor_size);
        let outer = ctx.input(|i| i.viewport().outer_rect.map(|r| r.size()));
        if let (Some(monitor), Some(size)) = (monitor, outer) {
            let pos = egui::pos2(
                ((monitor.x - size.x) / 2.0).max(0.0),
                ((monitor.y - size.y) / 2.0).max(0.0),
            );
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
            self.centered = true;
        }
    }

    fn finish(&self, ctx: &egui::Context, confirmed: bool) {
        let mut outcome = self.outcome.lock().expect("outcome mutex");
        outcome.text = self.text.clone();
        outcome.confirmed = confirmed;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

impl eframe::App for ReviewApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }
}

impl ReviewApp {
    /// The per-frame draw, split out of `eframe::App::update` so it can be
    /// driven headlessly in tests with a bare `egui::Context` (no `eframe::Frame`).
    fn ui(&mut self, ctx: &egui::Context) {
        self.center(ctx);
        let mut action: Option<bool> = None; // Some(true)=insert, Some(false)=cancel
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            action = Some(false);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command) {
            action = Some(true);
        }

        // Draggable header (the window is frameless) + title and hint.
        egui::TopBottomPanel::top("header")
            .frame(egui::Frame::none().fill(BG).inner_margin(egui::Margin {
                left: 22.0,
                right: 22.0,
                top: 16.0,
                bottom: 6.0,
            }))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new("Review dictation")
                                .heading()
                                .strong()
                                .color(TEXT),
                        )
                        .selectable(false),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("Ctrl+Enter to insert  ·  Esc to cancel")
                                    .small()
                                    .color(MUTED),
                            )
                            .selectable(false),
                        );
                    });
                });
                // Dragging anywhere on the header moves the whole window.
                let bar = ui.interact(
                    ui.max_rect(),
                    egui::Id::new("title_bar_drag"),
                    egui::Sense::click_and_drag(),
                );
                if bar.drag_started_by(egui::PointerButton::Primary) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
            });

        // Action buttons pinned to the bottom so they are never clipped.
        egui::TopBottomPanel::bottom("actions")
            .frame(egui::Frame::none().fill(BG).inner_margin(egui::Margin {
                left: 22.0,
                right: 22.0,
                top: 12.0,
                bottom: 16.0,
            }))
            .show(ctx, |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let insert = egui::Button::new(
                        egui::RichText::new("Insert")
                            .color(egui::Color32::WHITE)
                            .strong(),
                    )
                    .fill(ACCENT)
                    .min_size(egui::vec2(104.0, 34.0));
                    if ui.add(insert).clicked() {
                        action = Some(true);
                    }
                    ui.add_space(4.0);
                    let cancel = egui::Button::new(egui::RichText::new("Cancel").color(MUTED))
                        .fill(SURFACE)
                        .min_size(egui::vec2(96.0, 34.0));
                    if ui.add(cancel).clicked() {
                        action = Some(false);
                    }
                });
            });

        // The editable transcript fills the space between, scrolling if long.
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BG).inner_margin(egui::Margin {
                left: 22.0,
                right: 22.0,
                top: 2.0,
                bottom: 2.0,
            }))
            .show(ctx, |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            "Edit it however you like — your fix is recorded for training.",
                        )
                        .color(MUTED),
                    )
                    .selectable(false),
                );
                ui.add_space(10.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let edit = egui::TextEdit::multiline(&mut self.text)
                            .desired_width(f32::INFINITY)
                            .desired_rows(6)
                            .font(egui::TextStyle::Body)
                            .margin(egui::vec2(12.0, 10.0));
                        let response = ui.add_sized(ui.available_size(), edit);
                        if !self.focused {
                            response.request_focus();
                            self.focused = true;
                        }
                    });
            });

        if let Some(confirmed) = action {
            self.finish(ctx, confirmed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(text: &str) -> (ReviewApp, Arc<Mutex<Outcome>>) {
        let outcome = Arc::new(Mutex::new(Outcome::default()));
        let app = ReviewApp::new(text.to_owned(), Arc::clone(&outcome));
        (app, outcome)
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::RawInput {
        let mut input = egui::RawInput {
            modifiers,
            ..Default::default()
        };
        input.events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        });
        input
    }

    fn run(app: &mut ReviewApp, input: egui::RawInput) {
        let ctx = egui::Context::default();
        install_theme(&ctx);
        let _ = ctx.run(input, |ctx| app.ui(ctx));
    }

    #[test]
    fn renders_a_frame_without_input_and_does_not_confirm() {
        let (mut app, outcome) = app("hello world");
        run(&mut app, egui::RawInput::default());
        assert!(!outcome.lock().unwrap().confirmed);
    }

    #[test]
    fn ctrl_enter_confirms_with_the_edited_text() {
        let (mut app, outcome) = app("deploy traefik");
        app.text = "deploy traefik and nginx".to_owned(); // user edited the field
        run(
            &mut app,
            key(
                egui::Key::Enter,
                egui::Modifiers {
                    command: true,
                    ctrl: true,
                    ..Default::default()
                },
            ),
        );
        let out = outcome.lock().unwrap();
        assert!(out.confirmed);
        assert_eq!(out.text, "deploy traefik and nginx");
    }

    #[test]
    fn plain_enter_without_modifier_does_not_confirm() {
        let (mut app, outcome) = app("text");
        run(&mut app, key(egui::Key::Enter, egui::Modifiers::default()));
        assert!(!outcome.lock().unwrap().confirmed);
    }

    #[test]
    fn escape_cancels_without_confirming() {
        let (mut app, outcome) = app("text");
        run(&mut app, key(egui::Key::Escape, egui::Modifiers::default()));
        assert!(!outcome.lock().unwrap().confirmed);
    }
}
