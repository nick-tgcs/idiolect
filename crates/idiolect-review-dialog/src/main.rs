//! Idiolect review dialog: a small, self-contained GUI that shows the dictated
//! text in an editable box we fully control, so the user's correction can be
//! captured no matter which application the text is destined for.
//!
//! The dialog is also the live mid-take surface: in "review before insert"
//! mode the engine opens it at the first pause and streams each snippet into
//! it, so the user watches the conversation grow in the SAME window that they
//! will edit at stop — there is no separate preview.
//!
//! Protocol (so the toolkit stays swappable behind the engine's `ReviewDialog`):
//!   stdin  : one command per line —
//!              `append <payload>` : one more pause-snippet; the dialog shows
//!                                   it read-only in its "listening" state.
//!              `final <payload>`  : the take is over; the full merged text
//!                                   replaces the draft, the dialog takes
//!                                   focus and becomes editable.
//!            Payloads escape backslash as `\\` and newline as `\n`.
//!            EOF before any `final` means the take was cancelled: close.
//!   stdout : on confirm, the final edited text; process exits 0.
//!   exit 1 : the user cancelled — `dialog::CANCELLED_MARKER` on stdout first.
//!   exit 2 : the dialog could not start at all; reason on stderr.
//!
//! The MARKER, not the exit code, is what proves a cancel. libX11's default
//! I/O-error handler calls `exit(1)` itself when the X connection drops, so on
//! the commonest runtime GUI death `main` never runs at all and cannot pick a
//! different code. Without something written on the way out, a dialog that
//! died holding the user's take was indistinguishable from Cancel — and the
//! engine discarded every word of it without saying so.
//!
//! This is one interchangeable implementation; the engine only knows the
//! stdin/stdout contract, never egui.

use std::io::BufRead;

use idiolect_process::dialog::{CANCELLED_MARKER, EXIT_CANCELLED, EXIT_UNAVAILABLE};
use std::sync::{Arc, Mutex};

use eframe::egui;

/// Result shared out of the egui app when the window closes.
#[derive(Default)]
struct Outcome {
    text: String,
    confirmed: bool,
}

/// Lines arriving from the engine on stdin, drained by the UI each frame.
#[derive(Default)]
struct Feed {
    lines: Vec<String>,
    eof: bool,
}

/// Decode a protocol payload: `\\` is a backslash, `\n` a newline.
fn unescape_payload(payload: &str) -> String {
    let mut output = String::with_capacity(payload.len());
    let mut escaping = false;
    for character in payload.chars() {
        if escaping {
            match character {
                'n' => output.push('\n'),
                other => output.push(other),
            }
            escaping = false;
        } else if character == '\\' {
            escaping = true;
        } else {
            output.push(character);
        }
    }
    output
}

fn main() {
    let feed = Arc::new(Mutex::new(Feed::default()));
    let reader_feed = Arc::clone(&feed);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            reader_feed.lock().expect("feed mutex").lines.push(line);
        }
        reader_feed.lock().expect("feed mutex").eof = true;
    });

    let outcome = Arc::new(Mutex::new(Outcome::default()));

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([580.0, 300.0])
            .with_min_inner_size([440.0, 240.0])
            .with_resizable(true)
            .with_decorations(false)
            .with_always_on_top()
            // The dialog can open mid-take (first pause): it must not steal
            // keystrokes from the app the user is dictating into. It takes
            // focus itself when the `final` text arrives.
            .with_active(false)
            .with_title("Idiolect — review"),
        ..Default::default()
    };

    let app_outcome = Arc::clone(&outcome);
    if let Err(error) = eframe::run_native(
        "idiolect-review",
        options,
        Box::new(move |cc| {
            install_theme(&cc.egui_ctx);
            Ok(Box::new(ReviewApp::new(feed, app_outcome)))
        }),
    ) {
        eprintln!("review dialog could not start: {error}");
        std::process::exit(EXIT_UNAVAILABLE);
    }

    let outcome = outcome.lock().expect("outcome mutex");
    if !outcome.confirmed {
        print!("{CANCELLED_MARKER}");
        let _ = std::io::Write::flush(&mut std::io::stdout());
        std::process::exit(EXIT_CANCELLED);
    }
    print!("{}", outcome.text);
    let _ = std::io::Write::flush(&mut std::io::stdout());
}

