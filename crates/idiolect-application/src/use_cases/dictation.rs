use std::collections::HashSet;

use idiolect_common::ids::ImeSessionId;
use idiolect_ports::input_method::InputMethodPort;
use idiolect_ports::storage::MetadataStorePort;

#[derive(Debug, Eq, PartialEq)]
pub enum DictationUseCaseError<I, S> {
    Input(I),
    Storage(S),
}

#[derive(Debug)]
pub struct DictationUseCase<I, S> {
    input: I,
    storage: S,
    committed_idempotency_keys: HashSet<String>,
}

impl<I, S> DictationUseCase<I, S>
where
    I: InputMethodPort,
    S: MetadataStorePort,
{
    #[must_use]
    pub fn new(input: I, storage: S) -> Self {
        Self {
            input,
            storage,
            committed_idempotency_keys: HashSet::new(),
        }
    }

    pub fn start_dictation(
        &mut self,
    ) -> Result<ImeSessionId, DictationUseCaseError<I::Error, S::Error>> {
        self.storage
            .create_session(None)
            .map_err(DictationUseCaseError::Storage)
    }

    pub fn transcript_ready(
        &mut self,
        session_id: ImeSessionId,
        text: &str,
    ) -> Result<(), DictationUseCaseError<I::Error, S::Error>> {
        self.input
            .show_preedit(session_id, text)
            .map_err(DictationUseCaseError::Input)
    }

    pub fn correct_preedit(
        &mut self,
        session_id: ImeSessionId,
        from_text: &str,
        to_text: &str,
        event_index: u32,
    ) -> Result<(), DictationUseCaseError<I::Error, S::Error>> {
        self.storage
            .record_preedit_change(session_id, from_text, to_text, event_index)
            .map_err(DictationUseCaseError::Storage)?;
        self.input
            .update_preedit(session_id, to_text)
            .map_err(DictationUseCaseError::Input)
    }

    pub fn commit(
        &mut self,
        session_id: ImeSessionId,
        committed_text: &str,
        idempotency_key: &str,
    ) -> Result<(), DictationUseCaseError<I::Error, S::Error>> {
        self.storage
            .commit_session(session_id, committed_text, idempotency_key)
            .map_err(DictationUseCaseError::Storage)?;
        if !self.committed_idempotency_keys.contains(idempotency_key) {
            self.input
                .commit_text(session_id, committed_text)
                .map_err(DictationUseCaseError::Input)?;
            self.committed_idempotency_keys
                .insert(idempotency_key.to_owned());
        }
        Ok(())
    }

    pub fn cancel(
        &mut self,
        session_id: ImeSessionId,
        idempotency_key: &str,
    ) -> Result<(), DictationUseCaseError<I::Error, S::Error>> {
        self.storage
            .cancel_session(session_id, idempotency_key)
            .map_err(DictationUseCaseError::Storage)?;
        self.input
            .cancel_preedit(session_id)
            .map_err(DictationUseCaseError::Input)
    }

    #[must_use]
    pub fn input(&self) -> &I {
        &self.input
    }

    #[must_use]
    pub fn storage(&self) -> &S {
        &self.storage
    }
}

#[cfg(test)]
impl
    DictationUseCase<
        idiolect_test_support::fakes::FakeInputMethod,
        idiolect_test_support::fakes::FakeMetadataStore,
    >
{
    fn input_events(&self) -> Vec<&str> {
        self.input().events()
    }

    fn storage_events(&self) -> Vec<&str> {
        self.storage().events()
    }

    fn training_candidate_count(&self) -> usize {
        self.storage().training_candidate_count()
    }
}

#[cfg(test)]
mod tests {
    use super::{DictationUseCase, DictationUseCaseError};
    use idiolect_common::ids::ImeSessionId;
    use idiolect_ports::input_method::InputMethodPort;
    use idiolect_test_support::fakes::{FakeInputMethod, FakeMetadataStore};
    use std::cell::Cell;

