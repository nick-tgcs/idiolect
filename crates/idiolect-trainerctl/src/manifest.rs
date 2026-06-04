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

pub struct ManifestV2CandidateInput {
    pub training_candidate_id: String,
    pub user_id: String,
    pub utterance_id: String,
    pub text_session_id: String,
    pub audio_object_key: String,
    pub audio_digest: String,
    pub raw_transcript: String,
    pub corrected_transcript: String,
    pub source_label: String,
    pub label: CandidateLabel,
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

pub struct ManifestV2BuildInput {
    user_id: String,
    base_model_id: String,
    candidates: Vec<ManifestV2CandidateInput>,
}

impl ManifestV2BuildInput {
    #[must_use]
    pub fn new(
        user_id: impl Into<String>,
        base_model_id: impl Into<String>,
        candidates: Vec<ManifestV2CandidateInput>,
    ) -> Self {
        Self {
            user_id: user_id.into(),
            base_model_id: base_model_id.into(),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ManifestSplit {
    Train,
    Validation,
    Holdout,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ManifestV2Item {
    user_id: String,
    training_candidate_id: String,
    utterance_id: String,
    text_session_id: String,
    audio_object_key: String,
    audio_digest: String,
    raw_transcript: String,
    corrected_transcript: String,
    split: ManifestSplit,
    source_label: String,
    trust_score_bps: u16,
    base_model_id: String,
}

impl ManifestV2Item {
    #[must_use]
    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    #[must_use]
    pub fn training_candidate_id(&self) -> &str {
        &self.training_candidate_id
    }

    #[must_use]
    pub fn utterance_id(&self) -> &str {
        &self.utterance_id
    }

    #[must_use]
    pub fn text_session_id(&self) -> &str {
        &self.text_session_id
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
    pub fn raw_transcript(&self) -> &str {
        &self.raw_transcript
    }

    #[must_use]
    pub fn corrected_transcript(&self) -> &str {
        &self.corrected_transcript
    }

    #[must_use]
    pub fn split(&self) -> ManifestSplit {
        self.split
    }

    #[must_use]
    pub fn source_label(&self) -> &str {
        &self.source_label
    }

    #[must_use]
    pub fn trust_score_bps(&self) -> u16 {
        self.trust_score_bps
    }

    #[must_use]
    pub fn base_model_id(&self) -> &str {
        &self.base_model_id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestV2 {
    items: Vec<ManifestV2Item>,
    digest: String,
}

impl ManifestV2 {
    #[must_use]
    pub fn items(&self) -> &[ManifestV2Item] {
        &self.items
    }

    #[must_use]
    pub fn items_for_split(&self, split: ManifestSplit) -> Vec<&ManifestV2Item> {
        self.items
            .iter()
            .filter(|item| item.split == split)
            .collect()
    }

    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManifestBuildError {
    EmptyUserId,
    EmptyBaseModelId,
    EmptyAudioDigest,
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

#[derive(Serialize)]
struct DigestV2Input<'a> {
    user_id: &'a str,
    base_model_id: &'a str,
    items: &'a [ManifestV2Item],
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

    pub fn build_v2(input: ManifestV2BuildInput) -> Result<ManifestV2, ManifestBuildError> {
        if input.user_id.trim().is_empty() {
            return Err(ManifestBuildError::EmptyUserId);
        }
        if input.base_model_id.trim().is_empty() {
            return Err(ManifestBuildError::EmptyBaseModelId);
        }

        let mut approved = input
            .candidates
            .into_iter()
            .filter_map(|candidate| match candidate.label {
                CandidateLabel::Approved { trust_score_bps } => Some((candidate, trust_score_bps)),
                CandidateLabel::Rejected { .. } => None,
            })
            .collect::<Vec<_>>();
        approved.sort_by(|left, right| {
            left.0
                .training_candidate_id
                .cmp(&right.0.training_candidate_id)
        });

        let (train_count, validation_count) = split_counts(approved.len());
        let mut items = Vec::with_capacity(approved.len());

        for (index, (candidate, trust_score_bps)) in approved.into_iter().enumerate() {
            if candidate.audio_digest.trim().is_empty() {
                return Err(ManifestBuildError::EmptyAudioDigest);
            }

            items.push(ManifestV2Item {
                user_id: candidate.user_id,
                training_candidate_id: candidate.training_candidate_id,
                utterance_id: candidate.utterance_id,
                text_session_id: candidate.text_session_id,
                audio_object_key: candidate.audio_object_key,
                audio_digest: candidate.audio_digest,
                raw_transcript: candidate.raw_transcript,
                corrected_transcript: candidate.corrected_transcript,
                split: split_for_index(index, train_count, validation_count),
                source_label: candidate.source_label,
                trust_score_bps,
                base_model_id: input.base_model_id.clone(),
            });
        }

        let digest_input = DigestV2Input {
            user_id: &input.user_id,
            base_model_id: &input.base_model_id,
            items: &items,
        };
        let digest_bytes = serde_json::to_vec(&digest_input)
            .expect("manifest v2 digest input contains only infallible JSON values");

        Ok(ManifestV2 {
            items,
            digest: sha256_lower_hex(&digest_bytes),
        })
    }
}

fn split_counts(total: usize) -> (usize, usize) {
    if total < 3 {
        return (total, 0);
    }

    let holdout_count = (total / 10).max(1);
    let validation_count = (total / 10).max(1);
    let train_count = total
        .saturating_sub(holdout_count + validation_count)
        .max(1);
    (train_count, validation_count)
}

fn split_for_index(index: usize, train_count: usize, validation_count: usize) -> ManifestSplit {
    if index < train_count {
        ManifestSplit::Train
    } else if index < train_count + validation_count {
        ManifestSplit::Validation
    } else {
        ManifestSplit::Holdout
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
