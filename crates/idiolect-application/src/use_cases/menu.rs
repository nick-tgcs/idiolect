use idiolect_common::config::HistoryConfig;
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
fn training_retention_radio(current_days: u32) -> (Vec<String>, usize) {
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
        config: &HistoryConfig,
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

        // Settings submenu: two distinct retention concepts, kept visually
        // separate — the short tray-history list vs the long training-data corpus.
        let retention_options = vec!["1 day".to_owned(), "7 days".to_owned(), "30 days".to_owned()];
        let retention_selected = match config.retention_days {
            1 => 0,
            7 => 1,
            30 => 2,
            _ => 0,
        };

        let max_entries_options = vec!["10".to_owned(), "25".to_owned(), "50".to_owned()];
        let max_entries_selected = match config.max_entries {
            10 => 0,
            25 => 1,
            50 => 2,
            _ => 0,
        };

        // "Tray history": how long / how many recent dictations the menu shows.
        let tray_history = TrayMenuItem {
            id: "settings:tray_history".to_owned(),
            label: "Tray history".to_owned(),
            enabled: true,
            kind: TrayMenuItemKind::Standard {
                submenu: Some(vec![
                    TrayMenuItem {
                        id: "settings:retention".to_owned(),
                        label: "Show last".to_owned(),
                        enabled: true,
                        kind: TrayMenuItemKind::RadioGroup {
                            options: retention_options,
                            selected: retention_selected,
                        },
                    },
                    TrayMenuItem {
                        id: "settings:max_entries".to_owned(),
                        label: "Max items".to_owned(),
                        enabled: true,
                        kind: TrayMenuItemKind::RadioGroup {
                            options: max_entries_options,
                            selected: max_entries_selected,
                        },
                    },
                ]),
            },
        };

        // "Training data kept for": how long captured audio + transcripts are
        // retained for learning. Presets plus a free-form "Custom…" entry.
        let (training_options, training_selected) =
            training_retention_radio(config.training_retention_days);
        let training_data = TrayMenuItem {
            id: "settings:training_data".to_owned(),
            label: "Training data kept for".to_owned(),
            enabled: true,
            kind: TrayMenuItemKind::Standard {
                submenu: Some(vec![
                    TrayMenuItem {
                        id: "settings:training_retention".to_owned(),
                        label: "Keep for".to_owned(),
                        enabled: true,
                        kind: TrayMenuItemKind::RadioGroup {
                            options: training_options,
                            selected: training_selected,
                        },
                    },
                    TrayMenuItem {
                        id: "settings:training_retention_custom".to_owned(),
                        label: "Custom…".to_owned(),
                        enabled: true,
                        kind: TrayMenuItemKind::Standard { submenu: None },
                    },
                ]),
            },
        };

        items.push(TrayMenuItem {
            id: "settings".to_owned(),
            label: "Settings".to_owned(),
            enabled: true,
            kind: TrayMenuItemKind::Standard {
                submenu: Some(vec![tray_history, training_data]),
            },
        });

        items
    }
}

impl Default for MenuUseCase {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        mask_sensitive, truncate_for_menu, validate_max_entries, validate_retention_days,
        validate_training_retention_days, MenuUseCase, RecordingState, MENU_PREVIEW_MAX_CHARS,
    };
    use idiolect_common::config::HistoryConfig;
    use idiolect_common::ids::ImeSessionId;
    use idiolect_ports::storage::{HistoryEntry, HistoryState, TrayMenuItem, TrayMenuItemKind};

    /// Find a child item by id within a Standard submenu.
    fn child<'a>(items: &'a [TrayMenuItem], id: &str) -> &'a TrayMenuItem {
        items
            .iter()
            .find(|item| item.id == id)
            .unwrap_or_else(|| panic!("missing menu item {id}"))
    }

    fn submenu(item: &TrayMenuItem) -> &[TrayMenuItem] {
        match &item.kind {
            TrayMenuItemKind::Standard { submenu: Some(sub) } => sub,
            _ => panic!("{} should have a submenu", item.id),
        }
    }

    fn radio_selected(item: &TrayMenuItem) -> (usize, &[String]) {
        match &item.kind {
            TrayMenuItemKind::RadioGroup { selected, options } => (*selected, options),
            _ => panic!("{} should be a radio group", item.id),
        }
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
        let config = HistoryConfig::default();
        let menu = MenuUseCase::new().get_menu(RecordingState::Recording, &[], &config);

        assert_eq!(child(&menu, "start_recording").label, "Start Recording");
        assert_eq!(child(&menu, "stop_recording").label, "Stop & Insert");
        assert_eq!(child(&menu, "cancel").label, "Cancel (discard)");
    }

    #[test]
    fn menu_reflects_retention_selection_and_history() {
        let config = HistoryConfig {
            retention_days: 30,
            max_entries: 25,
            ..HistoryConfig::default()
        };
        let history = vec![entry(1, "hello world", HistoryState::Committed)];
        let menu = MenuUseCase::new().get_menu(RecordingState::Idle, &history, &config);

        let settings = submenu(child(&menu, "settings"));
        // Tray-history retention now lives under the "Tray history" group.
        let tray_history = submenu(child(settings, "settings:tray_history"));
        let (selected, _) = radio_selected(child(tray_history, "settings:retention"));
        assert_eq!(selected, 2, "30 days -> index 2");
        let (max_selected, _) = radio_selected(child(tray_history, "settings:max_entries"));
        assert_eq!(max_selected, 1, "25 entries -> index 1");
    }

    #[test]
    fn training_retention_radio_selects_the_matching_preset() {
        let config = HistoryConfig {
            training_retention_days: 365, // "1 year" -> index 4
            ..HistoryConfig::default()
        };
        let menu = MenuUseCase::new().get_menu(RecordingState::Idle, &[], &config);

        let settings = submenu(child(&menu, "settings"));
        let training = submenu(child(settings, "settings:training_data"));
        let (selected, options) = radio_selected(child(training, "settings:training_retention"));
        assert_eq!(options.len(), 8, "the eight presets, no custom marker");
        assert_eq!(options[selected], "1 year");
        // The free-form custom entry point is present.
        assert_eq!(child(training, "settings:training_retention_custom").label, "Custom…");
    }

    #[test]
    fn training_retention_radio_shows_a_custom_value_as_selected() {
        let config = HistoryConfig {
            training_retention_days: 540, // not a preset
            ..HistoryConfig::default()
        };
        let menu = MenuUseCase::new().get_menu(RecordingState::Idle, &[], &config);

        let settings = submenu(child(&menu, "settings"));
        let training = submenu(child(settings, "settings:training_data"));
        let (selected, options) = radio_selected(child(training, "settings:training_retention"));
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
        assert_eq!(mask_sensitive("card 4111-1111-1111-1111 ok"), "card [redacted] ok");
        assert_eq!(mask_sensitive("card 4111111111111111 ok"), "card [redacted] ok");
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
        let config = HistoryConfig::default();
        let history = vec![entry(1, "", HistoryState::Cancelled)];
        let menu = MenuUseCase::new().get_menu(RecordingState::Idle, &history, &config);
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