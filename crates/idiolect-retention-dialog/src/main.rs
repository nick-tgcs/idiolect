//! Idiolect retention dialog: a small, self-contained GUI for entering a custom
//! training-data retention window. The tray menu can't take typed input, so the
//! daemon launches this when the user picks "Custom…".
//!
//! Protocol (so the toolkit stays swappable behind the daemon's `RetentionDialog`):
//!   args[1] : optional current retention, in days, used to prefill the field.
//!   stdout  : on save, the chosen retention as a whole number of DAYS; exits 0.
//!   exit 1  : the user cancelled (nothing written).
//!
//! This is one interchangeable implementation; the daemon only knows the
//! args/stdout contract, never egui.

use std::sync::{Arc, Mutex};

use eframe::egui;

/// Months are approximated as 30 days — matching the tray presets (1 month = 30).
const DAYS_PER_MONTH: u32 = 30;
/// Guard against a fat-fingered value (~100 years); mirrors the daemon's cap.
const MAX_DAYS: u32 = 36_500;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Unit {
    Days,
    Months,
}

impl Unit {
    fn to_days(self, amount: u32) -> u32 {
        match self {
            Unit::Days => amount,
            Unit::Months => amount.saturating_mul(DAYS_PER_MONTH),
        }
    }
}

#[derive(Default)]
struct Outcome {
    days: u32,
    confirmed: bool,
}

fn main() -> eframe::Result<()> {
    let prefill_days: u32 = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(365);

    let outcome = Arc::new(Mutex::new(Outcome::default()));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([420.0, 220.0])
            .with_min_inner_size([360.0, 200.0])
            .with_resizable(false)
            .with_decorations(false)
            .with_always_on_top()
            .with_title("Idiolect — retention"),
        ..Default::default()
    };

    let app_outcome = Arc::clone(&outcome);
    eframe::run_native(
        "idiolect-retention",
        options,
        Box::new(move |cc| {
            install_theme(&cc.egui_ctx);
            Ok(Box::new(RetentionApp::new(prefill_days, app_outcome)))
        }),
    )?;

    let outcome = outcome.lock().expect("outcome mutex");
    if outcome.confirmed {
        print!("{}", outcome.days);
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

    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(19.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(15.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Small,
            FontId::new(12.5, FontFamily::Proportional),
        ),
    ]
    .into();

    let mut v = egui::Visuals::dark();
    let corner_radius = egui::CornerRadius::same(10);
    v.window_corner_radius = egui::CornerRadius::same(14);
    v.window_fill = BG;
    v.panel_fill = BG;
    v.extreme_bg_color = FIELD;
    v.override_text_color = Some(TEXT);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = corner_radius;
    }
    v.widgets.inactive.bg_fill = SURFACE;
    v.widgets.inactive.weak_bg_fill = SURFACE;
    let surface_hover = egui::Color32::from_rgb(44, 47, 60);
    v.widgets.hovered.bg_fill = surface_hover;
    v.widgets.hovered.weak_bg_fill = surface_hover;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    v.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(10.0, 12.0);
    style.spacing.button_padding = egui::vec2(16.0, 9.0);
    ctx.set_global_style(style);
}

struct RetentionApp {
    amount: String,
    unit: Unit,
    outcome: Arc<Mutex<Outcome>>,
    focused: bool,
    centered: bool,
}

impl RetentionApp {
    fn new(prefill_days: u32, outcome: Arc<Mutex<Outcome>>) -> Self {
        Self {
            amount: prefill_days.to_string(),
            unit: Unit::Days,
            outcome,
            focused: false,
            centered: false,
        }
    }

    /// The resolved retention in days, or `None` if the field isn't a valid,
    /// in-range positive number.
    fn resolved_days(&self) -> Option<u32> {
        let amount: u32 = self.amount.trim().parse().ok()?;
        if amount == 0 {
            return None;
        }
        let days = self.unit.to_days(amount);
        (1..=MAX_DAYS).contains(&days).then_some(days)
    }

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

    fn finish(&self, ctx: &egui::Context, days: Option<u32>) {
        if let Some(days) = days {
            let mut outcome = self.outcome.lock().expect("outcome mutex");
            outcome.days = days;
            outcome.confirmed = true;
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

impl eframe::App for RetentionApp {
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // rendering is done via RetentionApp::ui(ctx) called from update()
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ui(ctx);
    }
}

impl RetentionApp {
    /// The per-frame draw, split out of `eframe::App::update` so it can be
    /// driven headlessly in tests with a bare `egui::Context` (no `eframe::Frame`).
    fn ui(&mut self, ctx: &egui::Context) {
        self.center(ctx);
        let resolved = self.resolved_days();

        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.finish(ctx, None);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
            if let Some(days) = resolved {
                self.finish(ctx, Some(days));
            }
        }

        // Draggable frameless header.
        egui::TopBottomPanel::top("header")
            .frame(egui::Frame::none().fill(BG).inner_margin(egui::Margin { left: 22, right: 22, top: 16, bottom: 6 }))
            .show(ctx, |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("Keep training data for")
                            .heading()
                            .strong()
                            .color(TEXT),
                    )
                    .selectable(false),
                );
                if ui
                    .interact(ui.min_rect(), ui.id().with("drag"), egui::Sense::drag())
                    .dragged()
                {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
            });

