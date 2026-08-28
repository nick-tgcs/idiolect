//! A small, sleek dictation overlay pinned to the **text caret**, naming the
//! phase the take is in: a red mic labelled `RECORDING` while the microphone is
//! open, then a rotating ring with an elapsed-time label — `TRANSCRIBING · Ns`,
//! or `LOADING MODEL · Ns` when the speech model has to be read off disk first.
//!
//! The decode phase is why this shows more than a microphone. It runs after the
//! mic closes, takes seconds on a CPU build, and used to be invisible: the badge
//! kept pulsing as if still listening, then vanished, with nothing to tell a user
//! working from hung. There is no progress percentage to show (the decoder
//! reports none), so the ring is deliberately indeterminate and the elapsed
//! seconds carry the "still alive" signal instead.
//!
//! It is a tiny transparent, click-through, always-on-top borderless window.
//! The engine starts it ONCE, hidden, when the engine itself starts, and then
//! streams `"x y phase"` lines on stdin to move it, re-label it, and hide it
//! again (`hidden`). It is kept warm rather than respawned per take because
//! building the GL window costs about a third of a second — long enough that the
//! badge arrived visibly after the key that asked for it. The process ends when
//! it sees its stdin close — the engine parks forever and only ever dies by
//! signal, so that end-of-stream is both the notice it gets that the engine has
//! gone and, in practice, the only way it exits. Kept behind the engine's
//! `RecordingIndicator` trait so it is swappable.

use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui;

const ACCENT: egui::Color32 = egui::Color32::from_rgb(124, 131, 253);
const WARN: egui::Color32 = egui::Color32::from_rgb(224, 164, 74);
/// The "live microphone" red from the approved mockup.
const LIVE: egui::Color32 = egui::Color32::from_rgb(235, 87, 87);
const GLYPH: egui::Color32 = egui::Color32::from_rgb(240, 241, 250);
const LABEL_BACKDROP: egui::Color32 = egui::Color32::from_rgba_premultiplied(11, 12, 16, 232);
/// Wide enough for the longest label; the window is transparent and
/// click-through, so the unused area costs the user nothing.
const WIN: egui::Vec2 = egui::vec2(184.0, 56.0);
/// Where the mic badge sits inside the window. Fixed independently of [`WIN`] so
/// growing the window for the label cannot shift the badge off the caret.
const MIC_CENTER: egui::Pos2 = egui::pos2(28.0, 28.0);
/// The mic centre sits this far right of the caret; `y` is the caret's vertical
/// centre, so the badge lands right at the cursor, nudged to the right.
const MIC_RIGHT: f32 = 12.0;
/// The caption's font, and the spacing that places its pill beside the ring.
const LABEL_FONT: egui::FontId = egui::FontId::proportional(10.0);
const LABEL_GAP: f32 = 8.0;
const PILL_PAD: f32 = 6.0;
/// Radius of the progress ring — the same distance the recording pulse reaches at
/// its widest, so the two phases read as one object changing rather than two.
const RING_RADIUS: f32 = 18.0;

/// What the overlay is currently telling the user. Mirrors the engine-side
/// `ActivityPhase`, but this binary is deliberately free of idiolect
/// dependencies (it is a bare GUI process), so it re-parses the protocol words
/// instead of sharing the type.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Phase {
    /// No take in progress: the window stays built but unmapped. The overlay is
    /// kept warm between takes rather than respawned, because building a GL
    /// window costs about a third of a second — long enough that the badge
    /// arrived noticeably after the key that asked for it.
    Hidden,
    /// The microphone is open.
    #[default]
    Recording,
    /// The audio is being decoded.
    Transcribing,
    /// The speech model is being read off disk before it can decode — the cold
    /// start the daemon pays once. Normally only the first take after the daemon
    /// starts, and only if that take was short enough not to have loaded the
    /// model already while it was still running.
    LoadingModel,
}

