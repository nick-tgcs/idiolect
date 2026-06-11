use idiolect_common::config::TranslationConfig;
use idiolect_common::languages::{language_name, LANGUAGES};
use idiolect_ports::storage::{HistoryEntry, HistoryState, TrayMenuItem, TrayMenuItemKind};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RecordingState {
    Idle,
    Recording,
}

/// Maximum number of characters shown for a history entry preview in the tray.
pub const MENU_PREVIEW_MAX_CHARS: usize = 40;

/// Allowed retention-day choices surfaced in the tray settings menu.
pub const RETENTION_DAY_CHOICES: [u32; 3] = [1, 7, 30];

/// Allowed max-entry choices surfaced in the tray settings menu.
pub const MAX_ENTRY_CHOICES: [u32; 3] = [10, 25, 50];

/// Training-data retention presets shown in the tray, as `(label, days)`. The
/// user may also pick a free-form value via the "Custom…" item, so these are
/// conveniences, not the full set of valid values.
pub const TRAINING_RETENTION_CHOICES: [(&str, u32); 8] = [
    ("1 month", 30),
    ("3 months", 90),
    ("6 months", 180),
    ("9 months", 270),
    ("1 year", 365),
    ("2 years", 730),
    ("4 years", 1460),
    ("10 years", 3650),
];

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum MenuUseCaseError {
    #[error("invalid retention days: {0} (must be 1, 7, or 30)")]
    InvalidRetentionDays(u32),
    #[error("invalid max entries: {0} (must be 10, 25, or 50)")]
    InvalidMaxEntries(u32),
    #[error("invalid training retention days: {0} (must be 0..={max})", max = idiolect_common::config::MAX_TRAINING_RETENTION_DAYS)]
    InvalidTrainingRetentionDays(u32),
}

/// Validates a retention-day value against the allowed tray choices.
///
/// # Errors
/// Returns [`MenuUseCaseError::InvalidRetentionDays`] if `days` is not 1, 7, or 30.
pub fn validate_retention_days(days: u32) -> Result<(), MenuUseCaseError> {
    if RETENTION_DAY_CHOICES.contains(&days) {
        Ok(())
    } else {
        Err(MenuUseCaseError::InvalidRetentionDays(days))
    }
}

/// Validates a max-entries value against the allowed tray choices.
///
/// # Errors
/// Returns [`MenuUseCaseError::InvalidMaxEntries`] if `max` is not 10, 25, or 50.
pub fn validate_max_entries(max: u32) -> Result<(), MenuUseCaseError> {
    if MAX_ENTRY_CHOICES.contains(&max) {
        Ok(())
    } else {
        Err(MenuUseCaseError::InvalidMaxEntries(max))
    }
}

/// Validates a training-data retention value. Unlike the tray-history settings
/// this is free-form (presets + custom): any value from `0` (keep forever) up to
/// the sanity cap is accepted.
///
/// # Errors
/// Returns [`MenuUseCaseError::InvalidTrainingRetentionDays`] if `days` exceeds
/// [`idiolect_common::config::MAX_TRAINING_RETENTION_DAYS`].
pub fn validate_training_retention_days(days: u32) -> Result<(), MenuUseCaseError> {
    if days <= idiolect_common::config::MAX_TRAINING_RETENTION_DAYS {
        Ok(())
    } else {
        Err(MenuUseCaseError::InvalidTrainingRetentionDays(days))
    }
}

/// The radio options + selected index for the training-retention menu. When the
/// current value isn't one of the presets (a custom value), an extra
/// "N days (custom)" option is appended and selected so the menu always shows the
/// active choice.
pub fn training_retention_radio(current_days: u32) -> (Vec<String>, usize) {
    let mut options: Vec<String> = TRAINING_RETENTION_CHOICES
        .iter()
        .map(|(label, _)| (*label).to_owned())
        .collect();
    match TRAINING_RETENTION_CHOICES
        .iter()
        .position(|(_, days)| *days == current_days)
    {
        Some(index) => (options, index),
        None => {
            options.push(format!("{current_days} days (custom)"));
            let selected = options.len() - 1;
            (options, selected)
        }
    }
}