const ACCENT: egui::Color32 = egui::Color32::from_rgb(124, 131, 253);
const LIVE: egui::Color32 = egui::Color32::from_rgb(235, 87, 87);
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
    let corner_radius = egui::CornerRadius::same(10);
    v.window_corner_radius = egui::CornerRadius::same(14);
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
    v.window_shadow = egui::epaint::Shadow {
        offset: [0, 6],
        blur: 24,
        spread: 0,
        color: egui::Color32::from_black_alpha(120),
    };
    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(10.0, 12.0);
    style.spacing.button_padding = egui::vec2(16.0, 9.0);
    ctx.set_global_style(style);
}

struct ReviewApp {
    text: String,
    /// True while the take is still recording: text is read-only and grows
    /// per pause; confirm/cancel are disabled until `final` arrives.
    listening: bool,
    outcome: Arc<Mutex<Outcome>>,
    feed: Arc<Mutex<Feed>>,
    focused: bool,
    centered: bool,
}

impl ReviewApp {
    fn new(feed: Arc<Mutex<Feed>>, outcome: Arc<Mutex<Outcome>>) -> Self {
        Self {
            text: String::new(),
            listening: true,
            outcome,
            feed,
            focused: false,
            centered: false,
        }
    }

    /// Apply one protocol line. Returns true when this line ended the
    /// listening state (the caller then raises the window).
    fn apply_line(&mut self, line: &str) -> bool {
        if let Some(payload) = line.strip_prefix("append ") {
            self.text.push_str(&unescape_payload(payload));
            false
        } else if let Some(payload) = line.strip_prefix("final ") {
            self.text = unescape_payload(payload);
            let was_listening = self.listening;
            self.listening = false;
            // Re-request focus so the now-editable field is ready to type in.
            self.focused = false;
            was_listening
        } else {
            false
        }
    }

