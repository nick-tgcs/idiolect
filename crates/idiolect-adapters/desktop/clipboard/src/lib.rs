use arboard::Clipboard;
use idiolect_application::use_cases::history::ClipboardPort;

pub struct ArboardClipboard {
    clipboard: Clipboard,
}

impl ArboardClipboard {
    pub fn new() -> Result<Self, ArboardClipboardError> {
        let clipboard = Clipboard::new().map_err(ArboardClipboardError::Arboard)?;
        Ok(Self { clipboard })
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
        self.clipboard
            .get_text()
            .map_err(ArboardClipboardError::Arboard)
    }
}

impl ClipboardPort for ArboardClipboard {
    type Error = ArboardClipboardError;

    fn set_text(&mut self, text: &str) -> Result<(), Self::Error> {
        self.clipboard
            .set_text(text)
            .map_err(ArboardClipboardError::Arboard)?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ArboardClipboardError {
    #[error("arboard error: {0}")]
    Arboard(#[from] arboard::Error),
}
