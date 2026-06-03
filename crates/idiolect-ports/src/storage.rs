use idiolect_common::ids::ImeSessionId;

pub trait MetadataStorePort {
    type Error;

    fn create_session(&mut self, raw_stt_text: Option<&str>) -> Result<ImeSessionId, Self::Error>;
    fn record_preedit_change(
        &mut self,
        session_id: ImeSessionId,
        from_text: &str,
        to_text: &str,
        event_index: u32,
    ) -> Result<(), Self::Error>;
    fn commit_session(
        &mut self,
        session_id: ImeSessionId,
        committed_text: &str,
        idempotency_key: &str,
    ) -> Result<(), Self::Error>;
    fn cancel_session(
        &mut self,
        session_id: ImeSessionId,
        idempotency_key: &str,
    ) -> Result<(), Self::Error>;
}
