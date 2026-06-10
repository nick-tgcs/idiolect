use std::collections::HashSet;
use std::convert::Infallible;

use idiolect_common::ids::ImeSessionId;
use idiolect_ports::input_method::InputMethodPort;
use idiolect_ports::storage::{HistoryEntry, HistoryState, MetadataStorePort};

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

#[derive(Debug, Default, Clone)]
pub struct FakeHistoryEntry {
    pub id: i64,
    pub session_id: ImeSessionId,
    pub text: String,
    pub state: HistoryState,
    pub created_at: String,
}

#[derive(Debug, Default)]
pub struct FakeMetadataStore {
    events_log: Vec<String>,
    idempotency_keys: HashSet<String>,
    candidate_count: usize,
    history_entries: Vec<FakeHistoryEntry>,
    next_history_id: i64,
    tray_settings: std::collections::HashMap<String, String>,
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

    #[must_use]
    pub fn history_entries(&self) -> Vec<HistoryEntry> {
        self.history_entries
            .iter()
            .map(|e| HistoryEntry {
                id: e.id,
                session_id: e.session_id,
                text: e.text.clone(),
                state: e.state,
                created_at: e.created_at.clone(),
            })
            .collect()
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
        session_id: ImeSessionId,
        committed_text: &str,
        idempotency_key: &str,
    ) -> Result<(), Self::Error> {
        if self.idempotency_keys.insert(idempotency_key.to_owned()) {
            self.events_log.push(format!("commit:{committed_text}"));
            self.candidate_count += 1;
            // Store in history
            self.next_history_id += 1;
            self.history_entries.push(FakeHistoryEntry {
                id: self.next_history_id,
                session_id,
                text: committed_text.to_owned(),
                state: HistoryState::Committed,
                created_at: chrono::Utc::now().to_rfc3339(),
            });
        }
        Ok(())
    }

    fn cancel_session(
        &mut self,
        session_id: ImeSessionId,
        idempotency_key: &str,
    ) -> Result<(), Self::Error> {
        if self.idempotency_keys.insert(idempotency_key.to_owned()) {
            self.events_log.push("cancel".to_owned());
            // Store in history
            self.next_history_id += 1;
            self.history_entries.push(FakeHistoryEntry {
                id: self.next_history_id,
                session_id,
                text: String::new(), // cancelled sessions have empty text
                state: HistoryState::Cancelled,
                created_at: chrono::Utc::now().to_rfc3339(),
            });
        }
        Ok(())
    }

    fn recent_history(&self, limit: u32) -> Result<Vec<HistoryEntry>, Self::Error> {
        let mut entries = self.history_entries.clone();
        entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        let entries: Vec<HistoryEntry> = entries
            .into_iter()
            .take(limit as usize)
            .map(|e| HistoryEntry {
                id: e.id,
                session_id: e.session_id,
                text: e.text,
                state: e.state,
                created_at: e.created_at,
            })
            .collect();
        Ok(entries)
    }

    fn get_history_entry(&self, id: i64) -> Result<Option<HistoryEntry>, Self::Error> {
        Ok(self
            .history_entries
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| HistoryEntry {
                id: entry.id,
                session_id: entry.session_id,
                text: entry.text.clone(),
                state: entry.state,
                created_at: entry.created_at.clone(),
            }))
    }

    fn prune_history(&mut self, older_than_days: u32) -> Result<u64, Self::Error> {
        let cutoff = chrono::Utc::now() - chrono::Duration::days(older_than_days as i64);
        let cutoff_str = cutoff.to_rfc3339();
        let original_len = self.history_entries.len();
        self.history_entries.retain(|e| e.created_at >= cutoff_str);
        Ok((original_len - self.history_entries.len()) as u64)
    }

    fn delete_history_entry(&mut self, id: i64) -> Result<(), Self::Error> {
        self.history_entries.retain(|e| e.id != id);
        Ok(())
    }

    fn get_tray_setting(&self, key: &str) -> Result<Option<String>, Self::Error> {
        Ok(self.tray_settings.get(key).cloned())
    }

    fn set_tray_setting(&mut self, key: &str, value: &str) -> Result<(), Self::Error> {
        self.tray_settings
            .insert(key.to_string(), value.to_string());
        Ok(())
    }

    fn get_all_tray_settings(
        &self,
    ) -> Result<std::collections::HashMap<String, String>, Self::Error> {
        Ok(self.tray_settings.clone())
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
