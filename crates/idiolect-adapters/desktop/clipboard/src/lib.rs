use arboard::Clipboard;
use idiolect_application::use_cases::history::ClipboardPort;

pub struct ArboardClipboard {
    /// `None` on a headless host (no X/Wayland display) where arboard can't
    /// open a clipboard. The daemon degrades to inert get/set rather than
    /// refusing to start — mirroring how the tray degrades to no icon.
    clipboard: Option<Clipboard>,
}

impl ArboardClipboard {
    pub fn new() -> Result<Self, ArboardClipboardError> {
        let clipboard = Clipboard::new().map_err(ArboardClipboardError::Arboard)?;
        Ok(Self {
            clipboard: Some(clipboard),
        })
    }

    /// A no-op clipboard for headless use: get returns empty, set is dropped.
    #[must_use]
    pub fn disabled() -> Self {
        Self { clipboard: None }
    }
}

impl Default for ArboardClipboard {
    fn default() -> Self {
        Self::new().expect("Failed to create clipboard")
    }
}

impl ArboardClipboard {
    /// Returns the current clipboard text, if any readable text is present.
    ///
    /// # Errors
    /// Returns [`ArboardClipboardError`] if the clipboard cannot be read.
    pub fn get_text(&mut self) -> Result<String, ArboardClipboardError> {
        match &mut self.clipboard {
            Some(clipboard) => clipboard.get_text().map_err(ArboardClipboardError::Arboard),
            None => Ok(String::new()),
        }
    }
}

impl ClipboardPort for ArboardClipboard {
    type Error = ArboardClipboardError;

    fn set_text(&mut self, text: &str) -> Result<(), Self::Error> {
        if let Some(clipboard) = &mut self.clipboard {
            clipboard
                .set_text(text)
                .map_err(ArboardClipboardError::Arboard)?;
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArboardClipboardError {
    #[error("arboard error: {0}")]
    Arboard(#[from] arboard::Error),
}