    /// The header label text. While recording it is the plain word "Recording"
    /// — the round status cue is *painted* (see `ui`) rather than carried by a
    /// `●` glyph the dialog font can't render (it showed as a tofu square).
    fn header_title(&self) -> &'static str {
        if self.listening {
            "Recording"
        } else {
            "Review dictation"
        }
    }

    /// The header label colour: recording-red while live, normal text once the
    /// take is finalized and editable.
    fn header_color(&self) -> egui::Color32 {
        if self.listening {
            LIVE
        } else {
            TEXT
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
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl ReviewApp {
    /// The per-frame draw, split out of `eframe::App::ui` so it can be driven
    /// headlessly in tests via `egui::Context::run_ui` (no `eframe::Frame`).
    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // Drain the engine's feed first so this frame draws the latest state.
        let (lines, eof) = {
            let mut feed = self.feed.lock().expect("feed mutex");
            (std::mem::take(&mut feed.lines), feed.eof)
        };
        let mut finalized = false;
        for line in &lines {
            finalized |= self.apply_line(line);
        }
        if finalized {
            // The take ended: now (and only now) the dialog may take focus,
            // so Ctrl+Enter / typing land here instead of in the user's app.
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if eof && self.listening {
            // The engine went away before the take finished (cancel, error,
            // daemon restart): close without a result.
            self.finish(&ctx, false);
            return;
        }

        self.center(&ctx);
        let mut action: Option<bool> = None; // Some(true)=insert, Some(false)=cancel
                                             // Keys act only if the frame STARTED reviewable — a keystroke racing
                                             // the `final` line must not confirm text the user never saw.
        if !self.listening && !finalized {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                action = Some(false);
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Enter) && i.modifiers.command) {
                action = Some(true);
            }
        }

        // Draggable header (the window is frameless) + title and hint.
        egui::Panel::top("header")
            .frame(egui::Frame::NONE.fill(BG).inner_margin(egui::Margin {
                left: 22,
                right: 22,
                top: 16,
                bottom: 6,
            }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if self.listening {
                        // A *painted* round recording dot. The header used to
                        // prefix the title with a `●` glyph, but the dialog font
                        // has no such glyph so it rendered as a tofu square —
                        // paint a real red disc instead, vertically centred in
                        // the row next to the title.
                        let radius = 6.0;
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(radius * 2.0, radius * 2.0),
                            egui::Sense::hover(),
                        );
                        ui.painter().circle_filled(rect.center(), radius, LIVE);
                        ui.add_space(8.0);
                    }
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(self.header_title())
                                .heading()
                                .strong()
                                .color(self.header_color()),
                        )
                        .selectable(false),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let hint = if self.listening {
                            "Keep talking — Super+T to finish"
                        } else {
                            "Ctrl+Enter to insert  ·  Esc to cancel"
                        };
                        ui.add(
                            egui::Label::new(egui::RichText::new(hint).small().color(MUTED))
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
        egui::Panel::bottom("actions")
            .frame(egui::Frame::NONE.fill(BG).inner_margin(egui::Margin {
                left: 22,
                right: 22,
                top: 12,
                bottom: 16,
            }))
            .show(ui, |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let insert = egui::Button::new(
                        egui::RichText::new("Insert")
                            .color(egui::Color32::WHITE)
                            .strong(),
                    )
                    .fill(ACCENT)
                    .min_size(egui::vec2(104.0, 34.0));
                    if ui.add_enabled(!self.listening, insert).clicked() {
                        action = Some(true);
                    }
                    ui.add_space(4.0);
                    let cancel = egui::Button::new(egui::RichText::new("Cancel").color(MUTED))
                        .fill(SURFACE)
                        .min_size(egui::vec2(96.0, 34.0));
                    if ui.add_enabled(!self.listening, cancel).clicked() {
                        action = Some(false);
                    }
                });
            });

        // The transcript fills the space between, scrolling if long. Read-only
        // while listening (the daemon owns the text until the take ends).
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(BG).inner_margin(egui::Margin {
                left: 22,
                right: 22,
                top: 2,
                bottom: 2,
            }))
            .show(ui, |ui| {
                let blurb = if self.listening {
                    "Each pause adds a phrase. Nothing is typed until you finish."
                } else {
                    "Edit it however you like — your fix is recorded for training."
                };
                ui.add(egui::Label::new(egui::RichText::new(blurb).color(MUTED)).selectable(false));
                ui.add_space(10.0);
                egui::ScrollArea::vertical()
                    .stick_to_bottom(self.listening)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let edit = egui::TextEdit::multiline(&mut self.text)
                            .desired_width(f32::INFINITY)
                            .desired_rows(6)
                            .interactive(!self.listening)
                            .font(egui::TextStyle::Body)
                            .margin(egui::vec2(12.0, 10.0));
                        let response = ui.add_sized(ui.available_size(), edit);
                        if !self.listening && !self.focused {
                            response.request_focus();
                            self.focused = true;
                        }
                    });
            });

        if let Some(confirmed) = action {
            self.finish(&ctx, confirmed);
        }

        // Poll the feed even while idle so new snippets (and the final text)
        // appear without user interaction.
        if self.listening {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> (ReviewApp, Arc<Mutex<Outcome>>, Arc<Mutex<Feed>>) {
        let outcome = Arc::new(Mutex::new(Outcome::default()));
        let feed = Arc::new(Mutex::new(Feed::default()));
        let app = ReviewApp::new(Arc::clone(&feed), Arc::clone(&outcome));
        (app, outcome, feed)
    }

    fn push(feed: &Arc<Mutex<Feed>>, line: &str) {
        feed.lock().unwrap().lines.push(line.to_owned());
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::RawInput {
        let mut input = egui::RawInput::default();
        // egui 0.36 removed `RawInput::modifiers`. `InputState::modifiers` —
        // what the Ctrl+Enter path reads — is now driven ONLY by
        // `Event::ModifiersChanged`; the `modifiers` on `Event::Key` is not
        // folded into it. A real winit backend sends the change ahead of the
        // key, so send it here too, or `i.modifiers.command` stays false and
        // the accept path under test never fires.
        input.events.push(egui::Event::ModifiersChanged(modifiers));
        input.events.push(egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        });
        input
    }

    fn ctrl_enter() -> egui::RawInput {
        key(
            egui::Key::Enter,
            egui::Modifiers {
                command: true,
                ctrl: true,
                ..Default::default()
            },
        )
    }

    fn run(app: &mut ReviewApp, input: egui::RawInput) {
        let ctx = egui::Context::default();
        install_theme(&ctx);
        let mut output = ctx.run_ui(input, |ui| app.draw(ui));
        // epaint 0.36 added a `Drop` guard that debug-asserts a `TexturesDelta`
        // was applied. There is no renderer here to apply one to, so discard it
        // explicitly — the escape hatch the assertion message itself names.
        output.textures_delta.clear();
    }

    #[test]
    fn header_reads_recording_while_listening_with_no_tofu_glyph() {
        // The header used to embed a `●` glyph the dialog font can't render, so
        // it showed as a tofu square. The round "recording" cue is now painted
        // (a GUI-only detail, unreachable headlessly); the label text must be
        // the plain word "Recording" — verb the user asked for, no glyph.
        let (app, _, _) = app();
        assert!(app.listening);
        assert_eq!(app.header_title(), "Recording");
        assert!(
            !app.header_title().contains('●'),
            "no tofu glyph in the label"
        );
        assert_eq!(app.header_color(), LIVE);
    }

    #[test]
    fn header_reads_review_after_final() {
        let (mut app, _, feed) = app();
        push(&feed, "final done");
        run(&mut app, egui::RawInput::default());
        assert!(!app.listening);
        assert_eq!(app.header_title(), "Review dictation");
        assert_eq!(app.header_color(), TEXT);
    }

    #[test]
    fn unescape_decodes_newlines_and_backslashes() {
        assert_eq!(unescape_payload("a\\nb"), "a\nb");
        assert_eq!(unescape_payload("a\\\\nb"), "a\\nb");
        assert_eq!(unescape_payload("plain"), "plain");
    }

    #[test]
    fn appended_snippets_accumulate_while_listening() {
        let (mut app, _, feed) = app();
        push(&feed, "append hello");
        run(&mut app, egui::RawInput::default());
        push(&feed, "append  world");
        run(&mut app, egui::RawInput::default());
        assert_eq!(app.text, "hello world");
        assert!(app.listening, "still mid-take");
    }

    #[test]
    fn confirm_and_cancel_are_inert_while_listening() {
        let (mut app, outcome, feed) = app();
        push(&feed, "append draft");
        run(&mut app, ctrl_enter());
        assert!(
            !outcome.lock().unwrap().confirmed,
            "Ctrl+Enter ignored mid-take"
        );
        run(&mut app, key(egui::Key::Escape, egui::Modifiers::default()));
        assert!(app.listening, "Escape ignored mid-take");
        assert_eq!(app.text, "draft", "text survives");
    }

    #[test]
    fn final_replaces_the_draft_and_enables_confirm() {
        let (mut app, outcome, feed) = app();
        push(&feed, "append helo");
        run(&mut app, egui::RawInput::default());
        push(&feed, "final hello world");
        run(&mut app, egui::RawInput::default());
        assert!(!app.listening);
        assert_eq!(
            app.text, "hello world",
            "merged final text replaces the draft"
        );

        app.text = "hello world!".to_owned(); // user edited the field
        run(&mut app, ctrl_enter());
        let out = outcome.lock().unwrap();
        assert!(out.confirmed);
        assert_eq!(out.text, "hello world!");
    }

    #[test]
    fn final_without_any_appends_goes_straight_to_review() {
        // A take with no mid-take pause: the dialog opens already finalized.
        let (mut app, outcome, feed) = app();
        push(&feed, "final quick note");
        run(&mut app, ctrl_enter());
        // The final applies on the same frame, but the key arrived before the
        // user could see the text — the NEXT Ctrl+Enter confirms.
        run(&mut app, ctrl_enter());
        let out = outcome.lock().unwrap();
        assert!(out.confirmed);
        assert_eq!(out.text, "quick note");
    }

    #[test]
    fn escape_cancels_after_final() {
        let (mut app, outcome, feed) = app();
        push(&feed, "final text");
        run(&mut app, egui::RawInput::default());
        run(&mut app, key(egui::Key::Escape, egui::Modifiers::default()));
        assert!(!outcome.lock().unwrap().confirmed);
    }

    #[test]
    fn eof_before_final_closes_without_confirming() {
        // Cancelled take / engine death: the feed ends with no final line.
        let (mut app, outcome, feed) = app();
        push(&feed, "append doomed");
        feed.lock().unwrap().eof = true;
        run(&mut app, egui::RawInput::default());
        assert!(!outcome.lock().unwrap().confirmed);
    }

    #[test]
    fn eof_after_final_keeps_the_dialog_open_for_editing() {
        // The engine closes its pipe right after `final` (it is waiting on our
        // exit); that EOF must not close the dialog under the user.
        let (mut app, outcome, feed) = app();
        push(&feed, "final keep me");
        feed.lock().unwrap().eof = true;
        run(&mut app, egui::RawInput::default());
        assert!(!app.listening);
        assert_eq!(app.text, "keep me");
        run(&mut app, ctrl_enter());
        assert!(outcome.lock().unwrap().confirmed);
    }
}
