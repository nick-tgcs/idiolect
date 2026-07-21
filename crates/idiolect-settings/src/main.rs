//! Idiolect Settings window: every multi-choice setting in one place, with an
//! explanation under each knob, applying instantly. It exists because the tray
//! menu (DBusMenu) closes on every click and the protocol cannot keep it open —
//! adjusting several settings there meant reopening the menu over and over.
//! This window stays open while you adjust things and closes when you click
//! anywhere else (focus loss), Esc, or the close button — like a menu, minus
//! the disappearing.
//!
//! Subprocess contract (the daemon's `SettingsLauncher` drives this):
//!   stdin  : ONE JSON line with the current effective settings.
//!   stdout : one tray action id per line as the user changes things
//!            (`settings:pause:2`, `translation:output:41`, `review_mode`…).
//!            The daemon applies each exactly like a tray click.
//!
//! The choice lists and index grammar come from `idiolect-application`'s menu
//! helpers — the same source the daemon parses with — so they cannot drift.
//!
//! Test note: the egui layer is a GUI boundary with no headless seam beyond
//! frame-driving; all decision logic lives in the pure [`Model`] below, which
//! is unit-tested directly (selection → action id, warning visibility, JSON
//! parsing). The window chrome (focus-loss close, combo rendering) is the
//! remaining unreachable sliver.

use std::io::Write as _;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

mod icon;
use idiolect_application::use_cases::menu::{
    max_entries_radio, retention_radio, timing_radio, training_retention_radio,
    translation_input_language_for_index, translation_input_radio,
    translation_output_language_for_index, translation_output_radio, AUTO_STOP_CHOICES_MS,
    MAX_PHRASE_CHOICES_MS, MIN_SPEECH_CHOICES_MS, PAUSE_CHOICES_MS, TRAINING_RETENTION_CHOICES,
};

/// One multi-choice knob: its option labels and the currently selected index.
struct Choice {
    options: Vec<String>,
    selected: usize,
}

impl Choice {
    fn new((options, selected): (Vec<String>, usize)) -> Self {
        Self { options, selected }
    }

    fn current_label(&self) -> &str {
        self.options.get(self.selected).map_or("", String::as_str)
    }

    /// Select `index`; returns true when it changed and is a real option.
    fn pick(&mut self, index: usize) -> bool {
        if index == self.selected || index >= self.options.len() {
            return false;
        }
        self.selected = index;
        true
    }
}

/// The window's pure decision model: holds the current values and turns user
/// picks into the daemon's tray-action ids.
struct Model {
    pause: Choice,
    min_speech: Choice,
    max_phrase: Choice,
    auto_stop: Choice,
    review_mode: bool,
    preview_typing: bool,
    translation_enabled: bool,
    input_lang: Choice,
    output_lang: Choice,
    output_code: String,
    translator_configured: bool,
    retention: Choice,
    max_entries: Choice,
    training: Choice,
    custom_training_days: u32,
}

fn u32_field(value: &serde_json::Value, key: &str, fallback: u32) -> u32 {
    value
        .get(key)
        .and_then(serde_json::Value::as_u64)
        .and_then(|raw| u32::try_from(raw).ok())
        .unwrap_or(fallback)
}

fn bool_field(value: &serde_json::Value, key: &str, fallback: bool) -> bool {
    value
        .get(key)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(fallback)
}

fn str_field<'a>(value: &'a serde_json::Value, key: &str, fallback: &'a str) -> &'a str {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .unwrap_or(fallback)
}