impl Phase {
    /// The colour this phase is drawn in: the live red while the microphone is
    /// open, the accent while the machine is working, and a warmer tone for the
    /// cold-start wait the user actually notices.
    fn color(self) -> egui::Color32 {
        match self {
            Self::Hidden | Self::Recording => LIVE,
            Self::Transcribing => ACCENT,
            Self::LoadingModel => WARN,
        }
    }

    /// Whether this phase puts the badge on screen.
    fn is_visible(self) -> bool {
        !matches!(self, Self::Hidden)
    }
}

fn caret_to_window(x: f32, y: f32) -> egui::Pos2 {
    egui::pos2(
        (x + MIC_RIGHT - MIC_CENTER.x).max(0.0),
        (y - MIC_CENTER.y).max(0.0),
    )
}

/// Parse an `"x y [phase]"` line streamed on stdin. Returns `None` for malformed
/// lines so the reader thread can skip them. A missing or unrecognised phase word
/// falls back to `Recording` rather than dropping the line — a position update
/// must never be lost to a word this build does not know.
fn parse_caret(line: &str) -> Option<(f32, f32, Phase)> {
    let mut parts = line.split_whitespace();
    let x = parts.next()?.parse::<f32>().ok()?;
    let y = parts.next()?.parse::<f32>().ok()?;
    let phase = match parts.next() {
        Some("transcribing") => Phase::Transcribing,
        Some("loading") => Phase::LoadingModel,
        Some("hidden") => Phase::Hidden,
        _ => Phase::Recording,
    };
    Some((x, y, phase))
}

/// The caption for a phase, or `None` when the badge alone says it. Elapsed
/// seconds are truncated, so the label starts at `0s` and ticks up honestly.
fn label(phase: Phase, elapsed_secs: f32) -> Option<String> {
    let words = match phase {
        Phase::Hidden => return None,
        // No elapsed count while recording: the user knows how long they have
        // been talking, and a ticking timer beside their cursor is a distraction
        // during the one phase where they are the one doing the work.
        Phase::Recording => return Some("RECORDING".to_owned()),
        Phase::Transcribing => "TRANSCRIBING",
        Phase::LoadingModel => "LOADING MODEL",
    };
    Some(format!("{words} · {}s", elapsed_secs.max(0.0).trunc()))
}

/// How long the current phase has been showing. Restarted whenever the phase
/// changes, so the decode does not inherit the length of the take before it.
#[derive(Default)]
struct PhaseClock {
    started: Option<(Phase, f64)>,
}

impl PhaseClock {
    fn elapsed(&mut self, phase: Phase, now: f64) -> f32 {
        match self.started {
            Some((previous, since)) if previous == phase => (now - since) as f32,
            _ => {
                self.started = Some((phase, now));
                0.0
            }
        }
    }
}

/// What the engine has most recently told the overlay, written by the stdin
/// reader thread and read by the paint loop.
#[derive(Default)]
struct Overlay {
    caret: (f32, f32),
    phase: Phase,
}

/// Track the engine's `"x y phase"` stream until it ends, then flag that the
/// engine is gone.
///
/// The end of the stream is the only notice this process gets that its parent
/// died. Normally the engine kills us to hide the overlay; but if the ENGINE
/// itself is killed — a crash, an ibus restart, an upgrade — no `Drop` runs
/// (signals do not unwind), nothing reaps us, and the badge stays burned onto
/// the user's screen indefinitely with no take in progress. Closing on EOF is
/// what makes the overlay's lifetime genuinely bounded by the engine's.
fn follow_engine<R: BufRead>(
    reader: R,
    overlay: &Mutex<Overlay>,
    engine_gone: &AtomicBool,
    wake: &dyn Fn(),
) {
    for line in reader.lines() {
        let Ok(line) = line else { break };
        if let Some((nx, ny, phase)) = parse_caret(&line) {
            *overlay.lock().expect("overlay mutex") = Overlay {
                caret: (nx, ny),
                phase,
            };
            // A HIDDEN overlay draws nothing and requests no repaint, so egui
            // parks it. Only this wakes it for the next take.
            wake();
        }
    }
    engine_gone.store(true, Ordering::Relaxed);
    wake();
}

