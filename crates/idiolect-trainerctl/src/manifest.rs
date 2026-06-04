use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::CandidateLabel;

pub struct ManifestCandidateInput {
    id: String,
    raw_text: String,
    target_text: String,
    label: CandidateLabel,
}

impl ManifestCandidateInput {
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        raw_text: impl Into<String>,
        target_text: impl Into<String>,
        label: CandidateLabel,
    ) -> Self {
        Self {
            id: id.into(),
            raw_text: raw_text.into(),
            target_text: target_text.into(),
            label,
        }
    }
}

pub struct ManifestBuildInput {
    user_id: String,
    candidates: Vec<ManifestCandidateInput>,
}

impl ManifestBuildInput {
    #[must_use]
    pub fn new(user_id: impl Into<String>, candidates: Vec<ManifestCandidateInput>) -> Self {
        Self {
            user_id: user_id.into(),
            candidates,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestCandidate {
    id: String,
    raw_text: String,
    target_text: String,
    trust_score_bps: u16,
}

impl ManifestCandidate {
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn raw_text(&self) -> &str {
        &self.raw_text
    }

    #[must_use]
    pub fn target_text(&self) -> &str {
        &self.target_text
    }

    #[must_use]
    pub fn trust_score_bps(&self) -> u16 {
        self.trust_score_bps
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Manifest {
    candidates: Vec<ManifestCandidate>,
    digest: String,
}

impl Manifest {
    #[must_use]
    pub fn candidates(&self) -> &[ManifestCandidate] {
        &self.candidates
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManifestBuildError {
    EmptyUserId,
}

#[derive(Serialize)]
struct DigestInput {
    user_id: String,
    candidates: Vec<DigestCandidate>,
}

#[derive(Serialize)]
struct DigestCandidate {
    id: String,
    raw_text: String,
    target_text: String,
    trust_score_bps: u16,
}

pub struct LearningManifestBuilder;

impl LearningManifestBuilder {
    pub fn build(input: ManifestBuildInput) -> Result<Manifest, ManifestBuildError> {
        if input.user_id.trim().is_empty() {
            return Err(ManifestBuildError::EmptyUserId);
        }

        let mut candidates = input
            .candidates
            .into_iter()
            .filter_map(|candidate| {
                let ManifestCandidateInput {
                    id,
                    raw_text,
                    target_text,
                    label,
                } = candidate;
                match label {
                    CandidateLabel::Approved { trust_score_bps } => Some(ManifestCandidate {
                        id,
                        raw_text,
                        target_text,
                        trust_score_bps,
                    }),
                    CandidateLabel::Rejected { .. } => None,
                }
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.id.cmp(&right.id));

        let digest_input = DigestInput {
            user_id: input.user_id,
            candidates: candidates
                .iter()
                .map(|candidate| DigestCandidate {
                    id: candidate.id.clone(),
                    raw_text: candidate.raw_text.clone(),
                    target_text: candidate.target_text.clone(),
                    trust_score_bps: candidate.trust_score_bps,
                })
                .collect(),
        };
        let digest_bytes = serde_json::to_vec(&digest_input)
            .expect("manifest digest input contains only infallible JSON values");

        Ok(Manifest {
            candidates,
            digest: sha256_lower_hex(&digest_bytes),
        })
    }
}

fn sha256_lower_hex(input: &[u8]) -> String {
    let digest = Sha256::digest(input);
    let mut output = String::with_capacity(64);
    const HEX: &[u8; 16] = b"0123456789abcdef";

    for byte in digest {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }

    output
}

#[cfg(test)]
mod tests {
    use super::{
        LearningManifestBuilder, ManifestBuildError, ManifestBuildInput, ManifestCandidateInput,
    };
    use crate::CandidateLabel;

    fn approved_candidate(id: &str, trust_score_bps: u16) -> ManifestCandidateInput {
        ManifestCandidateInput::new(
            id,
            format!("{id} raw"),
            format!("{id} target"),
            CandidateLabel::Approved { trust_score_bps },
        )
    }

    fn rejected_candidate(id: &str) -> ManifestCandidateInput {
        ManifestCandidateInput::new(
            id,
            format!("{id} raw"),
            format!("{id} target"),
            CandidateLabel::Rejected {
                reason: "unchanged_text",
            },
        )
    }

    #[test]
    fn manifest_includes_only_approved_candidates_in_stable_order() {
        let manifest = LearningManifestBuilder::build(ManifestBuildInput::new(
            "default",
            vec![
                approved_candidate("b", 6_000),
                rejected_candidate("c"),
                approved_candidate("a", 10_000),
            ],
        ))
        .expect("manifest should build");

        let ids = manifest
            .candidates()
            .iter()
            .map(|candidate| candidate.id())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn manifest_digest_is_stable_for_same_inputs() {
        let first = LearningManifestBuilder::build(ManifestBuildInput::new(
            "default",
            vec![
                approved_candidate("b", 6_000),
                rejected_candidate("c"),
                approved_candidate("a", 10_000),
            ],
        ))
        .expect("first manifest should build");
        let second = LearningManifestBuilder::build(ManifestBuildInput::new(
            "default",
            vec![
                approved_candidate("b", 6_000),
                rejected_candidate("c"),
                approved_candidate("a", 10_000),
            ],
        ))
        .expect("second manifest should build");

        assert_eq!(first.digest(), second.digest());
        assert_eq!(first.digest().len(), 64);
        assert!(first
            .digest()
            .chars()
            .all(|character| matches!(character, '0'..='9' | 'a'..='f')));
    }

    #[test]
    fn manifest_rejects_empty_user_id() {
        let error = LearningManifestBuilder::build(ManifestBuildInput::new("  ", vec![]))
            .expect_err("empty user should fail");

        assert_eq!(error, ManifestBuildError::EmptyUserId);
    }
}
