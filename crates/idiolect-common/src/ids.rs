use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ImeSessionId(Uuid);

impl ImeSessionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for ImeSessionId {
    fn default() -> Self {
        Self::new()
    }
}

/// The audio-store key for a session's single recording: `utterance:<uuid>`.
///
/// The source-audio object store and the `audio_sha256` digest are both keyed by
/// this, so every front-end — the desktop daemon and the in-process mobile facade
/// — must derive it identically. Keeping it here (rather than copied per
/// front-end) makes that impossible to drift.
#[must_use]
pub fn utterance_id_for_session(session_id: ImeSessionId) -> String {
    // The bare hyphenated UUID. `Uuid`'s `Display` is byte-identical to the inner
    // value of `ImeSessionId`'s JSON serialization with its quotes stripped — the
    // form the desktop daemon historically wrote — so audio already on disk keeps
    // resolving. The test below cross-checks that equivalence.
    format!("utterance:{}", session_id.0)
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct UserId(String);

impl UserId {
    #[must_use]
    pub fn default_user() -> Self {
        Self("default".to_owned())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{utterance_id_for_session, ImeSessionId, UserId};

    #[test]
    fn ime_session_id_round_trips_through_json() {
        let id = ImeSessionId::new();
        let encoded = serde_json::to_string(&id).expect("session id should serialize");
        let decoded: ImeSessionId =
            serde_json::from_str(&encoded).expect("session id should deserialize");
        assert_eq!(decoded, id);
    }

    #[test]
    fn default_user_id_is_stable() {
        assert_eq!(UserId::default_user().as_str(), "default");
    }

    #[test]
    fn utterance_id_is_the_session_uuid_with_a_stable_prefix() {
        let id = ImeSessionId::new();
        let utterance_id = utterance_id_for_session(id);

        // `utterance:<uuid>`, where `<uuid>` is the JSON value with its quotes
        // stripped — never the quoted JSON form (a quote in an object key would
        // be rejected by the audio store's identifier validation).
        let bare_uuid = serde_json::to_string(&id)
            .unwrap()
            .trim_matches('"')
            .to_owned();
        assert_eq!(utterance_id, format!("utterance:{bare_uuid}"));
        assert!(!utterance_id.contains('"'));
        // Deterministic: the same session always maps to the same key (so audio
        // written, digested, and later deleted all resolve to one object).
        assert_eq!(utterance_id, utterance_id_for_session(id));
    }

    #[test]
    fn ime_session_id_can_be_used_in_ordered_sets() {
        let id = ImeSessionId::new();
        let mut session_ids = BTreeSet::new();

        assert!(session_ids.insert(id));
        assert!(session_ids.contains(&id));
    }
}