impl Model {
    /// Build from the daemon's state line. Unknown/missing fields fall back to
    /// the crate defaults so an older daemon still yields a usable window.
    fn from_json_line(line: &str) -> Self {
        let state: serde_json::Value =
            serde_json::from_str(line).unwrap_or(serde_json::Value::Null);
        let pause_ms = u32_field(&state, "pause_ms", 700);
        let min_speech_ms = u32_field(&state, "min_speech_ms", 250);
        let max_phrase_ms = u32_field(&state, "max_phrase_ms", 30_000);
        let auto_stop_ms = u32_field(&state, "auto_stop_ms", 0);
        let input_lang = str_field(&state, "input_lang", "auto");
        let output_lang = str_field(&state, "output_lang", "en");
        let training_days = u32_field(&state, "training_retention_days", 365);
        Self {
            pause: Choice::new(timing_radio(&PAUSE_CHOICES_MS, pause_ms, 700)),
            min_speech: Choice::new(timing_radio(&MIN_SPEECH_CHOICES_MS, min_speech_ms, 250)),
            max_phrase: Choice::new(timing_radio(&MAX_PHRASE_CHOICES_MS, max_phrase_ms, 30_000)),
            auto_stop: Choice::new(timing_radio(&AUTO_STOP_CHOICES_MS, auto_stop_ms, 0)),
            review_mode: bool_field(&state, "review_mode", false),
            // Default ON: absent ⇒ live preview typing, matching the daemon.
            preview_typing: bool_field(&state, "preview_typing", true),
            translation_enabled: bool_field(&state, "translation_enabled", false),
            input_lang: Choice::new(translation_input_radio(input_lang)),
            output_lang: Choice::new(translation_output_radio(output_lang)),
            output_code: output_lang.to_owned(),
            translator_configured: bool_field(&state, "translator_configured", false),
            retention: Choice::new(retention_radio(u32_field(&state, "retention_days", 1))),
            max_entries: Choice::new(max_entries_radio(u32_field(&state, "max_entries", 10))),
            training: Choice::new(training_retention_radio(training_days)),
            custom_training_days: training_days,
        }
    }

    fn pick_pause(&mut self, index: usize) -> Option<String> {
        // The trailing "(custom)" marker (if any) maps to no preset: inert.
        (index < PAUSE_CHOICES_MS.len() && self.pause.pick(index))
            .then(|| format!("settings:pause:{index}"))
    }

    fn pick_min_speech(&mut self, index: usize) -> Option<String> {
        (index < MIN_SPEECH_CHOICES_MS.len() && self.min_speech.pick(index))
            .then(|| format!("settings:min_speech:{index}"))
    }

    fn pick_max_phrase(&mut self, index: usize) -> Option<String> {
        (index < MAX_PHRASE_CHOICES_MS.len() && self.max_phrase.pick(index))
            .then(|| format!("settings:max_phrase:{index}"))
    }

    fn pick_auto_stop(&mut self, index: usize) -> Option<String> {
        (index < AUTO_STOP_CHOICES_MS.len() && self.auto_stop.pick(index))
            .then(|| format!("settings:auto_stop:{index}"))
    }

    fn toggle_review(&mut self) -> String {
        self.review_mode = !self.review_mode;
        "review_mode".to_owned()
    }

    fn toggle_preview_typing(&mut self) -> String {
        self.preview_typing = !self.preview_typing;
        "preview_typing".to_owned()
    }

    fn toggle_translation(&mut self) -> String {
        self.translation_enabled = !self.translation_enabled;
        "translation:enabled".to_owned()
    }

    fn pick_input_language(&mut self, index: usize) -> Option<String> {
        (translation_input_language_for_index(index).is_some() && self.input_lang.pick(index))
            .then(|| format!("translation:input:{index}"))
    }

    fn pick_output_language(&mut self, index: usize) -> Option<String> {
        let code = translation_output_language_for_index(index)?;
        if !self.output_lang.pick(index) {
            return None;
        }
        self.output_code = code.to_owned();
        Some(format!("translation:output:{index}"))
    }

    fn pick_retention(&mut self, index: usize) -> Option<String> {
        self.retention
            .pick(index)
            .then(|| format!("settings:retention:{index}"))
    }

    fn pick_max_entries(&mut self, index: usize) -> Option<String> {
        self.max_entries
            .pick(index)
            .then(|| format!("settings:max_entries:{index}"))
    }

    fn pick_training_retention(&mut self, index: usize) -> Option<String> {
        (index < TRAINING_RETENTION_CHOICES.len() && self.training.pick(index)).then(|| {
            self.custom_training_days = TRAINING_RETENTION_CHOICES[index].1;
            format!("settings:training_retention:{index}")
        })
    }

