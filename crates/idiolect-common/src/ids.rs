use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
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
    use super::{ImeSessionId, UserId};

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
}