/// Dictation-timing presets surfaced in the tray (milliseconds). Each radio's
/// parent label explains the behaviour, since DBusMenu offers no tooltips.
///
/// "Send a phrase after a pause of": the silence that completes a snippet.
pub const PAUSE_CHOICES_MS: [u32; 4] = [400, 700, 1_200, 2_000];
/// "Ignore noises shorter than": bursts below this are dropped as blips.
pub const MIN_SPEECH_CHOICES_MS: [u32; 3] = [150, 250, 400];
/// "Force-split non-stop speech after": cap on a single unpaused phrase.
pub const MAX_PHRASE_CHOICES_MS: [u32; 3] = [15_000, 30_000, 60_000];
/// "Stop listening after silence of": 0 = never (listening never times out).
pub const AUTO_STOP_CHOICES_MS: [u32; 4] = [0, 5_000, 10_000, 30_000];

/// Maps a `settings:pause:N` menu index back to milliseconds.
#[must_use]
pub fn pause_ms_for_index(index: usize) -> Option<u32> {
    PAUSE_CHOICES_MS.get(index).copied()
}

/// Maps a `settings:min_speech:N` menu index back to milliseconds.
#[must_use]
pub fn min_speech_ms_for_index(index: usize) -> Option<u32> {
    MIN_SPEECH_CHOICES_MS.get(index).copied()
}

/// Maps a `settings:max_phrase:N` menu index back to milliseconds.
#[must_use]
pub fn max_phrase_ms_for_index(index: usize) -> Option<u32> {
    MAX_PHRASE_CHOICES_MS.get(index).copied()
}

/// Maps a `settings:auto_stop:N` menu index back to milliseconds (0 = never).
#[must_use]
pub fn auto_stop_ms_for_index(index: usize) -> Option<u32> {
    AUTO_STOP_CHOICES_MS.get(index).copied()
}

/// A human label for a timing value: "Never" for 0, otherwise seconds
/// ("0.25 s", "0.7 s", "30 s").
pub fn seconds_label(ms: u32) -> String {
    if ms == 0 {
        return "Never".to_owned();
    }
    let seconds = f64::from(ms) / 1_000.0;
    format!("{seconds} s")
}

/// Radio options + selected index for one timing knob. The crate default is
/// marked "(default)"; a config-set value outside the presets is appended as
/// "N ms (custom)" and selected, so the menu always shows the active choice.
pub fn timing_radio(choices: &[u32], current: u32, default: u32) -> (Vec<String>, usize) {
    let mut options: Vec<String> = choices
        .iter()
        .map(|&ms| {
            let base = seconds_label(ms);
            if ms == default {
                format!("{base} (default)")
            } else {
                base
            }
        })
        .collect();
    match choices.iter().position(|&ms| ms == current) {
        Some(index) => (options, index),
        None => {
            options.push(format!("{current} ms (custom)"));
            let selected = options.len() - 1;
            (options, selected)
        }
    }
}

/// Maps a `translation:input:N` menu index back to a language code: index 0 is
/// the "Auto detect" entry, the rest follow the catalogue order.
#[must_use]
pub fn translation_input_language_for_index(index: usize) -> Option<&'static str> {
    if index == 0 {
        return Some("auto");
    }
    LANGUAGES.get(index - 1).map(|(code, _)| *code)
}

/// Maps a `translation:output:N` menu index back to a language code. Outputs
/// have no auto-detect entry — the target must be a concrete language.
#[must_use]
pub fn translation_output_language_for_index(index: usize) -> Option<&'static str> {
    LANGUAGES.get(index).map(|(code, _)| *code)
}

/// The "Speak in" radio options plus the selected index for the current code.
/// An unknown stored code falls back to "Auto detect" rather than panicking.
pub fn translation_input_radio(current: &str) -> (Vec<String>, usize) {
    let mut options = Vec::with_capacity(LANGUAGES.len() + 1);
    options.push("Auto detect".to_owned());
    options.extend(LANGUAGES.iter().map(|(_, name)| (*name).to_owned()));
    let selected = if current == "auto" {
        0
    } else {
        LANGUAGES
            .iter()
            .position(|(code, _)| *code == current)
            .map_or(0, |position| position + 1)
    };
    (options, selected)
}

