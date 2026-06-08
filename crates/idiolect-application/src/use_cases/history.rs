use idiolect_ports::input_method::InputMethodPort;
use idiolect_ports::storage::{HistoryEntry, MetadataStorePort};

#[derive(Debug, Eq, PartialEq)]
pub enum HistoryUseCaseError<I, S, C> {
    Input(I),
    Storage(S),
    Clipboard(C),
    NotFound,
}

/// Result alias for [`HistoryUseCase`] operations, keyed by the three port error
/// types, to keep method signatures readable.
pub type HistoryResult<T, I, S, C> = Result<T, HistoryUseCaseError<I, S, C>>;

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

    pub fn get_recent(
        &self,
        limit: u32,
    ) -> HistoryResult<Vec<HistoryEntry>, I::Error, S::Error, C::Error> {
        self.storage
            .recent_history(limit)
            .map_err(HistoryUseCaseError::Storage)
    }

    pub fn reinsert(&mut self, id: i64) -> HistoryResult<(), I::Error, S::Error, C::Error> {
        let entry = self
            .storage
            .get_history_entry(id)
            .map_err(HistoryUseCaseError::Storage)?
            .ok_or(HistoryUseCaseError::NotFound)?;

        self.input
            .commit_text(entry.session_id, &entry.text)
            .map_err(HistoryUseCaseError::Input)
    }

    pub fn copy(&mut self, id: i64) -> HistoryResult<(), I::Error, S::Error, C::Error> {
        let entry = self
            .storage
            .get_history_entry(id)
            .map_err(HistoryUseCaseError::Storage)?
            .ok_or(HistoryUseCaseError::NotFound)?;

        self.clipboard
            .set_text(&entry.text)
            .map_err(HistoryUseCaseError::Clipboard)
    }

    pub fn delete(&mut self, id: i64) -> HistoryResult<(), I::Error, S::Error, C::Error> {
        self.storage
            .delete_history_entry(id)
            .map_err(HistoryUseCaseError::Storage)
    }

    pub fn prune(
        &mut self,
        retention_days: u32,
    ) -> HistoryResult<u64, I::Error, S::Error, C::Error> {
        self.storage
            .prune_history(retention_days)
            .map_err(HistoryUseCaseError::Storage)
    }
}

pub trait ClipboardPort {
    type Error;
    fn set_text(&mut self, text: &str) -> Result<(), Self::Error>;
}