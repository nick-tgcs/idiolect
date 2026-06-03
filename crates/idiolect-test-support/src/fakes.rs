use std::convert::Infallible;

use idiolect_common::ids::ImeSessionId;
use idiolect_ports::input_method::InputMethodPort;

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
