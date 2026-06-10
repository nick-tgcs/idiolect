use std::sync::mpsc;

use idiolect_ports::storage::{TrayIcon, TrayMenuItem, TrayMenuItemKind, TrayPort, TrayStatus};
use ksni::menu::{CheckmarkItem, MenuItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{blocking::TrayMethods, ToolTip, Tray};

mod icons;

/// Commands the tray adapter sends back to the daemon's main thread.
#[derive(Debug, Clone)]
pub enum TrayCallback {
    /// Carry an opaque string action id (e.g. "insert:42", "copy:42", "delete:42", "settings:retention:7").
    Activate(String),
}

pub struct KsniTray {
    /// `None` when no tray host (`StatusNotifierWatcher`) was available at startup —
    /// e.g. a headless box, or the daemon started before the desktop shell at login.
    /// The tray then degrades to a no-op so dictation still works without an icon.
    handle: Option<ksni::blocking::Handle<InnerTray>>,
}

struct InnerTray {
    icon: TrayIcon,
    tooltip: String,
    status: ksni::Status,
    menu_items: Vec<TrayMenuItem>,
    /// Channel used by ksni callbacks to notify the daemon main thread.
    sender: mpsc::Sender<TrayCallback>,
}

impl TrayPort for KsniTray {
    type Error = KsniTrayError;

    fn set_icon(&mut self, icon: TrayIcon) -> Result<(), Self::Error> {
        // Custom line-art microphone rendered as a pixmap (see `icons`), so the
        // tray looks the same in any theme instead of a generic glyph.
        if let Some(handle) = &self.handle {
            handle.update(move |inner| inner.icon = icon);
        }
        Ok(())
    }

    fn set_tooltip(&mut self, tooltip: &str) -> Result<(), Self::Error> {
        if let Some(handle) = &self.handle {
            handle.update(|inner| inner.tooltip = tooltip.to_owned());
        }
        Ok(())
    }

    fn set_menu(&mut self, items: Vec<TrayMenuItem>) -> Result<(), Self::Error> {
        if let Some(handle) = &self.handle {
            handle.update(|inner| inner.menu_items = items);
        }
        Ok(())
    }

    fn set_status(&mut self, status: TrayStatus) -> Result<(), Self::Error> {
        let ksni_status = match status {
            TrayStatus::Active => ksni::Status::Active,
            TrayStatus::Passive => ksni::Status::Passive,
        };
        if let Some(handle) = &self.handle {
            handle.update(|inner| inner.status = ksni_status);
        }
        Ok(())
    }
}

impl KsniTray {
    pub fn new(sender: mpsc::Sender<TrayCallback>) -> Result<Self, KsniTrayError> {
        // Escape hatch for headless/in-process use (notably integration tests that
        // run several daemons inside one process): registering a StatusNotifierItem
        // means a D-Bus round-trip on a pid-keyed bus name, which collides and
        // destabilises sibling in-process daemons on a bare session bus. When set,
        // run without a tray — the daemon already degrades gracefully to no icon.
        if std::env::var_os("IDIOLECT_DISABLE_TRAY").is_some() {
            return Ok(Self { handle: None });
        }
        let inner = InnerTray {
            icon: TrayIcon::Idle,
            tooltip: "Idiolect — Ready".to_owned(),
            status: ksni::Status::Passive,
            menu_items: Vec::new(),
            sender,
        };
        // Degrade gracefully when there is no tray host: a missing
        // `StatusNotifierWatcher` (headless, or the desktop shell not up yet at
        // login) must not crash the daemon — dictation works without an icon, and
        // the daemon stays up instead of the autostart unit giving up on it.
        let handle = match inner.spawn() {
            Ok(handle) => Some(handle),
            Err(error) => {
                eprintln!("idiolect: tray unavailable, running without a tray icon: {error}");
                None
            }
        };
        Ok(Self { handle })
    }
}

impl InnerTray {
    fn map_menu_item(
        &self,
        item: &TrayMenuItem,
        sender: mpsc::Sender<TrayCallback>,
    ) -> MenuItem<InnerTray> {
        match &item.kind {
            TrayMenuItemKind::Standard {
                submenu: Some(sub_items),
            } => {
                let submenu = sub_items
                    .iter()
                    .map(|sub| self.map_menu_item(sub, sender.clone()))
                    .collect();
                MenuItem::SubMenu(SubMenu {
                    label: item.label.clone(),
                    enabled: item.enabled,
                    submenu,
                    ..Default::default()
                })
            }
            TrayMenuItemKind::Standard { submenu: None } => {
                let action_id = item.id.clone();
                let sender = sender.clone();
                MenuItem::Standard(StandardItem {
                    label: item.label.clone(),
                    enabled: item.enabled,
                    activate: Box::new(move |_this: &mut InnerTray| {
                        let _ = sender.send(TrayCallback::Activate(action_id.clone()));
                    }),
                    ..Default::default()
                })
            }
            TrayMenuItemKind::Checkable { checked } => {
                let action_id = item.id.clone();
                let sender = sender.clone();
                MenuItem::Checkmark(CheckmarkItem {
                    label: item.label.clone(),
                    enabled: item.enabled,
                    checked: *checked,
                    activate: Box::new(move |_this: &mut InnerTray| {
                        let _ = sender.send(TrayCallback::Activate(action_id.clone()));
                    }),
                    ..Default::default()
                })
            }
            TrayMenuItemKind::RadioGroup { options, selected } => {
                let radio_items = options
                    .iter()
                    .map(|option| RadioItem {
                        label: option.clone(),
                        enabled: true,
                        ..Default::default()
                    })
                    .collect();
                // Emit "<group-id>:<index>" (e.g. "settings:retention:1") so the
                // daemon can identify both which setting changed and the chosen
                // option index.
                let group_id = item.id.clone();
                let sender = sender.clone();
                MenuItem::RadioGroup(RadioGroup {
                    selected: *selected,
                    options: radio_items,
                    select: Box::new(move |_this: &mut InnerTray, idx: usize| {
                        let _ = sender.send(TrayCallback::Activate(format!("{group_id}:{idx}")));
                    }),
                })
            }
            TrayMenuItemKind::Separator => MenuItem::Separator,
        }
    }
}

impl Tray for InnerTray {
    fn id(&self) -> String {
        "idiolect".to_owned()
    }

    fn icon_name(&self) -> String {
        // Empty so the host uses our custom `icon_pixmap` below.
        String::new()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        vec![icons::render(self.icon)]
    }

    fn title(&self) -> String {
        "Idiolect".to_owned()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            icon_name: String::new(),
            title: "Idiolect".to_owned(),
            description: self.tooltip.clone(),
            icon_pixmap: Vec::new(),
        }
    }

    fn status(&self) -> ksni::Status {
        self.status
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let sender = self.sender.clone();
        self.menu_items
            .iter()
            .map(|item| self.map_menu_item(item, sender.clone()))
            .collect()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum KsniTrayError {
    #[error("ksni error: {0}")]
    Ksni(#[from] ksni::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degraded_tray_without_a_host_is_a_safe_noop() {
        // When no tray host is available the tray has no handle; every operation
        // must succeed as a no-op so the daemon runs headless instead of crashing.
        let mut tray = KsniTray { handle: None };

        assert!(tray.set_icon(TrayIcon::Recording).is_ok());
        assert!(tray.set_tooltip("Idiolect — Recording…").is_ok());
        assert!(tray.set_menu(Vec::new()).is_ok());
        assert!(tray.set_status(TrayStatus::Active).is_ok());
    }
}