        egui::TopBottomPanel::bottom("actions")
            .frame(egui::Frame::none().fill(BG).inner_margin(egui::Margin { left: 22, right: 22, top: 8, bottom: 16 }))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let hint = match resolved {
                        Some(days) => format!("≈ {days} days"),
                        None => "enter a positive number".to_owned(),
                    };
                    ui.add(
                        egui::Label::new(egui::RichText::new(hint).small().color(MUTED))
                            .selectable(false),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                resolved.is_some(),
                                egui::Button::new(egui::RichText::new("Save").strong().color(BG))
                                    .fill(ACCENT),
                            )
                            .clicked()
                        {
                            self.finish(ctx, resolved);
                        }
                        if ui.button("Cancel").clicked() {
                            self.finish(ctx, None);
                        }
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(BG).inner_margin(egui::Margin::symmetric(22.0, 8.0)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let field = ui.add(
                        egui::TextEdit::singleline(&mut self.amount)
                            .desired_width(90.0)
                            .font(egui::TextStyle::Body),
                    );
                    if !self.focused {
                        field.request_focus();
                        self.focused = true;
                    }
                    ui.add_space(8.0);
                    ui.selectable_value(&mut self.unit, Unit::Days, "Days");
                    ui.selectable_value(&mut self.unit, Unit::Months, "Months");
                });
                ui.add_space(6.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("Older audio + transcripts are purged. 0 is not allowed here — use a preset to keep forever.")
                            .small()
                            .color(MUTED),
                    )
                    .selectable(false),
                );
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(amount: &str, unit: Unit) -> (RetentionApp, Arc<Mutex<Outcome>>) {
        let outcome = Arc::new(Mutex::new(Outcome::default()));
        let mut app = RetentionApp::new(365, Arc::clone(&outcome));
        app.amount = amount.to_owned();
        app.unit = unit;
        (app, outcome)
    }

    fn key(key: egui::Key) -> egui::RawInput {
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        });
        input
    }

    fn run(app: &mut RetentionApp, input: egui::RawInput) {
        let ctx = egui::Context::default();
        install_theme(&ctx);
        let _ = ctx.run(input, |ctx| app.ui(ctx));
    }

    #[test]
    fn unit_converts_months_to_days_and_saturates() {
        assert_eq!(Unit::Days.to_days(7), 7);
        assert_eq!(Unit::Months.to_days(2), 60);
        assert_eq!(Unit::Months.to_days(u32::MAX), u32::MAX); // saturating, no overflow
    }

    #[test]
    fn resolved_days_accepts_valid_and_rejects_out_of_range() {
        assert_eq!(app("30", Unit::Days).0.resolved_days(), Some(30));
        assert_eq!(app("2", Unit::Months).0.resolved_days(), Some(60));
        assert_eq!(app("0", Unit::Days).0.resolved_days(), None); // zero disallowed
        assert_eq!(app("  ", Unit::Days).0.resolved_days(), None); // blank
        assert_eq!(app("abc", Unit::Days).0.resolved_days(), None); // non-numeric
        assert_eq!(app("999999", Unit::Days).0.resolved_days(), None); // over MAX_DAYS
    }

    #[test]
    fn enter_with_valid_amount_confirms_in_days() {
        let (mut app, outcome) = app("3", Unit::Months);
        run(&mut app, key(egui::Key::Enter));
        let out = outcome.lock().unwrap();
        assert!(out.confirmed);
        assert_eq!(out.days, 90);
    }

    #[test]
    fn enter_with_invalid_amount_does_not_confirm() {
        let (mut app, outcome) = app("0", Unit::Days);
        run(&mut app, key(egui::Key::Enter));
        assert!(!outcome.lock().unwrap().confirmed);
    }

    #[test]
    fn escape_cancels_without_confirming() {
        let (mut app, outcome) = app("30", Unit::Days);
        run(&mut app, key(egui::Key::Escape));
        assert!(!outcome.lock().unwrap().confirmed);
    }

    #[test]
    fn clear_color_is_transparent() {
        let (app, _) = app("30", Unit::Days);
        assert_eq!(
            eframe::App::clear_color(&app, &egui::Visuals::dark()),
            [0.0, 0.0, 0.0, 0.0]
        );
    }
}