/// The "Translate to" radio options plus the selected index. An unknown stored
/// code falls back to English (the engine-internal translation target).
pub fn translation_output_radio(current: &str) -> (Vec<String>, usize) {
    let options: Vec<String> = LANGUAGES
        .iter()
        .map(|(_, name)| (*name).to_owned())
        .collect();
    let selected = LANGUAGES
        .iter()
        .position(|(code, _)| *code == current)
        .unwrap_or(0);
    (options, selected)
}

const REDACTED: &str = "[redacted]";

/// Keywords whose adjacent value is treated as a secret and masked.
const SENSITIVE_KEYWORDS: &[&str] = &[
    "password", "passwd", "pwd", "token", "secret", "apikey", "api_key", "api-key", "bearer",
];

/// Masks likely-sensitive content (secret keyword values, emails, long digit
/// runs) for display in the tray. This is a display-only defence-in-depth step;
/// the stored text and clipboard/reinsert paths always use the real value.
///
/// Whitespace is normalised to single spaces — acceptable for a menu preview.
#[must_use]
pub fn mask_sensitive(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut redact_next = false;

    for token in text.split_whitespace() {
        if redact_next {
            out.push(REDACTED.to_owned());
            redact_next = false;
            continue;
        }

        if let Some(masked) = mask_inline_keyword(token) {
            out.push(masked);
            continue;
        }

        let keyword_candidate = token.trim_end_matches([':', '=']);
        if is_sensitive_keyword(keyword_candidate) {
            out.push(token.to_owned());
            redact_next = true;
            continue;
        }

        if looks_like_email(token) || looks_like_long_number(token) {
            out.push(REDACTED.to_owned());
            continue;
        }

        out.push(token.to_owned());
    }

    out.join(" ")
}

fn is_sensitive_keyword(candidate: &str) -> bool {
    let lowered = candidate.to_ascii_lowercase();
    SENSITIVE_KEYWORDS.contains(&lowered.as_str())
}

/// Masks `key=value` / `key:value` tokens whose key is sensitive.
fn mask_inline_keyword(token: &str) -> Option<String> {
    let separator = token.find([':', '='])?;
    let (key, rest) = token.split_at(separator);
    let value = &rest[1..];
    if value.is_empty() || !is_sensitive_keyword(key) {
        return None;
    }
    let separator_char = rest.as_bytes()[0] as char;
    Some(format!("{key}{separator_char}{REDACTED}"))
}

fn looks_like_email(token: &str) -> bool {
    // Trim trailing punctuation so "me@example.com." still matches.
    let token = token.trim_end_matches([',', '.', ';', ':']);
    let Some((local, domain)) = token.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && domain.contains('.')
        && domain.split('.').all(|label| !label.is_empty())
        && !token.contains(char::is_whitespace)
}

/// Detects card-/account-like tokens: 12+ digits once separators are removed.
fn looks_like_long_number(token: &str) -> bool {
    let digits = token
        .chars()
        .filter(|character| !matches!(character, '-' | ' '))
        .collect::<Vec<_>>();
    digits.len() >= 12 && digits.iter().all(char::is_ascii_digit)
}

/// Truncates `text` to at most `max_chars` characters (not bytes), appending an
/// ellipsis when truncation occurs. Operates on `char` boundaries so it never
/// panics on multibyte UTF-8.
#[must_use]
pub fn truncate_for_menu(text: &str, max_chars: usize) -> String {
    if text.chars().count() > max_chars {
        let truncated: String = text.chars().take(max_chars).collect();
        format!("{truncated}…")
    } else {
        text.to_owned()
    }
}

pub struct MenuUseCase;

