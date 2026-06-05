use idiolect_common::config::HistoryConfig;
use idiolect_ports::storage::{HistoryEntry, HistoryState, TrayIcon, TrayMenuItem, TrayMenuItemKind, TrayStatus};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RecordingState {
    Idle,
    Recording,
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
            .enumerate()
            .map(|(idx, entry)| {
                let label = if entry.text.is_empty() {
                    "[cancelled]".to_owned()
                } else if entry.text.len() > 40 {
                    format!("{}…", &entry.text[..40])
                } else {
                    entry.text.clone()
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
        let retention_options = vec!["1 day", "7 days", "30 days"];
        let retention_selected = match config.retention_days {
            1 => 0,
            7 => 1,
            30 => 2,
            _ => 0,
        };

        let max_entries_options = vec!["10", "25", "50"];
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