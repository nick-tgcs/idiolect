use std::sync::mpsc;

use idiolect_ports::storage::{HistoryEntry, HistoryState, TrayIcon, TrayMenuItem, TrayMenuItemKind, TrayPort, TrayStatus};
use ksni::menu::{StandardItem, SubMenu, RadioGroup, CheckmarkItem, SeparatorItem};
use ksni::{Tray, TrayMethods};

/// Commands the tray adapter sends back to the daemon's main thread.
#[derive(Debug, Clone)]
pub enum TrayCallback {
    /// Carry an opaque string action id (e.g. "insert:42", "copy:42", "delete:42", "settings:retention:7").
    Activate(String),
}

pub struct KsniTray {
    handle: ksni::blocking::Handle<InnerTray>,
}

struct InnerTray {
    icon: String,
    tooltip: String,
    status: ksni::Status,
    menu_items: Vec<TrayMenuItem>,
    /// Channel used by ksni callbacks to notify the daemon main thread.
    sender: mpsc::Sender<TrayCallback>,
}

impl TrayPort for KsniTray {
    type Error = KsniTrayError;

    fn set_icon(&mut self, icon: TrayIcon) -> Result<(), Self::Error> {
        let icon_name = match icon {
            TrayIcon::Idle => "idiolect-idle",
            TrayIcon::Recording => "idiolect-recording",
            TrayIcon::Error => "idiolect-error",
        };
        self.handle.update(|inner| inner.icon = icon_name.to_owned())?;
        Ok(())
    }

    fn set_tooltip(&mut self, tooltip: &str) -> Result<(), Self::Error> {
        self.handle.update(|inner| inner.tooltip = tooltip.to_owned())?;
        Ok(())
    }

    fn set_menu(&mut self, items: Vec<TrayMenuItem>) -> Result<(), Self::Error> {
        self.handle.update(|inner| inner.menu_items = items)?;
        Ok(())
    }

    fn set_status(&mut self, status: TrayStatus) -> Result<(), Self::Error> {
        let ksni_status = match status {
            TrayStatus::Active => ksni::Status::Active,
            TrayStatus::Passive => ksni::Status::Passive,
        };
        self.handle.update(|inner| inner.status = ksni_status)?;
        Ok(())
    }
}

impl KsniTray {
    pub fn new(sender: mpsc::Sender<TrayCallback>) -> Result<Self, KsniTrayError> {
        let inner = InnerTray {
            icon: "idiolect-idle".to_owned(),
            tooltip: "Idiolect — Ready".to_owned(),
            status: ksni::Status::Passive,
            menu_items: Vec::new(),
            sender,
        };
        let handle = ksni::blocking::Tray::new(inner)?;
        Ok(Self { handle })
    }
}

impl InnerTray {
    fn map_menu_item(&self, item: &TrayMenuItem, sender: mpsc::Sender<TrayCallback>) -> Box<dyn ksni::menu::MenuItem> {
        match &item.kind {
            TrayMenuItemKind::Standard { submenu } => {
                let mut standard = StandardItem::new(&item.label);
                standard.set_enabled(item.enabled);
                if let Some(sub_items) = submenu {
                    let submenu_items: Vec<Box<dyn ksni::menu::MenuItem>> = sub_items
                        .iter()
                        .map(|sub| self.map_menu_item(sub, sender.clone()))
                        .collect();
                    standard.set_submenu(SubMenu::new(submenu_items));
                } else {
                    let action_id = item.id.clone();
                    let s = sender.clone();
                    standard.on_activate(move || {
                        let _ = s.send(TrayCallback::Activate(action_id.clone()));
                    });
                }
                Box::new(standard)
            }
            TrayMenuItemKind::Checkable { checked } => {
                let mut checkable = CheckmarkItem::new(&item.label);
                checkable.set_enabled(item.enabled);
                checkable.set_checked(*checked);
                let action_id = item.id.clone();
                let s = sender.clone();
                checkable.on_activate(move || {
                    let _ = s.send(TrayCallback::Activate(action_id.clone()));
                });
                Box::new(checkable)
            }
            TrayMenuItemKind::RadioGroup { options, selected } => {
                let mut radio_group = RadioGroup::new();
                for (idx, option) in options.iter().enumerate() {
                    let mut item = ksni::menu::RadioItem::new(option);
                    item.set_checked(idx == *selected);
                    let action_id = format!("{}:{}", item.id, idx);
                    let s = sender.clone();
                    item.on_activate(move || {
                        let _ = s.send(TrayCallback::Activate(action_id.clone()));
                    });
                    radio_group.add(item);
                }
                Box::new(radio_group)
            }
            TrayMenuItemKind::Separator => {
                Box::new(SeparatorItem::new())
            }
        }
    }
}

impl Tray for InnerTray {
    fn icon_name(&self) -> String {
        self.icon.clone()
    }

    fn title(&self) -> String {
        "Idiolect".to_owned()
    }

    fn tool_tip(&self) -> String {
        self.tooltip.clone()
    }

    fn status(&self) -> ksni::Status {
        self.status
    }

    fn menu(&self) -> Vec<Box<dyn ksni::menu::MenuItem>> {
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
    #[error("update error: {0}")]
    Update(#[from] ksni::blocking::UpdateError),
}