fn main() -> eframe::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (x, y, phase) = parse_caret(&args.join(" ")).unwrap_or((400.0, 400.0, Phase::Recording));

    // Latest caret position and phase, updated by a stdin reader thread as the
    // engine streams "x y phase" lines.
    let overlay = Arc::new(Mutex::new(Overlay {
        caret: (x, y),
        phase,
    }));
    // Set when the engine's pipe closes; the paint loop then closes the window.
    let engine_gone = Arc::new(AtomicBool::new(false));
    let options = eframe::NativeOptions {
        viewport: viewport(x, y, phase),
        ..Default::default()
    };

    eframe::run_native(
        "idiolect-recording-indicator",
        options,
        Box::new(move |cc| {
            let reader_overlay = Arc::clone(&overlay);
            let reader_gone = Arc::clone(&engine_gone);
            // The egui context is the only way to wake a parked paint loop, and
            // it only exists once the app is created — so the reader starts here.
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                let stdin = std::io::stdin();
                follow_engine(stdin.lock(), &reader_overlay, &reader_gone, &|| {
                    ctx.request_repaint();
                });
            });
            Ok(Box::new(Indicator {
                overlay,
                clock: PhaseClock::default(),
                engine_gone,
            }))
        }),
    )
}

/// The HUD window configuration, split out of `main` so its focus-proofing can be
/// asserted without launching a GUI (the `eframe::run_native` call is the real
/// GUI boundary and stays untestable).
///
/// This window must be a **passive, non-focusing overlay**: it appears mid-take,
/// on top of the app being dictated into, and must never take the input focus. If
/// it does, it registers an IBus input context and trades X11 focus with the app
/// frame-by-frame for the whole take — so the engine's `CommitText` races that flap
/// and the dictated text lands in the HUD (i.e. nowhere) about half the time. The
/// `Notification` window type is the decisive lever: WMs never give input focus to
/// `_NET_WM_WINDOW_TYPE_NOTIFICATION` windows (and docks never list them), so the app
/// keeps focus throughout and the commit is deterministic. `with_active(false)` (don't
/// activate on map) and `with_mouse_passthrough(true)` are belt-and-braces on the same
/// intent.
///
/// NOTE: this only works because the crate enables eframe's `x11` feature — egui-winit
/// applies `with_window_type` solely under `#[cfg(feature = "x11")]`, so without it the
/// type is silently dropped and the window reverts to a focus-stealing, dock-listed
/// `_NET_WM_WINDOW_TYPE_NORMAL`. The window type can't be asserted headlessly (it only
/// reaches X11 at `run_native`), so the guarantee is the `x11` feature + a manual xprop
/// check; the test below pins the `ViewportBuilder` config that feeds it.
fn viewport(x: f32, y: f32, phase: Phase) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_visible(phase.is_visible())
        .with_inner_size(WIN)
        .with_position(caret_to_window(x, y))
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_resizable(false)
        .with_mouse_passthrough(true)
        .with_active(false)
        .with_window_type(egui::X11WindowType::Notification)
        .with_taskbar(false)
        .with_title("idiolect-recording")
}

struct Indicator {
    overlay: Arc<Mutex<Overlay>>,
    clock: PhaseClock,
    /// Set once the engine's stream ends — see [`follow_engine`].
    engine_gone: Arc<AtomicBool>,
}

impl eframe::App for Indicator {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0] // fully transparent window
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}

