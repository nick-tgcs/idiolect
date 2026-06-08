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

#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum MenuUseCaseError {
    #[error("invalid retention days: {0} (must be 1, 7, or 30)")]
    InvalidRetentionDays(u32),
    #[error("invalid max entries: {0} (must be 10, 25, or 50)")]
    InvalidMaxEntries(u32),
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
                    label: "Stop Recording".to_owned(),
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
                    label: "Stop Recording".to_owned(),
                    enabled: true,
                    kind: TrayMenuItemKind::Standard { submenu: None },
                });
            }
        }

        // Cancel
        items.push(TrayMenuItem {
            id: "cancel".to_owned(),
            label: "Cancel".to_owned(),
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

        // Settings submenu
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

        items.push(TrayMenuItem {
            id: "settings".to_owned(),
            label: "Settings".to_owned(),
            enabled: true,
            kind: TrayMenuItemKind::Standard {
                submenu: Some(vec![
                    TrayMenuItem {
                        id: "settings:retention".to_owned(),
                        label: "Retention".to_owned(),
                        enabled: true,
                        kind: TrayMenuItemKind::RadioGroup {
                            options: retention_options,
                            selected: retention_selected,
                        },
                    },
                    TrayMenuItem {
                        id: "settings:max_entries".to_owned(),
                        label: "Max Entries".to_owned(),
                        enabled: true,
                        kind: TrayMenuItemKind::RadioGroup {
                            options: max_entries_options,
                            selected: max_entries_selected,
                        },
                    },
                ]),
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
        MenuUseCase, RecordingState, MENU_PREVIEW_MAX_CHARS,
    };
    use idiolect_common::config::HistoryConfig;
    use idiolect_common::ids::ImeSessionId;
    use idiolect_ports::storage::{HistoryEntry, HistoryState, TrayMenuItemKind};

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
    fn menu_reflects_retention_selection_and_history() {
        let config = HistoryConfig {
            retention_days: 30,
            max_entries: 25,
            ..HistoryConfig::default()
        };
        let history = vec![entry(1, "hello world", HistoryState::Committed)];
        let menu = MenuUseCase::new().get_menu(RecordingState::Idle, &history, &config);

        let settings = menu
            .iter()
            .find(|item| item.id == "settings")
            .expect("settings present");
        let TrayMenuItemKind::Standard { submenu: Some(sub) } = &settings.kind else {
            panic!("settings should have a submenu");
        };
        let retention = sub
            .iter()
            .find(|item| item.id == "settings:retention")
            .expect("retention present");
        let TrayMenuItemKind::RadioGroup { selected, .. } = &retention.kind else {
            panic!("retention should be a radio group");
        };
        assert_eq!(*selected, 2); // 30 days -> index 2
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