#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImeSessionState {
    Created,
    Recording,
    Transcribing,
    PreeditActive,
    Committed,
    Cancelled,
    Abandoned,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImeSession {
    state: ImeSessionState,
    raw_stt_text: Option<String>,
    committed_text: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionTransitionError {
    AlreadyCommitted,
    AlreadyCancelled,
    InvalidTransition {
        from: ImeSessionState,
        action: &'static str,
    },
}

impl ImeSession {
    #[must_use]
    pub fn new_for_test() -> Self {
        Self {
            state: ImeSessionState::Created,
            raw_stt_text: None,
            committed_text: None,
        }
    }

    #[must_use]
    pub fn state(&self) -> ImeSessionState {
        self.state
    }

    #[must_use]
    pub fn recording_started(self) -> Self {
        if self.state == ImeSessionState::Created {
            Self {
                state: ImeSessionState::Recording,
                ..self
            }
        } else {
            self
        }
    }

    #[must_use]
    pub fn transcription_started(self) -> Self {
        if self.state == ImeSessionState::Recording {
            Self {
                state: ImeSessionState::Transcribing,
                ..self
            }
        } else {
            self
        }
    }

    #[must_use]
    pub fn preedit_started(self, raw_stt_text: &str) -> Self {
        if self.state == ImeSessionState::Transcribing {
            Self {
                state: ImeSessionState::PreeditActive,
                raw_stt_text: Some(raw_stt_text.to_owned()),
                ..self
            }
        } else {
            self
        }
    }

    #[must_use]
    pub fn committed(self, committed_text: &str) -> Self {
        if self.state == ImeSessionState::PreeditActive {
            Self {
                state: ImeSessionState::Committed,
                committed_text: Some(committed_text.to_owned()),
                ..self
            }
        } else {
            self
        }
    }

    pub fn try_commit(&self, committed_text: &str) -> Result<Self, SessionTransitionError> {
        match self.state {
            ImeSessionState::PreeditActive => Ok(Self {
                state: ImeSessionState::Committed,
                raw_stt_text: self.raw_stt_text.clone(),
                committed_text: Some(committed_text.to_owned()),
            }),
            ImeSessionState::Committed
                if self.committed_text.as_deref() == Some(committed_text) =>
            {
                Ok(self.clone())
            }
            ImeSessionState::Cancelled | ImeSessionState::Abandoned => {
                Err(SessionTransitionError::AlreadyCancelled)
            }
            _ => Err(SessionTransitionError::InvalidTransition {
                from: self.state,
                action: "commit",
            }),
        }
    }

    pub fn try_cancel(&self) -> Result<Self, SessionTransitionError> {
        match self.state {
            ImeSessionState::Committed => Err(SessionTransitionError::AlreadyCommitted),
            ImeSessionState::Cancelled | ImeSessionState::Abandoned => Ok(self.clone()),
            _ => Ok(Self {
                state: ImeSessionState::Cancelled,
                raw_stt_text: self.raw_stt_text.clone(),
                committed_text: self.committed_text.clone(),
            }),
        }
    }
}