impl Indicator {
    /// The per-frame draw, split out of `eframe::App::ui` so it can be driven
    /// headlessly in tests via `egui::Context::run_ui` (no `eframe::Frame`).
    fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.engine_gone.load(Ordering::Relaxed) {
            // The engine that owns this overlay has gone; take the badge with it
            // rather than leaving it stranded over the user's work.
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let Overlay {
            caret: (cx, cy),
            phase,
        } = *self.overlay.lock().expect("overlay mutex");
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(caret_to_window(
            cx, cy,
        )));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(phase.is_visible()));
        if !phase.is_visible() {
            // Between takes: keep the process (and its GL context) alive, draw
            // nothing, and stop animating so an idle overlay costs nothing.
            return;
        }

        ctx.request_repaint(); // keep animating + tracking the caret
        let t = ctx.input(|i| i.time);
        let elapsed = self.clock.elapsed(phase, t);
        let t = t as f32;

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                let center = ui.max_rect().min + MIC_CENTER.to_vec2();
                let painter = ui.painter();
                let color = phase.color();

                match phase {
                    // `Hidden` returned above; matched here only for totality.
                    Phase::Hidden | Phase::Recording => draw_pulse(painter, center, t, color),
                    Phase::Transcribing | Phase::LoadingModel => {
                        draw_spinner(painter, center, t, color);
                    }
                }

                // Accent badge with a subtle breathing size.
                let badge_r = 9.0 * (1.0 + (t * 3.0).sin() * 0.05);
                painter.circle_filled(center, badge_r, color);
                draw_mic_glyph(painter, center);

                if let Some(text) = label(phase, elapsed) {
                    draw_label(painter, ui, center, &text, color);
                }
            });
    }
}

/// The recording badge's soft outward pulse (~1.4 s period), in the badge's own
/// colour so the halo and the microphone read as one object.
fn draw_pulse(painter: &egui::Painter, center: egui::Pos2, t: f32, color: egui::Color32) {
    let phase = (t / 1.4).fract();
    painter.circle_stroke(
        center,
        9.0 + phase * (RING_RADIUS - 9.0),
        egui::Stroke::new(2.0, color.gamma_multiply((1.0 - phase) * 0.55)),
    );
}

/// An indeterminate rotating arc on a faint full-circle track.
///
/// Indeterminate on purpose: whisper reports no decode progress, and inventing a
/// percentage that jumps or stalls would be worse than admitting we cannot know.
/// The rotation says "working", the label's seconds say "for how long".
fn draw_spinner(painter: &egui::Painter, center: egui::Pos2, t: f32, color: egui::Color32) {
    painter.circle_stroke(
        center,
        RING_RADIUS,
        egui::Stroke::new(2.5, color.gamma_multiply(0.18)),
    );
    const SWEEP: f32 = std::f32::consts::FRAC_PI_2 * 1.15;
    const SEGMENTS: usize = 18;
    let start = t * 2.6;
    let points: Vec<egui::Pos2> = (0..=SEGMENTS)
        .map(|step| {
            let angle = start + SWEEP * (step as f32 / SEGMENTS as f32);
            center + egui::vec2(angle.cos(), angle.sin()) * RING_RADIUS
        })
        .collect();
    painter.add(egui::Shape::line(points, egui::Stroke::new(2.5, color)));
}

/// The microphone pictogram inside the badge.
fn draw_mic_glyph(painter: &egui::Painter, center: egui::Pos2) {
    let body = egui::Rect::from_center_size(center - egui::vec2(0.0, 1.5), egui::vec2(5.0, 8.5));
    painter.rect_filled(body, egui::CornerRadius::same(2), GLYPH);
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
        [
            center + egui::vec2(-2.5, 8.0),
            center + egui::vec2(2.5, 8.0),
        ],
        stroke,
    );
}