    fn set_custom_training_days(&mut self, days: u32) -> String {
        self.custom_training_days = days;
        self.training = Choice::new(training_retention_radio(days));
        format!("settings:training_retention_days:{days}")
    }

    /// The same up-front warning the tray shows: a non-English target without
    /// an external translator command fails every snippet.
    fn translation_warning(&self) -> Option<String> {
        if self.translation_enabled && self.output_code != "en" && !self.translator_configured {
            Some(format!(
                "⚠ {} won't work: only English does without a translator. Set translation.command in config.toml.",
                self.output_lang.current_label()
            ))
        } else {
            None
        }
    }
}

fn main() -> eframe::Result<()> {
    // The daemon writes exactly one state line before anything else.
    let mut state_line = String::new();
    let _ = std::io::stdin().read_line(&mut state_line);
    let model = Model::from_json_line(state_line.trim());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([560.0, 720.0])
            .with_min_inner_size([460.0, 420.0])
            .with_always_on_top()
            .with_icon(Arc::new(icon::window_icon()))
            .with_title("Idiolect — Settings"),
        ..Default::default()
    };

    eframe::run_native(
        "idiolect-settings",
        options,
        Box::new(move |cc| {
            install_theme(&cc.egui_ctx);
            Ok(Box::new(SettingsApp {
                model,
                dismiss: Dismiss::default(),
            }))
        }),
    )
}

const ACCENT: egui::Color32 = egui::Color32::from_rgb(124, 131, 253);
const WARN: egui::Color32 = egui::Color32::from_rgb(235, 87, 87);
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
            FontId::new(18.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(14.5, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(14.5, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(14.0, FontFamily::Monospace),
        ),
        (
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        ),
    ]
    .into();

    let mut v = egui::Visuals::dark();
    let corner_radius = egui::CornerRadius::same(8);
    v.window_fill = BG;
    v.panel_fill = BG;
    v.extreme_bg_color = FIELD;
    v.override_text_color = Some(TEXT);
    v.widgets.noninteractive.corner_radius = corner_radius;
    v.widgets.inactive.corner_radius = corner_radius;
    v.widgets.hovered.corner_radius = corner_radius;
    v.widgets.active.corner_radius = corner_radius;
    v.widgets.open.corner_radius = corner_radius;
    v.widgets.inactive.bg_fill = SURFACE;
    v.widgets.inactive.weak_bg_fill = SURFACE;
    let surface_hover = egui::Color32::from_rgb(44, 47, 60);
    v.widgets.hovered.bg_fill = surface_hover;
    v.widgets.hovered.weak_bg_fill = surface_hover;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    v.selection.stroke = egui::Stroke::new(1.0, ACCENT);
    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(10.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    ctx.set_global_style(style);
}

/// How long the window must stay unfocused AND stationary before a focus-loss is
/// treated as a click-away and dismisses it. Long enough to ride over a WM move
/// (focus returns when the drag ends) and the compositor's transient blips, short
/// enough that clicking away still feels like dismissing a menu.
const DISMISS_GRACE: Duration = Duration::from_millis(350);

/// Decides when a focus-loss should close the window. Closing on click-away is
/// intentional (it mirrors the tray menu it replaces), but a WM title-bar drag
/// also drops focus while streaming new positions, and compositors emit the odd
/// transient focus blip — both would otherwise slam the window shut. So a
/// dismissal fires only once the window has been BOTH unfocused and stationary
/// for [`DISMISS_GRACE`]: a move keeps resetting the timer (the position keeps
/// changing), a blip refocuses before it elapses, and a genuine click-away rides
/// it out. Pure and clock-injected so it is unit-tested without a display.
#[derive(Default)]
struct Dismiss {
    ever_focused: bool,
    last_pos: Option<(f32, f32)>,
    idle_since: Option<Instant>,
}

impl Dismiss {
    /// Feed one frame's `focused`/window-position; returns true to close now.
    fn poll(
        &mut self,
        focused: bool,
        pos: Option<(f32, f32)>,
        now: Instant,
        grace: Duration,
    ) -> bool {
        let moving = matches!(
            (self.last_pos, pos),
            (Some((ax, ay)), Some((bx, by))) if (ax - bx).abs() > 0.5 || (ay - by).abs() > 0.5
        );
        self.last_pos = pos;

        if focused {
            self.ever_focused = true;
            self.idle_since = None;
            return false;
        }
        // Opened (or still) without ever having had focus: never insta-close.
        if !self.ever_focused {
            return false;
        }
        if moving {
            // Being dragged by the window manager — stay open, reset the timer.
            self.idle_since = None;
            return false;
        }
        // Unfocused and stationary: start (or continue) timing, and only dismiss
        // once it has stayed that way for the whole grace.
        match self.idle_since {
            None => {
                self.idle_since = Some(now);
                false
            }
            Some(started) => now.duration_since(started) >= grace,
        }
    }

    /// True while a dismissal is being timed, so the caller can schedule a repaint.
    fn pending(&self) -> bool {
        self.idle_since.is_some()
    }
}

struct SettingsApp {
    model: Model,
    dismiss: Dismiss,
}

/// Print one action line for the daemon, immediately (the pipe is line-read).
fn emit(action: &str) {
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{action}");
    let _ = stdout.flush();
}

impl eframe::App for SettingsApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // "Click off" closes the window, like the menu it replaces — but a window
        // manager title-bar *move* also drops focus (while streaming the new
        // position), so dismissal is debounced through `Dismiss`: it fires only on
        // a sustained, stationary focus-loss (a real click-away), never mid-move
        // and not on the compositor's occasional transient blip.
        let focused = ctx.input(|i| i.focused);
        let pos = ctx.input(|i| i.viewport().outer_rect.map(|r| (r.min.x, r.min.y)));
        if self
            .dismiss
            .poll(focused, pos, Instant::now(), DISMISS_GRACE)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if self.dismiss.pending() {
            // Keep re-evaluating the grace even while no further input arrives.
            ctx.request_repaint_after(DISMISS_GRACE);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(BG)
                    .inner_margin(egui::Margin::same(18)),
            )
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(
                                "Changes apply immediately. Click anywhere else to close.",
                            )
                            .small()
                            .color(MUTED),
                        );
                        ui.add_space(6.0);
                        self.dictation_section(ui);
                        ui.add_space(14.0);
                        self.translation_section(ui);
                        ui.add_space(14.0);
                        self.history_section(ui);
                        ui.add_space(14.0);
                        self.training_section(ui);
                    });
            });
    }
}

