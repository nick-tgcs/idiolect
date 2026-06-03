#[cfg(test)]
mod tests {
    use crate::domain::session::{ImeSession, ImeSessionState};

    #[test]
    fn committed_session_cannot_be_cancelled() {
        let session = ImeSession::new_for_test()
            .recording_started()
            .transcription_started()
            .preedit_started("restart traffic")
            .committed("restart Traefik");

        let result = session.try_cancel();

        assert!(result.is_err());
        assert_eq!(session.state(), ImeSessionState::Committed);
    }

    #[test]
    fn duplicate_commit_is_idempotent() {
        let session = ImeSession::new_for_test()
            .recording_started()
            .transcription_started()
            .preedit_started("restart traffic")
            .committed("restart Traefik");

        let result = session.try_commit("restart Traefik");

        assert!(result.is_ok());
        assert_eq!(session.state(), ImeSessionState::Committed);
    }

    #[test]
    fn direct_commit_from_created_state_does_not_bypass_lifecycle() {
        let session = ImeSession::new_for_test().committed("restart Traefik");

        assert_eq!(session.state(), ImeSessionState::Created);
    }

    #[test]
    fn duplicate_cancel_is_idempotent() {
        let session = ImeSession::new_for_test().recording_started();
        let cancelled = session.try_cancel().expect("initial cancel should succeed");

        let duplicate = cancelled.try_cancel();

        assert!(duplicate.is_ok());
        assert_eq!(cancelled.state(), ImeSessionState::Cancelled);
        assert_eq!(
            duplicate
                .expect("duplicate cancel should remain cancelled")
                .state(),
            ImeSessionState::Cancelled
        );
    }
}
