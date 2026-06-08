use std::sync::mpsc;

use idiolect_ports::storage::{TrayIcon, TrayMenuItem, TrayMenuItemKind, TrayPort, TrayStatus};
use ksni::menu::{StandardItem, RadioGroup, CheckmarkItem, MenuItem, RadioItem, SubMenu};
use ksni::{Tray, ToolTip, blocking::TrayMethods};

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
        self.handle.update(|inner| inner.icon = icon_name.to_owned());
        Ok(())
    }

    fn set_tooltip(&mut self, tooltip: &str) -> Result<(), Self::Error> {
        self.handle.update(|inner| inner.tooltip = tooltip.to_owned());
        Ok(())
    }

    fn set_menu(&mut self, items: Vec<TrayMenuItem>) -> Result<(), Self::Error> {
        self.handle.update(|inner| inner.menu_items = items);
        Ok(())
    }

    fn set_status(&mut self, status: TrayStatus) -> Result<(), Self::Error> {
        let ksni_status = match status {
            TrayStatus::Active => ksni::Status::Active,
            TrayStatus::Passive => ksni::Status::Passive,
        };
        self.handle.update(|inner| inner.status = ksni_status);
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
        let handle = inner.spawn()?;
        Ok(Self { handle })
    }
}

impl InnerTray {
    fn map_menu_item(&self, item: &TrayMenuItem, sender: mpsc::Sender<TrayCallback>) -> MenuItem<InnerTray> {
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
        self.icon.clone()
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