impl MenuUseCase {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    pub fn get_menu(
        &self,
        recording_state: RecordingState,
        history: &[HistoryEntry],
        translation: &TranslationConfig,
    ) -> Vec<TrayMenuItem> {
        let mut items = Vec::new();

        // Start/Stop Recording
        match recording_state {
            RecordingState::Idle => {
                items.push(TrayMenuItem {
                    id: "start_recording".to_owned(),
                    label: "Start Recording".to_owned(),
                    enabled: true,
                    kind: TrayMenuItemKind::Standard { submenu: None },
                });
                items.push(TrayMenuItem {
                    id: "stop_recording".to_owned(),
                    label: "Stop & Insert".to_owned(),
                    enabled: false,
                    kind: TrayMenuItemKind::Standard { submenu: None },
                });
            }
            RecordingState::Recording => {
                items.push(TrayMenuItem {
                    id: "start_recording".to_owned(),
                    label: "Start Recording".to_owned(),
                    enabled: false,
                    kind: TrayMenuItemKind::Standard { submenu: None },
                });
                items.push(TrayMenuItem {
                    id: "stop_recording".to_owned(),
                    label: "Stop & Insert".to_owned(),
                    enabled: true,
                    kind: TrayMenuItemKind::Standard { submenu: None },
                });
            }
        }

        // Cancel
        items.push(TrayMenuItem {
            id: "cancel".to_owned(),
            label: "Cancel (discard)".to_owned(),
            enabled: matches!(recording_state, RecordingState::Recording),
            kind: TrayMenuItemKind::Standard { submenu: None },
        });

        // Separator
        items.push(TrayMenuItem {
            id: "sep1".to_owned(),
            label: String::new(),
            enabled: false,
            kind: TrayMenuItemKind::Separator,
        });

        // Recent History submenu
        let history_items: Vec<TrayMenuItem> = history
            .iter()
            .map(|entry| {
                let label = if entry.text.is_empty() {
                    "[cancelled]".to_owned()
                } else {
                    truncate_for_menu(&mask_sensitive(&entry.text), MENU_PREVIEW_MAX_CHARS)
                };

                let state_label = match entry.state {
                    HistoryState::Committed => "✓",
                    HistoryState::Cancelled => "✗",
                };

                TrayMenuItem {
                    id: format!("history:{}", entry.id),
                    label: format!("{} {}", state_label, label),
                    enabled: true,
                    kind: TrayMenuItemKind::Standard {
                        submenu: Some(vec![
                            TrayMenuItem {
                                id: format!("insert:{}", entry.id),
                                label: "Insert".to_owned(),
                                enabled: true,
                                kind: TrayMenuItemKind::Standard { submenu: None },
                            },
                            TrayMenuItem {
                                id: format!("copy:{}", entry.id),
                                label: "Copy".to_owned(),
                                enabled: !entry.text.is_empty(),
                                kind: TrayMenuItemKind::Standard { submenu: None },
                            },
                            TrayMenuItem {
                                id: format!("delete:{}", entry.id),
                                label: "Delete".to_owned(),
                                enabled: true,
                                kind: TrayMenuItemKind::Standard { submenu: None },
                            },
                        ]),
                    },
                }
            })
            .collect();

        items.push(TrayMenuItem {
            id: "history".to_owned(),
            label: "Recent History".to_owned(),
            enabled: !history_items.is_empty(),
            kind: TrayMenuItemKind::Standard {
                submenu: Some(history_items),
            },
        });

        items.push(TrayMenuItem {
            id: "sep2".to_owned(),
            label: String::new(),
            enabled: false,
            kind: TrayMenuItemKind::Separator,
        });

        // Quick toggle: a single click, so it is fine in a menu that closes on
        // every activation. Multi-choice settings are NOT — DBusMenu menus
        // close on each click and the protocol cannot keep them open, so those
        // all live in the Settings window instead ("settings:open" below).
        items.push(TrayMenuItem {
            id: "translation:enabled".to_owned(),
            label: "Translate while dictating".to_owned(),
            enabled: true,
            kind: TrayMenuItemKind::Checkable {
                checked: translation.enabled,
            },
        });
        // Only English works without an external translator (whisper's built-in
        // task). Any other target with no command fails every snippet at
        // dictation time — say so HERE, before the user dictates into silence.
        if translation.enabled
            && translation.output_language != "en"
            && translation.command.is_empty()
        {
            let language = language_name(&translation.output_language)
                .unwrap_or(translation.output_language.as_str());
            items.push(TrayMenuItem {
                id: "translation:unavailable".to_owned(),
                label: format!(
                    "⚠ {language} won't work: set translation.command in config.toml (only English works without it)"
                ),
                enabled: false,
                kind: TrayMenuItemKind::Standard { submenu: None },
            });
        }

        // Everything multi-choice (languages, timing, retention) opens in one
        // window that stays open while the user adjusts several things and
        // closes when they click elsewhere.
        items.push(TrayMenuItem {
            id: "settings:open".to_owned(),
            label: "Settings…".to_owned(),
            enabled: true,
            kind: TrayMenuItemKind::Standard { submenu: None },
        });

        items
    }
}