impl SettingsApp {
    fn section_header(ui: &mut egui::Ui, title: &str) {
        ui.label(egui::RichText::new(title).heading().strong().color(ACCENT));
        ui.separator();
    }

    /// A labelled combo + muted explanation. `on_pick` maps the clicked option
    /// index to a daemon action.
    fn combo_row(
        ui: &mut egui::Ui,
        id: &str,
        label: &str,
        explanation: &str,
        options: &[String],
        selected_label: &str,
        mut on_pick: impl FnMut(usize) -> Option<String>,
    ) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(label).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                egui::ComboBox::from_id_salt(id)
                    .selected_text(selected_label.to_owned())
                    .width(170.0)
                    .show_ui(ui, |ui| {
                        for (index, option) in options.iter().enumerate() {
                            if ui
                                .selectable_label(option == selected_label, option)
                                .clicked()
                            {
                                if let Some(action) = on_pick(index) {
                                    emit(&action);
                                }
                            }
                        }
                    });
            });
        });
        ui.label(egui::RichText::new(explanation).small().color(MUTED));
    }

    fn dictation_section(&mut self, ui: &mut egui::Ui) {
        Self::section_header(ui, "Dictation");

        let mut review = self.model.review_mode;
        if ui
            .checkbox(
                &mut review,
                egui::RichText::new("Review before insert").strong(),
            )
            .changed()
        {
            emit(&self.model.toggle_review());
        }
        ui.label(
            egui::RichText::new(
                "On: the take collects in a dialog you edit and confirm before anything is typed. \
                 Off: each phrase types straight into the app as you pause.",
            )
            .small()
            .color(MUTED),
        );

        let mut preview = self.model.preview_typing;
        if ui
            .checkbox(&mut preview, egui::RichText::new("Preview typing").strong())
            .changed()
        {
            emit(&self.model.toggle_preview_typing());
        }
        ui.label(
            egui::RichText::new(
                "On: words appear as you speak, then the whole phrase is replaced with the \
                 verified text when you stop. Off: nothing is typed until you stop, then the \
                 verified text is inserted once. (Ignored in review mode.)",
            )
            .small()
            .color(MUTED),
        );

        let model = &mut self.model;
        let options = model.pause.options.clone();
        let current = model.pause.current_label().to_owned();
        Self::combo_row(
            ui,
            "pause",
            "Send a phrase after a pause of",
            "How long a pause finishes a phrase. Shorter feels snappier; longer joins hesitations together.",
            &options,
            &current,
            |index| model.pick_pause(index),
        );

        let options = model.min_speech.options.clone();
        let current = model.min_speech.current_label().to_owned();
        Self::combo_row(
            ui,
            "min_speech",
            "Ignore noises shorter than",
            "Sounds briefer than this are dropped as blips — knocks, clicks, breaths.",
            &options,
            &current,
            |index| model.pick_min_speech(index),
        );

        let options = model.max_phrase.options.clone();
        let current = model.max_phrase.current_label().to_owned();
        Self::combo_row(
            ui,
            "max_phrase",
            "Force-split non-stop speech after",
            "If you never pause, the phrase is split here anyway so text keeps appearing.",
            &options,
            &current,
            |index| model.pick_max_phrase(index),
        );

        let options = model.auto_stop.options.clone();
        let current = model.auto_stop.current_label().to_owned();
        Self::combo_row(
            ui,
            "auto_stop",
            "Stop listening after silence of",
            "End the take by itself after this much quiet. Never: only Super+T stops it.",
            &options,
            &current,
            |index| model.pick_auto_stop(index),
        );
    }

    fn translation_section(&mut self, ui: &mut egui::Ui) {
        Self::section_header(ui, "Translation");

        let mut enabled = self.model.translation_enabled;
        if ui
            .checkbox(
                &mut enabled,
                egui::RichText::new("Translate while dictating").strong(),
            )
            .changed()
        {
            emit(&self.model.toggle_translation());
        }
        ui.label(
            egui::RichText::new("Each phrase is translated as you pause, live.")
                .small()
                .color(MUTED),
        );

        if let Some(warning) = self.model.translation_warning() {
            ui.label(egui::RichText::new(warning).color(WARN));
        }

        let model = &mut self.model;
        let options = model.input_lang.options.clone();
        let current = model.input_lang.current_label().to_owned();
        Self::combo_row(
            ui,
            "input_lang",
            "Speak in",
            "The language you are speaking. Auto detect handles most speech fine.",
            &options,
            &current,
            |index| model.pick_input_language(index),
        );

        let options = model.output_lang.options.clone();
        let current = model.output_lang.current_label().to_owned();
        Self::combo_row(
            ui,
            "output_lang",
            "Translate to",
            "The language that gets typed. English works out of the box; anything else needs translation.command.",
            &options,
            &current,
            |index| model.pick_output_language(index),
        );
    }

    fn history_section(&mut self, ui: &mut egui::Ui) {
        Self::section_header(ui, "Tray history");

        let model = &mut self.model;
        let options = model.retention.options.clone();
        let current = model.retention.current_label().to_owned();
        Self::combo_row(
            ui,
            "retention",
            "Show dictations from the last",
            "How far back the tray's Recent History reaches.",
            &options,
            &current,
            |index| model.pick_retention(index),
        );

        let options = model.max_entries.options.clone();
        let current = model.max_entries.current_label().to_owned();
        Self::combo_row(
            ui,
            "max_entries",
            "Show at most",
            "The maximum number of entries the tray menu lists.",
            &options,
            &current,
            |index| model.pick_max_entries(index),
        );
    }

    fn training_section(&mut self, ui: &mut egui::Ui) {
        Self::section_header(ui, "Training data");

        let model = &mut self.model;
        let options = model.training.options.clone();
        let current = model.training.current_label().to_owned();
        Self::combo_row(
            ui,
            "training",
            "Keep recordings for",
            "How long audio + transcripts are retained to personalise your model. 0 days keeps them forever.",
            &options,
            &current,
            |index| model.pick_training_retention(index),
        );

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Custom:").strong());
            let mut days = self.model.custom_training_days;
            ui.add(
                egui::DragValue::new(&mut days)
                    .range(0..=36_500)
                    .suffix(" days"),
            );
            self.model.custom_training_days = days;
            if ui.button("Set").clicked() {
                emit(&self.model.set_custom_training_days(days));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATE: &str = r#"{"pause_ms":700,"min_speech_ms":250,"max_phrase_ms":30000,
        "auto_stop_ms":0,"review_mode":true,"preview_typing":false,"translation_enabled":true,
        "input_lang":"auto","output_lang":"zh","translator_configured":false,
        "retention_days":30,"max_entries":25,"training_retention_days":365}"#;

    #[test]
    fn parses_the_daemon_state_line_into_current_selections() {
        let model = Model::from_json_line(STATE);
        assert_eq!(model.pause.current_label(), "0.7 s (default)");
        assert_eq!(model.auto_stop.current_label(), "Never (default)");
        assert!(model.review_mode);
        assert!(
            !model.preview_typing,
            "the state line says preview typing off"
        );
        assert!(model.translation_enabled);
        assert_eq!(model.input_lang.current_label(), "Auto detect");
        assert_eq!(model.output_lang.current_label(), "Chinese");
        assert_eq!(model.retention.current_label(), "30 days");
        assert_eq!(model.max_entries.current_label(), "25");
        assert_eq!(model.training.current_label(), "1 year");
    }

    #[test]
    fn garbage_or_missing_state_falls_back_to_defaults() {
        let model = Model::from_json_line("not json at all");
        assert_eq!(model.pause.current_label(), "0.7 s (default)");
        assert!(!model.review_mode);
        assert!(
            model.preview_typing,
            "preview typing defaults ON when the daemon omits it"
        );
        assert_eq!(model.output_lang.current_label(), "English");
    }

    #[test]
    fn picks_emit_the_daemon_action_grammar() {
        // The ids must be byte-identical to what the tray used to emit — the
        // daemon parses both through the same handler.
        let mut model = Model::from_json_line(STATE);
        assert_eq!(model.pick_pause(2).as_deref(), Some("settings:pause:2"));
        assert_eq!(
            model.pick_min_speech(0).as_deref(),
            Some("settings:min_speech:0")
        );
        assert_eq!(
            model.pick_auto_stop(1).as_deref(),
            Some("settings:auto_stop:1")
        );
        assert_eq!(model.toggle_review(), "review_mode");
        assert_eq!(model.toggle_preview_typing(), "preview_typing");
        assert_eq!(model.toggle_translation(), "translation:enabled");
        assert_eq!(
            model.pick_input_language(1).as_deref(),
            Some("translation:input:1")
        );
        assert_eq!(
            model.pick_retention(0).as_deref(),
            Some("settings:retention:0")
        );
        assert_eq!(
            model.pick_max_entries(2).as_deref(),
            Some("settings:max_entries:2")
        );
        assert_eq!(
            model.pick_training_retention(0).as_deref(),
            Some("settings:training_retention:0")
        );
        assert_eq!(
            model.set_custom_training_days(540),
            "settings:training_retention_days:540"
        );
        assert_eq!(model.training.current_label(), "540 days (custom)");
    }

    #[test]
    fn repicking_the_current_value_or_out_of_range_is_inert() {
        let mut model = Model::from_json_line(STATE);
        assert_eq!(model.pick_pause(1), None, "already 0.7 s");
        assert_eq!(model.pick_pause(99), None, "out of range");
        assert_eq!(
            model.pick_output_language(9_999),
            None,
            "unknown language index"
        );
    }

    #[test]
    fn warning_mirrors_the_trays_unworkable_translation_rule() {
        let mut model = Model::from_json_line(STATE); // zh, no command, enabled
        let warning = model.translation_warning().expect("zh must warn");
        assert!(warning.contains("Chinese"), "{warning}");
        assert!(warning.contains("translation.command"), "{warning}");

        // Switching the target to English clears it.
        let english = translation_output_radio("en").1;
        assert!(model.pick_output_language(english).is_some());
        assert_eq!(model.translation_warning(), None);

        // And disabling translation clears it regardless of target.
        let mut model = Model::from_json_line(STATE);
        model.toggle_translation();
        assert_eq!(model.translation_warning(), None);
    }

    mod dismiss {
        use super::super::{Dismiss, DISMISS_GRACE};
        use std::time::{Duration, Instant};

        const GRACE: Duration = DISMISS_GRACE;
        const POS: Option<(f32, f32)> = Some((100.0, 100.0));

        #[test]
        fn opening_unfocused_never_dismisses() {
            // The window opens without focus (it must not steal it from the app);
            // a stream of unfocused frames before it is ever focused must not close.
            let mut d = Dismiss::default();
            let t = Instant::now();
            assert!(!d.poll(false, POS, t, GRACE));
            assert!(!d.poll(false, POS, t + Duration::from_secs(5), GRACE));
        }

        #[test]
        fn sustained_click_away_dismisses_after_the_grace() {
            let mut d = Dismiss::default();
            let t = Instant::now();
            assert!(!d.poll(true, POS, t, GRACE), "gained focus");
            // Stationary + unfocused, but the grace has not elapsed yet:
            assert!(!d.poll(false, POS, t + Duration::from_millis(10), GRACE));
            assert!(!d.poll(false, POS, t + Duration::from_millis(200), GRACE));
            // Past the grace, still stationary and unfocused: a real click-away.
            assert!(d.poll(false, POS, t + Duration::from_millis(400), GRACE));
        }

        #[test]
        fn moving_the_window_never_dismisses_however_long_it_takes() {
            // A WM title-bar drag drops focus and streams a new position every
            // frame for well past the grace; the window must survive the whole move.
            let mut d = Dismiss::default();
            let t0 = Instant::now();
            assert!(!d.poll(true, POS, t0, GRACE), "focused once");
            let mut x = 100.0_f32;
            for frame in 0..120 {
                x += 3.0; // position changes every frame
                let now = t0 + Duration::from_millis(50 + frame * 16);
                assert!(
                    !d.poll(false, Some((x, 100.0)), now, GRACE),
                    "must not dismiss mid-move at frame {frame}"
                );
            }
        }

        #[test]
        fn transient_blur_then_refocus_resets_and_does_not_dismiss() {
            let mut d = Dismiss::default();
            let t = Instant::now();
            assert!(!d.poll(true, POS, t, GRACE));
            // A couple of stationary unfocused frames (a compositor blip) under grace.
            assert!(!d.poll(false, POS, t + Duration::from_millis(16), GRACE));
            assert!(!d.poll(false, POS, t + Duration::from_millis(32), GRACE));
            // Focus returns: the timer resets.
            assert!(!d.poll(true, POS, t + Duration::from_millis(48), GRACE));
            // A fresh brief blur must start timing again, not insta-close.
            assert!(!d.poll(false, POS, t + Duration::from_millis(64), GRACE));
        }

        #[test]
        fn click_away_after_a_move_still_dismisses() {
            // Move ends (focus returns), then a genuine click-away must still work.
            let mut d = Dismiss::default();
            let t0 = Instant::now();
            d.poll(true, Some((100.0, 100.0)), t0, GRACE);
            d.poll(
                false,
                Some((130.0, 100.0)),
                t0 + Duration::from_millis(20),
                GRACE,
            ); // moving
            d.poll(
                true,
                Some((160.0, 100.0)),
                t0 + Duration::from_millis(40),
                GRACE,
            ); // released
            assert!(!d.poll(
                false,
                Some((160.0, 100.0)),
                t0 + Duration::from_millis(60),
                GRACE
            ));
            assert!(d.poll(
                false,
                Some((160.0, 100.0)),
                t0 + Duration::from_millis(500),
                GRACE
            ));
        }
    }
}