    #[test]
    fn transcript_ready_shows_preedit_and_records_session() {
        let mut use_case =
            DictationUseCase::new(FakeInputMethod::default(), FakeMetadataStore::default());

        let session_id = use_case.start_dictation().expect("session should start");
        use_case
            .transcript_ready(session_id, "restart traffic")
            .unwrap();

        assert_eq!(use_case.input_events(), ["show_preedit:restart traffic"]);
        assert_eq!(use_case.storage_events(), ["create_session:<none>"]);
    }

    #[test]
    fn correction_then_duplicate_commit_records_one_training_candidate() {
        let mut use_case =
            DictationUseCase::new(FakeInputMethod::default(), FakeMetadataStore::default());

        let session_id = use_case.start_dictation().expect("session should start");
        use_case
            .transcript_ready(session_id, "restart traffic")
            .unwrap();
        use_case
            .correct_preedit(session_id, "restart traffic", "restart Traefik", 0)
            .unwrap();
        use_case
            .commit(session_id, "restart Traefik", "commit-session-1")
            .unwrap();
        use_case
            .commit(session_id, "restart Traefik", "commit-session-1")
            .unwrap();

        assert_eq!(
            use_case.input_events(),
            [
                "show_preedit:restart traffic",
                "update_preedit:restart Traefik",
                "commit:restart Traefik",
            ]
        );
        assert_eq!(use_case.training_candidate_count(), 1);
    }

    #[test]
    fn cancel_after_preedit_clears_input_and_records_no_candidate() {
        let mut use_case =
            DictationUseCase::new(FakeInputMethod::default(), FakeMetadataStore::default());

        let session_id = use_case.start_dictation().expect("session should start");
        use_case.transcript_ready(session_id, "open notes").unwrap();
        use_case.cancel(session_id, "cancel-session-1").unwrap();

        assert_eq!(
            use_case.input_events(),
            ["show_preedit:open notes", "cancel_preedit"]
        );
        assert_eq!(use_case.training_candidate_count(), 0);
    }

    #[derive(Debug, Default)]
    struct FailOnceInputMethod {
        failed_once: Cell<bool>,
        events_log: Vec<String>,
    }

    impl FailOnceInputMethod {
        fn events(&self) -> Vec<&str> {
            self.events_log.iter().map(String::as_str).collect()
        }
    }

    impl InputMethodPort for FailOnceInputMethod {
        type Error = &'static str;

        fn show_preedit(
            &mut self,
            _session_id: ImeSessionId,
            text: &str,
        ) -> Result<(), Self::Error> {
            self.events_log.push(format!("show_preedit:{text}"));
            Ok(())
        }

        fn update_preedit(
            &mut self,
            _session_id: ImeSessionId,
            text: &str,
        ) -> Result<(), Self::Error> {
            self.events_log.push(format!("update_preedit:{text}"));
            Ok(())
        }

        fn commit_text(
            &mut self,
            _session_id: ImeSessionId,
            text: &str,
        ) -> Result<(), Self::Error> {
            if !self.failed_once.replace(true) {
                return Err("input commit failed");
            }
            self.events_log.push(format!("commit:{text}"));
            Ok(())
        }

        fn cancel_preedit(&mut self, _session_id: ImeSessionId) -> Result<(), Self::Error> {
            self.events_log.push("cancel_preedit".to_owned());
            Ok(())
        }
    }

    #[test]
    fn retry_after_input_commit_failure_replays_commit() {
        let mut use_case =
            DictationUseCase::new(FailOnceInputMethod::default(), FakeMetadataStore::default());
        let session_id = use_case.start_dictation().expect("session should start");

        let result = use_case.commit(session_id, "restart Traefik", "commit-session-1");
        assert_eq!(
            result,
            Err(DictationUseCaseError::Input("input commit failed"))
        );

        use_case
            .commit(session_id, "restart Traefik", "commit-session-1")
            .unwrap();

        assert_eq!(use_case.input().events(), ["commit:restart Traefik"]);
    }
}
