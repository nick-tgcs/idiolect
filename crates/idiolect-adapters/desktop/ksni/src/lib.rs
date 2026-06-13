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
    /// Whether the ksni service loop is running and the tray will (re-)register
    /// with `StatusNotifierWatcher` when it appears. `false` means the daemon is
    /// running headless (no icon, no reconnect).
    pub fn is_live(&self) -> bool {
        self.handle.is_some()
    }

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
        // The daemon's systemd unit fires as soon as graphical-session.target is
        // reached, but GNOME Shell (which registers org.kde.StatusNotifierWatcher)
        // can take ~800 ms longer. assume_sni_available(true) tells ksni to treat
        // a missing watcher as transient: it starts its reconnect loop instead of
        // returning Err, so the icon appears once GNOME Shell catches up. Without
        // this, the spawn failed with ServiceUnknown, the handle became None, and
        // no icon ever appeared for the entire daemon session.
        //
        // If the session bus is absent entirely (headless server, CI without
        // dbus-run-session), the session() connection builder itself fails before
        // we reach the watcher check, and we still degrade gracefully to None.
        let handle = match inner.assume_sni_available(true).spawn() {
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
                let group = MenuItem::RadioGroup(RadioGroup {
                    selected: *selected,
                    options: radio_items,
                    select: Box::new(move |_this: &mut InnerTray, idx: usize| {
                        let _ = sender.send(TrayCallback::Activate(format!("{group_id}:{idx}")));
                    }),
                });
                if item.label.is_empty() {
                    return group;
                }
                // DBusMenu radio groups carry no caption, so a bare group renders
                // as an anonymous run of options — adjacent groups blur into one
                // unreadable list. Preserve the label as a submenu title, with
                // the current choice appended so "what is it set to?" is
                // answered without expanding.
                let title = match options.get(*selected) {
                    Some(current) => format!("{} — {}", item.label, current),
                    None => item.label.clone(),
                };
                MenuItem::SubMenu(SubMenu {
                    label: title,
                    enabled: item.enabled,
                    submenu: vec![group],
                    ..Default::default()
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
    fn tray_is_not_live_when_handle_is_none() {
        // is_live() is the observable proxy for whether the ksni reconnect loop is
        // running. handle: None means either IDIOLECT_DISABLE_TRAY was set or the
        // session bus was absent; either way the tray is not live.
        let tray = KsniTray { handle: None };
        assert!(!tray.is_live());
    }

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

    fn inner_tray() -> (InnerTray, mpsc::Receiver<TrayCallback>) {
        let (sender, receiver) = mpsc::channel();
        (
            InnerTray {
                icon: TrayIcon::Idle,
                tooltip: String::new(),
                status: ksni::Status::Passive,
                menu_items: Vec::new(),
                sender,
            },
            receiver,
        )
    }

    fn radio_group_item(label: &str, options: &[&str], selected: usize) -> TrayMenuItem {
        TrayMenuItem {
            id: "settings:pause".to_owned(),
            label: label.to_owned(),
            enabled: true,
            kind: TrayMenuItemKind::RadioGroup {
                options: options.iter().map(|o| (*o).to_owned()).collect(),
                selected,
            },
        }
    }

    #[test]
    fn a_labelled_radio_group_keeps_its_label_and_shows_the_current_value() {
        // DBusMenu radio groups have no caption: rendered bare, four groups in a
        // row become one anonymous run of numbers ("0.4 s, 0.7 s, 0.15 s, …") —
        // unusable. The label must survive as a submenu title, with the current
        // choice visible at a glance without expanding.
        let (mut tray, receiver) = inner_tray();
        let item = radio_group_item(
            "Send a phrase after a pause of",
            &["0.4 s", "0.7 s (default)", "1.2 s"],
            1,
        );
        let sender = tray.sender.clone();
        let mapped = tray.map_menu_item(&item, sender);

        let MenuItem::SubMenu(submenu) = mapped else {
            panic!("labelled radio group must render as a titled submenu");
        };
        assert_eq!(
            submenu.label, "Send a phrase after a pause of — 0.7 s (default)",
            "submenu title = group label + current choice"
        );
        assert_eq!(submenu.submenu.len(), 1, "the choices live inside");
        let MenuItem::RadioGroup(group) = &submenu.submenu[0] else {
            panic!("the submenu must contain the actual radio group");
        };
        assert_eq!(group.selected, 1);
        assert_eq!(group.options.len(), 3);

        // Selecting still emits the same "<group-id>:<index>" action id, so the
        // daemon's tray-action parsing is untouched.
        let select = match &submenu.submenu[0] {
            MenuItem::RadioGroup(group) => &group.select,
            _ => unreachable!(),
        };
        select(&mut tray, 2);
        match receiver.try_recv() {
            Ok(TrayCallback::Activate(id)) => assert_eq!(id, "settings:pause:2"),
            other => panic!("expected Activate(settings:pause:2), got {other:?}"),
        }
    }

    #[test]
    fn an_unlabelled_radio_group_stays_inline() {
        // No label means nothing to preserve — don't force an extra menu level.
        let (tray, _receiver) = inner_tray();
        let item = radio_group_item("", &["a", "b"], 0);
        let sender = tray.sender.clone();
        let mapped = tray.map_menu_item(&item, sender);
        assert!(
            matches!(mapped, MenuItem::RadioGroup(_)),
            "unlabelled groups render bare"
        );
    }

    #[test]
    fn an_out_of_range_selection_still_renders_with_the_plain_label() {
        // Defensive: a bad index must not panic or invent a value.
        let (tray, _receiver) = inner_tray();
        let item = radio_group_item("Show last", &["1 day"], 9);
        let sender = tray.sender.clone();
        let mapped = tray.map_menu_item(&item, sender);
        let MenuItem::SubMenu(submenu) = mapped else {
            panic!("labelled radio group must render as a titled submenu");
        };
        assert_eq!(submenu.label, "Show last");
    }
}
