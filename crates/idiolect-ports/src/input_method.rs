use idiolect_common::ids::ImeSessionId;

pub trait InputMethodPort {
    type Error;

    fn show_preedit(&mut self, session_id: ImeSessionId, text: &str) -> Result<(), Self::Error>;
    fn update_preedit(&mut self, session_id: ImeSessionId, text: &str) -> Result<(), Self::Error>;
    fn commit_text(&mut self, session_id: ImeSessionId, text: &str) -> Result<(), Self::Error>;
    fn cancel_preedit(&mut self, session_id: ImeSessionId) -> Result<(), Self::Error>;
}