/// The caption, on its own dark pill so it stays readable over whatever the user
/// happens to be dictating into.
fn draw_label(
    painter: &egui::Painter,
    ui: &egui::Ui,
    center: egui::Pos2,
    text: &str,
    color: egui::Color32,
) {
    let galley = painter.layout_no_wrap(text.to_owned(), LABEL_FONT, color);
    let text_left = center.x + RING_RADIUS + LABEL_GAP;
    let pill = egui::Rect::from_min_size(
        egui::pos2(text_left - PILL_PAD, center.y - galley.size().y / 2.0 - 4.0),
        galley.size() + egui::vec2(PILL_PAD * 2.0, 8.0),
    );
    // A label wider than the window would be clipped mid-word; the window is
    // sized for the longest caption, so this only guards a future longer one.
    if pill.max.x <= ui.max_rect().max.x {
        painter.rect_filled(pill, egui::CornerRadius::same(7), LABEL_BACKDROP);
        painter.galley(
            egui::pos2(text_left, center.y - galley.size().y / 2.0),
            galley,
            color,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn caret_to_window_offsets_right_and_clamps_to_zero() {
        // The mic centre sits MIC_RIGHT past the caret, wherever in the window
        // that centre happens to be. Pinned against the anchor rather than the
        // window size, because the window has to be wide enough for the
        // transcribing label and the badge must NOT move when it grew.
        let p = caret_to_window(400.0, 400.0);
        assert_eq!(p.x + MIC_CENTER.x, 400.0 + MIC_RIGHT);
        assert_eq!(p.y + MIC_CENTER.y, 400.0);
        // Near the screen edge the position is clamped, never negative.
        let edge = caret_to_window(0.0, 0.0);
        assert_eq!(edge, egui::pos2(0.0, 0.0));
    }

    #[test]
    fn parse_caret_reads_valid_lines_and_rejects_junk() {
        assert_eq!(
            parse_caret("120 340"),
            Some((120.0, 340.0, Phase::Recording)),
            "a bare position keeps the microphone badge",
        );
        assert_eq!(
            parse_caret("  12.5   7  "),
            Some((12.5, 7.0, Phase::Recording))
        );
        assert_eq!(parse_caret(""), None);
        assert_eq!(parse_caret("100"), None);
        assert_eq!(parse_caret("left right"), None);
    }

    #[test]
    fn parse_caret_reads_the_phase_the_engine_streams() {
        // These words are a wire protocol with the IME engine, which pins the
        // same three in `indicator::phase_word`. Neither side changes alone.
        assert_eq!(
            parse_caret("10 20 recording"),
            Some((10.0, 20.0, Phase::Recording))
        );
        assert_eq!(
            parse_caret("10 20 transcribing"),
            Some((10.0, 20.0, Phase::Transcribing))
        );
        assert_eq!(
            parse_caret("10 20 loading"),
            Some((10.0, 20.0, Phase::LoadingModel))
        );
        // An unknown word must not lose the position: a future phase from a newer
        // engine still has to leave the overlay tracking the caret.
        assert_eq!(
            parse_caret("10 20 something-new"),
            Some((10.0, 20.0, Phase::Recording))
        );
    }

    #[test]
    fn the_working_phases_say_what_is_happening_and_for_how_long() {
        // Elapsed seconds are the point: they are what distinguishes "busy" from
        // "hung" when there is no percentage to show.
        assert_eq!(
            label(Phase::Transcribing, 3.4).as_deref(),
            Some("TRANSCRIBING · 3s")
        );
        assert_eq!(
            label(Phase::LoadingModel, 12.9).as_deref(),
            Some("LOADING MODEL · 12s")
        );
        // Recording is labelled too. Without it the badge was indistinguishable
        // from the plain mic overlay that predated all of this, so starting a
        // take looked like nothing had changed — the state the user is in was
        // the one state that never said what it was.
        assert_eq!(label(Phase::Recording, 5.0).as_deref(), Some("RECORDING"));
        // Hidden is the only phase with nothing to say.
        assert_eq!(label(Phase::Hidden, 5.0), None);
    }

    #[test]
    fn the_elapsed_clock_restarts_on_each_phase_change() {
        // Otherwise the decode would inherit the length of the take that produced
        // it and open at "TRANSCRIBING · 40s".
        let mut clock = PhaseClock::default();
        assert_eq!(clock.elapsed(Phase::Recording, 100.0), 0.0);
        assert_eq!(clock.elapsed(Phase::Recording, 104.0), 4.0);
        assert_eq!(
            clock.elapsed(Phase::Transcribing, 104.0),
            0.0,
            "the decode starts its own clock"
        );
        assert_eq!(clock.elapsed(Phase::Transcribing, 107.5), 3.5);
    }

    #[test]
    fn every_label_is_laid_out_and_fits_the_window() {
        // Two failures at once, both silent on screen:
        //
        // 1. Text that measures ZERO. eframe with `default-features = false`
        //    embeds no fonts unless `default_fonts` is on, and egui then lays
        //    every string out to nothing — which is exactly what shipped the
        //    first time: a correct-looking ring beside an empty pill.
        // 2. A caption too wide for the window, which `draw_label` drops rather
        //    than clipping mid-word.
        let ctx = egui::Context::default();
        let mut widest: f32 = 0.0;
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for phase in [Phase::Transcribing, Phase::LoadingModel] {
                // 9999s: the caption is at its longest when the clock has run up.
                let text = label(phase, 9999.0).expect("a working phase has a caption");
                let galley =
                    ui.painter()
                        .layout_no_wrap(text.clone(), LABEL_FONT, egui::Color32::WHITE);
                assert!(
                    galley.size().x > f32::from(u8::try_from(text.len()).unwrap_or(u8::MAX)),
                    "{text:?} laid out to {:?} — are the fonts compiled in?",
                    galley.size(),
                );
                widest = widest.max(galley.size().x);
            }
        });
        output.textures_delta.clear();
        let needed = MIC_CENTER.x + RING_RADIUS + LABEL_GAP + widest + PILL_PAD;
        assert!(
            needed <= WIN.x,
            "the widest caption needs {needed}px but the window is {}px",
            WIN.x,
        );
    }

    #[test]
    fn a_hidden_phase_unmaps_the_window_instead_of_ending_the_process() {
        // The overlay is kept WARM across takes: spawning a fresh GL window per
        // take cost ~0.35 s of process start, context creation and first paint,
        // so the badge arrived a third of a second after the user pressed the
        // key. Hiding is now a window command, and the next take is instant.
        let mut indicator = Indicator {
            overlay: Arc::new(Mutex::new(Overlay {
                caret: (250.0, 150.0),
                phase: Phase::Hidden,
            })),
            clock: PhaseClock::default(),
            engine_gone: Arc::new(AtomicBool::new(false)),
        };
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| indicator.draw(ui));
        output.textures_delta.clear();
        assert!(
            visibility_commands(&output).contains(&false),
            "a hidden phase must unmap the window",
        );
        assert!(
            !closes(&output),
            "hiding must NOT end the process — that is what made it slow",
        );
    }

    #[test]
    fn a_working_phase_maps_the_window() {
        for phase in [Phase::Recording, Phase::Transcribing, Phase::LoadingModel] {
            let mut indicator = Indicator {
                overlay: Arc::new(Mutex::new(Overlay {
                    caret: (250.0, 150.0),
                    phase,
                })),
                clock: PhaseClock::default(),
                engine_gone: Arc::new(AtomicBool::new(false)),
            };
            let ctx = egui::Context::default();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| indicator.draw(ui));
            output.textures_delta.clear();
            assert!(
                visibility_commands(&output).contains(&true),
                "{phase:?} must map the window",
            );
        }
    }

    #[test]
    fn a_warm_start_comes_up_hidden_so_nothing_flashes() {
        // Launched at engine start, long before any take: the window must be
        // built (paying the GL cost then) but never shown.
        let vb = viewport(400.0, 400.0, Phase::Hidden);
        assert_eq!(vb.visible, Some(false));
        // A take-time launch (the fallback path) still comes up visible.
        assert_ne!(
            viewport(400.0, 400.0, Phase::Recording).visible,
            Some(false)
        );
    }

    #[test]
    fn the_engine_can_ask_for_a_hidden_overlay_over_the_wire() {
        assert_eq!(
            parse_caret("10 20 hidden"),
            Some((10.0, 20.0, Phase::Hidden))
        );
        assert_eq!(label(Phase::Hidden, 5.0), None);
    }

    #[test]
    fn recording_is_drawn_in_the_live_colour_not_the_accent() {
        // The mockup gives the live microphone its own colour, so "I am
        // listening to you" never has to be told apart from "I am working".
        assert_eq!(Phase::Recording.color(), LIVE);
        assert_eq!(Phase::Transcribing.color(), ACCENT);
        assert_eq!(Phase::LoadingModel.color(), WARN);
    }

    #[test]
    fn the_recording_pulse_uses_the_live_colour_not_the_accent() {
        // The badge and its label went red, but the pulsing halo around them was
        // left on the accent — a red microphone inside a purple ring, which is
        // neither state the mockup describes (its recording ring is the live red
        // at low alpha). Asserted on hue rather than the exact value because the
        // alpha is animated: LIVE is red-dominant, ACCENT is blue-dominant, and
        // `gamma_multiply` scales every channel alike, so the ordering survives.
        let mut indicator = Indicator {
            overlay: Arc::new(Mutex::new(Overlay {
                caret: (250.0, 150.0),
                phase: Phase::Recording,
            })),
            clock: PhaseClock::default(),
            engine_gone: Arc::new(AtomicBool::new(false)),
        };
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| indicator.draw(ui));
        output.textures_delta.clear();

        let rings = circle_stroke_colors(&output);
        assert!(!rings.is_empty(), "the recording phase draws a pulse ring");
        for ring in rings {
            assert!(
                ring.r() >= ring.b(),
                "the pulse must be red-dominant like the badge, got {ring:?}",
            );
        }
    }

    /// The colour of every circle OUTLINE a frame drew.
    fn circle_stroke_colors(output: &egui::FullOutput) -> Vec<egui::Color32> {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Circle(circle) if circle.stroke.width > 0.0 => {
                    Some(circle.stroke.color)
                }
                _ => None,
            })
            .collect()
    }

    /// Every `Visible(..)` command a frame emitted.
    fn visibility_commands(output: &egui::FullOutput) -> Vec<bool> {
        output
            .viewport_output
            .values()
            .flat_map(|vp| vp.commands.iter())
            .filter_map(|cmd| match cmd {
                egui::ViewportCommand::Visible(v) => Some(*v),
                _ => None,
            })
            .collect()
    }

    fn closes(output: &egui::FullOutput) -> bool {
        output.viewport_output.values().any(|vp| {
            vp.commands
                .iter()
                .any(|cmd| matches!(cmd, egui::ViewportCommand::Close))
        })
    }

    #[test]
    fn every_engine_line_wakes_the_paint_loop() {
        // A hidden overlay draws nothing and asks for no repaint, so egui puts
        // it to sleep — correct, and free. But then only an incoming line can
        // wake it: without this the warm overlay would sleep through the next
        // take and never appear at all.
        let overlay = Arc::new(Mutex::new(Overlay::default()));
        let gone = Arc::new(AtomicBool::new(false));
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&wakes);
        follow_engine(
            std::io::Cursor::new("10 20 recording\n30 40 hidden\n"),
            &overlay,
            &gone,
            &move || {
                counter.fetch_add(1, Ordering::Relaxed);
            },
        );
        // Two lines, plus one final wake at end-of-stream so a parked overlay
        // acts on the engine's death promptly instead of hanging around.
        assert_eq!(wakes.load(Ordering::Relaxed), 3);
        assert!(gone.load(Ordering::Relaxed));
    }

    #[test]
    fn the_end_of_the_engines_stream_is_recorded_as_the_engine_going_away() {
        // The engine owns this process and kills it to hide the overlay. If the
        // engine instead DIES — a crash, a restart, a SIGKILL — nothing kills us:
        // `Drop` does not run on a signal, so the badge is stranded on the user's
        // screen for good. The closing of its pipe is the only notice we get.
        let overlay = Arc::new(Mutex::new(Overlay::default()));
        let gone = Arc::new(AtomicBool::new(false));
        follow_engine(
            std::io::Cursor::new("10 20 transcribing\n"),
            &overlay,
            &gone,
            &|| {},
        );
        assert_eq!(overlay.lock().unwrap().caret, (10.0, 20.0), "line applied");
        assert_eq!(overlay.lock().unwrap().phase, Phase::Transcribing);
        assert!(
            gone.load(Ordering::Relaxed),
            "end of stream means engine gone"
        );
    }

    #[test]
    fn a_departed_engine_closes_the_window_instead_of_stranding_it() {
        let mut indicator = Indicator {
            overlay: Arc::new(Mutex::new(Overlay::default())),
            clock: PhaseClock::default(),
            engine_gone: Arc::new(AtomicBool::new(true)),
        };
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| indicator.draw(ui));
        output.textures_delta.clear();
        let closed = output.viewport_output.values().any(|vp| {
            vp.commands
                .iter()
                .any(|cmd| matches!(cmd, egui::ViewportCommand::Close))
        });
        assert!(
            closed,
            "the overlay must close itself once the engine is gone"
        );
    }

    #[test]
    fn a_live_engine_keeps_the_window_open() {
        let mut indicator = Indicator {
            overlay: Arc::new(Mutex::new(Overlay::default())),
            clock: PhaseClock::default(),
            engine_gone: Arc::new(AtomicBool::new(false)),
        };
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| indicator.draw(ui));
        output.textures_delta.clear();
        let closed = output.viewport_output.values().any(|vp| {
            vp.commands
                .iter()
                .any(|cmd| matches!(cmd, egui::ViewportCommand::Close))
        });
        assert!(!closed, "a live take must not close the overlay");
    }

    #[test]
    fn every_phase_draws_a_frame_without_panicking() {
        for phase in [Phase::Recording, Phase::Transcribing, Phase::LoadingModel] {
            let mut indicator = Indicator {
                overlay: Arc::new(Mutex::new(Overlay {
                    caret: (250.0, 150.0),
                    phase,
                })),
                clock: PhaseClock::default(),
                engine_gone: Arc::new(AtomicBool::new(false)),
            };
            let ctx = egui::Context::default();
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| indicator.draw(ui));
            output.textures_delta.clear();
        }
    }

    #[test]
    fn ui_draws_a_frame_and_tracks_the_caret_headlessly() {
        let mut indicator = Indicator {
            overlay: Arc::new(Mutex::new(Overlay {
                caret: (250.0, 150.0),
                phase: Phase::Recording,
            })),
            clock: PhaseClock::default(),
            engine_gone: Arc::new(AtomicBool::new(false)),
        };
        let ctx = egui::Context::default();
        // Running a frame must not panic and must move the window to the caret.
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| indicator.draw(ui));
        // epaint 0.36 added a `Drop` guard that debug-asserts a `TexturesDelta`
        // was applied. There is no renderer here to apply one to, so discard it
        // explicitly — the escape hatch the assertion message itself names.
        output.textures_delta.clear();
        let moved_to_caret = output.viewport_output.values().any(|vp| {
            vp.commands.iter().any(|cmd| {
                matches!(cmd, egui::ViewportCommand::OuterPosition(p)
                    if *p == caret_to_window(250.0, 150.0))
            })
        });
        assert!(moved_to_caret, "indicator should reposition onto the caret");
    }

    #[test]
    fn viewport_is_a_passive_non_focusing_overlay() {
        let vb = viewport(400.0, 400.0, Phase::Recording);
        // The crux: a Notification-type window is never given input focus by the WM,
        // so the HUD can't steal the IBus input context from the app being dictated
        // into — without this the engine's CommitText races a focus flap and the
        // text vanishes on roughly every other take.
        assert_eq!(vb.window_type, Some(egui::X11WindowType::Notification));
        // Reinforcing the same "never take focus" intent.
        assert_eq!(vb.active, Some(false));
        assert_eq!(vb.mouse_passthrough, Some(true));
        assert_eq!(vb.taskbar, Some(false));
        // Still positioned on the caret it was launched at.
        assert_eq!(vb.position, Some(caret_to_window(400.0, 400.0)));
    }

    #[test]
    fn clear_color_is_fully_transparent() {
        let indicator = Indicator {
            overlay: Arc::new(Mutex::new(Overlay::default())),
            clock: PhaseClock::default(),
            engine_gone: Arc::new(AtomicBool::new(false)),
        };
        assert_eq!(
            eframe::App::clear_color(&indicator, &egui::Visuals::dark()),
            [0.0, 0.0, 0.0, 0.0]
        );
    }
}