/// Radio options + selected index for the tray-history retention presets.
pub fn retention_radio(current_days: u32) -> (Vec<String>, usize) {
    let options = RETENTION_DAY_CHOICES
        .iter()
        .map(|days| {
            if *days == 1 {
                "1 day".to_owned()
            } else {
                format!("{days} days")
            }
        })
        .collect();
    let selected = RETENTION_DAY_CHOICES
        .iter()
        .position(|days| *days == current_days)
        .unwrap_or(0);
    (options, selected)
}

/// Radio options + selected index for the tray-history max-entry presets.
pub fn max_entries_radio(current: u32) -> (Vec<String>, usize) {
    let options = MAX_ENTRY_CHOICES
        .iter()
        .map(|count| count.to_string())
        .collect();
    let selected = MAX_ENTRY_CHOICES
        .iter()
        .position(|count| *count == current)
        .unwrap_or(0);
    (options, selected)
}

impl Default for MenuUseCase {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        mask_sensitive, max_entries_radio, retention_radio, training_retention_radio,
        truncate_for_menu, validate_max_entries, validate_retention_days,
        validate_training_retention_days, MenuUseCase, RecordingState, MENU_PREVIEW_MAX_CHARS,
    };
    use idiolect_common::ids::ImeSessionId;
    use idiolect_ports::storage::{HistoryEntry, HistoryState, TrayMenuItem, TrayMenuItemKind};

    /// Find a child item by id within a Standard submenu.
    fn child<'a>(items: &'a [TrayMenuItem], id: &str) -> &'a TrayMenuItem {
        items
            .iter()
            .find(|item| item.id == id)
            .unwrap_or_else(|| panic!("missing menu item {id}"))
    }

    fn entry(id: i64, text: &str, state: HistoryState) -> HistoryEntry {
        HistoryEntry {
            id,
            session_id: ImeSessionId::new(),
            text: text.to_owned(),
            state,
            created_at: "2026-01-01T00:00:00.000Z".to_owned(),
        }
    }

    #[test]
    fn truncate_is_char_safe_on_multibyte_text() {
        // 50 multibyte chars: byte-slicing at 40 would panic mid-codepoint.
        let text = "é".repeat(50);
        let truncated = truncate_for_menu(&text, MENU_PREVIEW_MAX_CHARS);
        assert_eq!(truncated.chars().count(), MENU_PREVIEW_MAX_CHARS + 1); // +1 ellipsis
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn truncate_leaves_short_text_untouched() {
        assert_eq!(truncate_for_menu("hello", MENU_PREVIEW_MAX_CHARS), "hello");
    }

    #[test]
    fn validation_accepts_allowed_choices_and_rejects_others() {
        assert!(validate_retention_days(7).is_ok());
        assert!(validate_retention_days(5).is_err());
        assert!(validate_max_entries(25).is_ok());
        assert!(validate_max_entries(11).is_err());
    }

    #[test]
    fn recording_controls_have_self_documenting_labels() {
        // Per-item tray tooltips are not supported by the DBusMenu protocol, so the
        // labels themselves must convey what Stop vs Cancel do: Stop transcribes and
        // inserts the text; Cancel throws the audio away.
        let menu =
            MenuUseCase::new().get_menu(RecordingState::Recording, &[], &Default::default());

        assert_eq!(child(&menu, "start_recording").label, "Start Recording");
        assert_eq!(child(&menu, "stop_recording").label, "Stop & Insert");
        assert_eq!(child(&menu, "cancel").label, "Cancel (discard)");
    }

    #[test]
    fn the_menu_is_actions_only_with_a_settings_window_entry() {
        // DBusMenu menus close on every click and cannot be kept open, so
        // multi-choice settings don't belong in them at all: the menu offers
        // actions, single-click toggles, and ONE "Settings…" entry that opens
        // the window where all multi-choice configuration lives.
        let history = vec![entry(1, "hello world", HistoryState::Committed)];
        let menu = MenuUseCase::new().get_menu(
            RecordingState::Idle,
            &history,
            &Default::default(),
        );

        let settings = child(&menu, "settings:open");
        assert_eq!(settings.label, "Settings…");
        assert!(settings.enabled);
        assert!(
            matches!(settings.kind, TrayMenuItemKind::Standard { submenu: None }),
            "opens the window; no inline submenu"
        );

        fn no_radio_groups(items: &[TrayMenuItem]) {
            for item in items {
                match &item.kind {
                    TrayMenuItemKind::RadioGroup { .. } => {
                        panic!("multi-choice {} must live in the Settings window", item.id)
                    }
                    TrayMenuItemKind::Standard { submenu: Some(sub) } => no_radio_groups(sub),
                    _ => {}
                }
            }
        }
        no_radio_groups(&menu);
    }

    #[test]
    fn retention_and_max_entry_radios_select_the_current_value() {
        // These builders feed the Settings window; index grammar must mirror
        // the daemon's `settings:retention:N` / `settings:max_entries:N` ids.
        let (options, selected) = retention_radio(30);
        assert_eq!(options, vec!["1 day", "7 days", "30 days"]);
        assert_eq!(selected, 2);
        let (options, selected) = max_entries_radio(25);
        assert_eq!(options, vec!["10", "25", "50"]);
        assert_eq!(selected, 1);
    }

    #[test]
    fn training_retention_radio_selects_the_matching_preset() {
        let (options, selected) = training_retention_radio(365);
        assert_eq!(options.len(), 8, "the eight presets, no custom marker");
        assert_eq!(options[selected], "1 year");
    }

    #[test]
    fn training_retention_radio_shows_a_custom_value_as_selected() {
        let (options, selected) = training_retention_radio(540);
        assert_eq!(options.len(), 9, "presets + the custom marker");
        assert_eq!(options[selected], "540 days (custom)");
    }

    #[test]
    fn training_retention_validation_allows_custom_and_zero_but_caps_absurd_values() {
        assert!(validate_training_retention_days(0).is_ok()); // keep forever
        assert!(validate_training_retention_days(540).is_ok()); // custom
        assert!(validate_training_retention_days(3650).is_ok()); // preset
        assert!(validate_training_retention_days(36_500).is_ok()); // cap
        assert!(validate_training_retention_days(36_501).is_err());
    }

    mod dictation_timing_choices {
        use super::super::{
            auto_stop_ms_for_index, max_phrase_ms_for_index, min_speech_ms_for_index,
            pause_ms_for_index, timing_radio, AUTO_STOP_CHOICES_MS, MAX_PHRASE_CHOICES_MS,
            MIN_SPEECH_CHOICES_MS, PAUSE_CHOICES_MS,
        };
        use idiolect_common::config::VadConfig;

        // These builders feed the Settings window; the option order must mirror
        // the daemon's `settings:pause:N` etc. action-id grammar, and the
        // defaults are marked in the options themselves.
        #[test]
        fn timing_radios_offer_all_four_knobs_with_defaults_selected() {
            let vad = VadConfig::default();

            let (options, selected) = timing_radio(&PAUSE_CHOICES_MS, vad.post_roll_ms, 700);
            assert_eq!(options.len(), PAUSE_CHOICES_MS.len());
            assert_eq!(options[selected], "0.7 s (default)");

            let (options, selected) = timing_radio(&MIN_SPEECH_CHOICES_MS, vad.min_speech_ms, 250);
            assert_eq!(options.len(), MIN_SPEECH_CHOICES_MS.len());
            assert_eq!(options[selected], "0.25 s (default)");

            let (options, selected) =
                timing_radio(&MAX_PHRASE_CHOICES_MS, vad.max_utterance_ms, 30_000);
            assert_eq!(options.len(), MAX_PHRASE_CHOICES_MS.len());
            assert_eq!(options[selected], "30 s (default)");

            let (options, selected) =
                timing_radio(&AUTO_STOP_CHOICES_MS, vad.auto_stop_silence_ms, 0);
            assert_eq!(options.len(), AUTO_STOP_CHOICES_MS.len());
            assert_eq!(options[selected], "Never (default)");
        }

        #[test]
        fn config_set_values_outside_the_presets_render_as_custom() {
            let (options, selected) = timing_radio(&PAUSE_CHOICES_MS, 900, 700);
            assert_eq!(options[selected], "900 ms (custom)");
        }

        #[test]
        fn menu_indices_map_back_to_milliseconds() {
            assert_eq!(pause_ms_for_index(0), Some(400));
            assert_eq!(pause_ms_for_index(1), Some(700));
            assert_eq!(pause_ms_for_index(PAUSE_CHOICES_MS.len()), None);
            assert_eq!(min_speech_ms_for_index(1), Some(250));
            assert_eq!(max_phrase_ms_for_index(2), Some(60_000));
            assert_eq!(auto_stop_ms_for_index(0), Some(0), "index 0 is Never");
            assert_eq!(auto_stop_ms_for_index(1), Some(5_000));
            assert_eq!(auto_stop_ms_for_index(AUTO_STOP_CHOICES_MS.len()), None);
        }
    }

    mod translation_menu {
        use super::super::{
            translation_input_language_for_index, translation_input_radio,
            translation_output_language_for_index, translation_output_radio, MenuUseCase,
            RecordingState,
        };
        use super::child;
        use idiolect_common::config::TranslationConfig;
        use idiolect_common::languages::LANGUAGES;
        use idiolect_ports::storage::TrayMenuItemKind;

        fn menu_with(
            translation: &TranslationConfig,
        ) -> Vec<idiolect_ports::storage::TrayMenuItem> {
            MenuUseCase::new().get_menu(RecordingState::Idle, &[], translation)
        }

        #[test]
        fn the_toggle_is_a_top_level_single_click() {
            // The toggle is the one translation control that works in a menu
            // that closes per click; the language pickers live in the Settings
            // window.
            let translation = TranslationConfig {
                enabled: true,
                ..TranslationConfig::default()
            };
            let menu = menu_with(&translation);
            match &child(&menu, "translation:enabled").kind {
                TrayMenuItemKind::Checkable { checked } => assert!(*checked),
                other => panic!("toggle should be checkable, got {other:?}"),
            }
            let menu = menu_with(&TranslationConfig::default());
            match &child(&menu, "translation:enabled").kind {
                TrayMenuItemKind::Checkable { checked } => assert!(!*checked),
                other => panic!("toggle should be checkable, got {other:?}"),
            }
        }

        #[test]
        fn language_radios_offer_every_language_both_ways() {
            // These builders feed the Settings window; option order must mirror
            // the daemon's translation:input:N / translation:output:N grammar.
            let (input_options, input_selected) = translation_input_radio("sv");
            assert_eq!(input_options.len(), LANGUAGES.len() + 1);
            assert_eq!(input_options[0], "Auto detect");
            assert_eq!(input_options[input_selected], "Swedish");
            let (_, auto_selected) = translation_input_radio("auto");
            assert_eq!(auto_selected, 0, "auto-detect is the first entry");

            let (output_options, output_selected) = translation_output_radio("ja");
            assert_eq!(output_options.len(), LANGUAGES.len());
            assert_eq!(output_options[output_selected], "Japanese");
            let (output_options, en_selected) = translation_output_radio("en");
            assert_eq!(output_options[en_selected], "English");
        }

        #[test]
        fn unworkable_output_language_warns_instead_of_failing_silently() {
            // Enabled + non-English target + no translator command means every
            // snippet will fail at dictation time. The menu must say so up
            // front — never let the user dictate into silence.
            let translation = TranslationConfig {
                enabled: true,
                input_language: "auto".to_owned(),
                output_language: "zh".to_owned(),
                command: String::new(),
            };
            let menu = menu_with(&translation);
            let warning = child(&menu, "translation:unavailable");
            assert!(!warning.enabled, "informational, not clickable");
            assert!(
                warning.label.contains("Chinese"),
                "names the broken language: {}",
                warning.label
            );
            assert!(
                warning.label.contains("translation.command"),
                "says what would fix it: {}",
                warning.label
            );
        }

        #[test]
        fn no_warning_when_the_configuration_actually_works() {
            let workable = [
                // English target: whisper translates in-engine, no command needed.
                TranslationConfig {
                    enabled: true,
                    input_language: "auto".to_owned(),
                    output_language: "en".to_owned(),
                    command: String::new(),
                },
                // Non-English target but a translator command is configured.
                TranslationConfig {
                    enabled: true,
                    input_language: "auto".to_owned(),
                    output_language: "zh".to_owned(),
                    command: "/usr/local/bin/translate".to_owned(),
                },
                // Translation off: nothing can fail, nothing to warn about.
                TranslationConfig {
                    enabled: false,
                    input_language: "auto".to_owned(),
                    output_language: "zh".to_owned(),
                    command: String::new(),
                },
            ];
            for translation in &workable {
                let menu = menu_with(translation);
                assert!(
                    menu.iter().all(|item| item.id != "translation:unavailable"),
                    "no warning for workable config {translation:?}"
                );
            }
        }

        #[test]
        fn menu_indices_map_back_to_language_codes() {
            // The activation callback resolves "translation:input:N" /
            // "translation:output:N" through these helpers — they must mirror
            // the option order exactly.
            assert_eq!(translation_input_language_for_index(0), Some("auto"));
            assert_eq!(
                translation_input_language_for_index(1),
                Some(LANGUAGES[0].0)
            );
            assert_eq!(
                translation_input_language_for_index(LANGUAGES.len()),
                Some(LANGUAGES[LANGUAGES.len() - 1].0)
            );
            assert_eq!(
                translation_input_language_for_index(LANGUAGES.len() + 1),
                None
            );

            assert_eq!(
                translation_output_language_for_index(0),
                Some(LANGUAGES[0].0)
            );
            assert_eq!(
                translation_output_language_for_index(LANGUAGES.len() - 1),
                Some(LANGUAGES[LANGUAGES.len() - 1].0)
            );
            assert_eq!(translation_output_language_for_index(LANGUAGES.len()), None);
        }
    }

    #[test]
    fn masking_redacts_secrets_emails_and_card_numbers() {
        assert_eq!(mask_sensitive("password: hunter2"), "password: [redacted]");
        assert_eq!(mask_sensitive("password=hunter2"), "password=[redacted]");
        assert_eq!(mask_sensitive("API_KEY=sk-abc123"), "API_KEY=[redacted]");
        assert_eq!(
            mask_sensitive("email me at me@example.com please"),
            "email me at [redacted] please"
        );
        // Contiguous and dash-separated card numbers are single tokens.
        assert_eq!(
            mask_sensitive("card 4111-1111-1111-1111 ok"),
            "card [redacted] ok"
        );
        assert_eq!(
            mask_sensitive("card 4111111111111111 ok"),
            "card [redacted] ok"
        );
    }

    #[test]
    fn masking_leaves_ordinary_text_intact() {
        assert_eq!(
            mask_sensitive("restart the Traefik service"),
            "restart the Traefik service"
        );
    }

    #[test]
    fn cancelled_entries_render_as_cancelled_label() {
        let history = vec![entry(1, "", HistoryState::Cancelled)];
        let menu =
            MenuUseCase::new().get_menu(RecordingState::Idle, &history, &Default::default());
        let history_item = menu
            .iter()
            .find(|item| item.id == "history")
            .expect("history present");
        let TrayMenuItemKind::Standard { submenu: Some(sub) } = &history_item.kind else {
            panic!("history should have a submenu");
        };
        assert!(sub[0].label.contains("[cancelled]"));
    }
}
