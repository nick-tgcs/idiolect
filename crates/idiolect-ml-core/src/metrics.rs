#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvaluationSuites {
    suite_ids: Vec<String>,
}

impl EvaluationSuites {
    #[must_use]
    pub fn new(suite_ids: Vec<String>) -> Self {
        Self { suite_ids }
    }

    #[must_use]
    pub fn suite_ids(&self) -> &[String] {
        &self.suite_ids
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvaluationReport {
    digest: String,
    artifact_digest: String,
}

impl EvaluationReport {
    #[must_use]
    pub fn new(digest: impl Into<String>, artifact_digest: impl Into<String>) -> Self {
        Self {
            digest: digest.into(),
            artifact_digest: artifact_digest.into(),
        }
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    #[must_use]
    pub fn artifact_digest(&self) -> &str {
        &self.artifact_digest
    }
}
