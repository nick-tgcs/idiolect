use std::convert::Infallible;

use idiolect_application::use_cases::dictation::{DictationUseCase, DictationUseCaseError};
use idiolect_common::ids::ImeSessionId;
use idiolect_test_support::fakes::{FakeInputMethod, FakeMetadataStore};

#[derive(Debug)]
pub struct FixtureDaemon {
    use_case: DictationUseCase<FakeInputMethod, FakeMetadataStore>,
    session_id: Option<ImeSessionId>,
    transcript: String,
}

impl FixtureDaemon {
    const COMMIT_IDEMPOTENCY_KEY: &'static str = "fixture-daemon-commit";
    const CANCEL_IDEMPOTENCY_KEY: &'static str = "fixture-daemon-cancel";

    pub fn new_for_tests<S: Into<String>>(transcript: S) -> Self {
        let transcript = transcript.into();
        Self {
            use_case: DictationUseCase::new(
                FakeInputMethod::default(),
                FakeMetadataStore::default(),
            ),
            session_id: None,
            transcript,
        }
    }

    pub fn begin_fake_dictation(
        &mut self,
    ) -> Result<(), DictationUseCaseError<Infallible, Infallible>> {
        let session_id = self.use_case.start_dictation()?;
        self.use_case
            .transcript_ready(session_id, &self.transcript)?;
        self.session_id = Some(session_id);
        Ok(())
    }

    pub fn correct(
        &mut self,
        corrected_text: &str,
    ) -> Result<(), DictationUseCaseError<Infallible, Infallible>> {
        let session_id = self.session_id.expect("dictation has not been started");
        self.use_case
            .correct_preedit(session_id, &self.transcript, corrected_text, 0)?;
        self.transcript = corrected_text.to_owned();
        Ok(())
    }

    pub fn commit(&mut self) -> Result<(), DictationUseCaseError<Infallible, Infallible>> {
        let session_id = self.session_id.expect("dictation has not been started");
        self.use_case
            .commit(session_id, &self.transcript, Self::COMMIT_IDEMPOTENCY_KEY)
    }

    pub fn cancel(&mut self) -> Result<(), DictationUseCaseError<Infallible, Infallible>> {
        let session_id = self.session_id.expect("dictation has not been started");
        self.use_case
            .cancel(session_id, Self::CANCEL_IDEMPOTENCY_KEY)
    }

    pub fn input_events(&self) -> Vec<&str> {
        self.use_case.input().events()
    }

    pub fn training_candidate_count(&self) -> usize {
        self.use_case.storage().training_candidate_count()
    }
}

#[cfg(test)]
mod tests {
    use super::FixtureDaemon;

    #[test]
    fn fixture_daemon_commits_corrected_text_once() {
        let mut daemon = FixtureDaemon::new_for_tests("restart traffic");

        daemon.begin_fake_dictation().unwrap();
        daemon.correct("restart Traefik").unwrap();
        daemon.commit().unwrap();
        daemon.commit().unwrap();

        assert_eq!(
            daemon.input_events(),
            [
                "show_preedit:restart traffic",
                "update_preedit:restart Traefik",
                "commit:restart Traefik"
            ]
        );
        assert_eq!(daemon.training_candidate_count(), 1);
    }

    #[test]
    fn fixture_daemon_cancel_records_no_candidate() {
        let mut daemon = FixtureDaemon::new_for_tests("open notes");

        daemon.begin_fake_dictation().unwrap();
        daemon.cancel().unwrap();

        assert_eq!(
            daemon.input_events(),
            ["show_preedit:open notes", "cancel_preedit"]
        );
        assert_eq!(daemon.training_candidate_count(), 0);
    }
}
