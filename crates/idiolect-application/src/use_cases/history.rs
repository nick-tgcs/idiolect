use idiolect_common::ids::ImeSessionId;
use idiolect_ports::input_method::InputMethodPort;
use idiolect_ports::storage::{HistoryEntry, HistoryState, MetadataStorePort};

#[derive(Debug, Eq, PartialEq)]
pub enum HistoryUseCaseError<I, S, C> {
    Input(I),
    Storage(S),
    Clipboard(C),
    NotFound,
}

pub struct HistoryUseCase<I, S, C> {
    input: I,
    storage: S,
    clipboard: C,
}

impl<I, S, C> HistoryUseCase<I, S, C>
where
    I: InputMethodPort,
    S: MetadataStorePort,
    C: ClipboardPort,
{
    #[must_use]
    pub fn new(input: I, storage: S, clipboard: C) -> Self {
        Self {
            input,
            storage,
            clipboard,
        }
    }

    pub fn get_recent(&self, limit: u32) -> Result<Vec<HistoryEntry>, HistoryUseCaseError<I::Error, S::Error, C::Error>> {
        self.storage
            .recent_history(limit)
            .map_err(HistoryUseCaseError::Storage)
    }

    pub fn reinsert(
        &mut self,
        id: i64,
    ) -> Result<(), HistoryUseCaseError<I::Error, S::Error, C::Error>> {
        let entries = self.storage.recent_history(100).map_err(HistoryUseCaseError::Storage)?;
        let entry = entries
            .into_iter()
            .find(|e| e.id == id)
            .ok_or(HistoryUseCaseError::NotFound)?;
        
        self.input
            .commit_text(entry.session_id, &entry.text)
            .map_err(HistoryUseCaseError::Input)
    }

    pub fn copy(
        &mut self,
        id: i64,
    ) -> Result<(), HistoryUseCaseError<I::Error, S::Error, C::Error>> {
        let entries = self.storage.recent_history(100).map_err(HistoryUseCaseError::Storage)?;
        let entry = entries
            .into_iter()
            .find(|e| e.id == id)
            .ok_or(HistoryUseCaseError::NotFound)?;
        
        self.clipboard
            .set_text(&entry.text)
            .map_err(HistoryUseCaseError::Clipboard)
    }

    pub fn delete(
        &mut self,
        id: i64,
    ) -> Result<(), HistoryUseCaseError<I::Error, S::Error, C::Error>> {
        self.storage
            .delete_history_entry(id)
            .map_err(HistoryUseCaseError::Storage)
    }
}

pub trait ClipboardPort {
    type Error;
    fn set_text(&mut self, text: &str) -> Result<(), Self::Error>;
}