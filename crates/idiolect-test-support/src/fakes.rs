use std::collections::HashSet;
use std::convert::Infallible;

use idiolect_common::ids::ImeSessionId;
use idiolect_ports::input_method::InputMethodPort;
use idiolect_ports::storage::MetadataStorePort;

#[derive(Debug, Default)]
pub struct FakeInputMethod {
    events_log: Vec<String>,
}

impl FakeInputMethod {
    #[must_use]
    pub fn events(&self) -> Vec<&str> {
        self.events_log.iter().map(String::as_str).collect()
    }
}

impl InputMethodPort for FakeInputMethod {
    type Error = Infallible;

    fn show_preedit(&mut self, _session_id: ImeSessionId, text: &str) -> Result<(), Self::Error> {
        self.events_log.push(format!("show_preedit:{text}"));
        Ok(())
    }

    fn update_preedit(&mut self, _session_id: ImeSessionId, text: &str) -> Result<(), Self::Error> {
        self.events_log.push(format!("update_preedit:{text}"));
        Ok(())
    }

    fn commit_text(&mut self, _session_id: ImeSessionId, text: &str) -> Result<(), Self::Error> {
        self.events_log.push(format!("commit:{text}"));
        Ok(())
    }

    fn cancel_preedit(&mut self, _session_id: ImeSessionId) -> Result<(), Self::Error> {
        self.events_log.push("cancel_preedit".to_owned());
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct FakeMetadataStore {
    events_log: Vec<String>,
    idempotency_keys: HashSet<String>,
    candidate_count: usize,
}

impl FakeMetadataStore {
    #[must_use]
    pub fn events(&self) -> Vec<&str> {
        self.events_log.iter().map(String::as_str).collect()
    }

    #[must_use]
    pub fn training_candidate_count(&self) -> usize {
        self.candidate_count
    }
}

impl MetadataStorePort for FakeMetadataStore {
    type Error = Infallible;

    fn create_session(&mut self, raw_stt_text: Option<&str>) -> Result<ImeSessionId, Self::Error> {
        let event = match raw_stt_text {
            Some(text) => format!("create_session:{text}"),
            None => "create_session:<none>".to_owned(),
        };
        self.events_log.push(event);
        Ok(ImeSessionId::new())
    }

    fn record_preedit_change(
        &mut self,
        _session_id: ImeSessionId,
        from_text: &str,
        to_text: &str,
        event_index: u32,
    ) -> Result<(), Self::Error> {
        self.events_log
            .push(format!("correction:{from_text}->{to_text}:{event_index}"));
        Ok(())
    }

    fn commit_session(
        &mut self,
        _session_id: ImeSessionId,
        committed_text: &str,
        idempotency_key: &str,
    ) -> Result<(), Self::Error> {
        if self.idempotency_keys.insert(idempotency_key.to_owned()) {
            self.events_log.push(format!("commit:{committed_text}"));
            self.candidate_count += 1;
        }
        Ok(())
    }

    fn cancel_session(
        &mut self,
        _session_id: ImeSessionId,
        idempotency_key: &str,
    ) -> Result<(), Self::Error> {
        if self.idempotency_keys.insert(idempotency_key.to_owned()) {
            self.events_log.push("cancel".to_owned());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::FakeInputMethod;
    use idiolect_common::ids::ImeSessionId;
    use idiolect_ports::input_method::InputMethodPort;

    #[test]
    fn input_method_records_preedit_before_commit() {
        let mut input = FakeInputMethod::default();
        let session_id = ImeSessionId::new();

        input
            .show_preedit(session_id, "restart traffic")
            .expect("preedit should show");
        input
            .commit_text(session_id, "restart Traefik")
            .expect("commit should succeed");

        assert_eq!(
            input.events(),
            ["show_preedit:restart traffic", "commit:restart Traefik"]
        );
    }
}
