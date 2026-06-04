#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrainingManifestItem {
    audio_object_key: String,
    audio_digest: String,
    transcript: String,
}

impl TrainingManifestItem {
    #[must_use]
    pub fn new(
        audio_object_key: impl Into<String>,
        audio_digest: impl Into<String>,
        transcript: impl Into<String>,
    ) -> Self {
        Self {
            audio_object_key: audio_object_key.into(),
            audio_digest: audio_digest.into(),
            transcript: transcript.into(),
        }
    }

    #[must_use]
    pub fn audio_object_key(&self) -> &str {
        &self.audio_object_key
    }

    #[must_use]
    pub fn audio_digest(&self) -> &str {
        &self.audio_digest
    }

    #[must_use]
    pub fn transcript(&self) -> &str {
        &self.transcript
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrainingManifest {
    digest: String,
    base_model_id: String,
    items: Vec<TrainingManifestItem>,
}

impl TrainingManifest {
    #[must_use]
    pub fn new(
        digest: impl Into<String>,
        base_model_id: impl Into<String>,
        items: Vec<TrainingManifestItem>,
    ) -> Self {
        Self {
            digest: digest.into(),
            base_model_id: base_model_id.into(),
            items,
        }
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    #[must_use]
    pub fn base_model_id(&self) -> &str {
        &self.base_model_id
    }

    #[must_use]
    pub fn items(&self) -> &[TrainingManifestItem] {
        &self.items
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrainingConfig {
    candidate_id: String,
}

impl TrainingConfig {
    #[must_use]
    pub fn new(candidate_id: impl Into<String>) -> Self {
        Self {
            candidate_id: candidate_id.into(),
        }
    }

    #[must_use]
    pub fn candidate_id(&self) -> &str {
        &self.candidate_id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TrainingArtifact {
    digest: String,
    manifest_digest: String,
    base_model_id: String,
    backend_id: String,
    candidate_id: String,
}

impl TrainingArtifact {
    #[must_use]
    pub fn new(
        digest: impl Into<String>,
        manifest_digest: impl Into<String>,
        base_model_id: impl Into<String>,
        backend_id: impl Into<String>,
        candidate_id: impl Into<String>,
    ) -> Self {
        Self {
            digest: digest.into(),
            manifest_digest: manifest_digest.into(),
            base_model_id: base_model_id.into(),
            backend_id: backend_id.into(),
            candidate_id: candidate_id.into(),
        }
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    #[must_use]
    pub fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }

    #[must_use]
    pub fn base_model_id(&self) -> &str {
        &self.base_model_id
    }

    #[must_use]
    pub fn backend_id(&self) -> &str {
        &self.backend_id
    }

    #[must_use]
    pub fn candidate_id(&self) -> &str {
        &self.candidate_id
    }
}
