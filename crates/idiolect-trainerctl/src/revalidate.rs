//! Revalidates stored training candidates against their stored audio.
//!
//! The streaming pipeline used to drop words at pause boundaries (short-context
//! snippet decodes), so a candidate's stored text can omit words its stored
//! audio provably contains. Training on such a pair teaches the model to skip
//! words. Revalidation re-decodes each candidate's audio WHOLE and repairs or
//! rejects the record:
//!
//! - `accepted_without_edit`: the user never proof-read this text — the whole
//!   decode is strictly the better label, so any material difference replaces
//!   every stored copy of the text.
//! - `accepted_with_edit`: the user's correction is gold and stays — unless the
//!   audio contains word runs the text the user saw never had (a pipeline
//!   drop), in which case the correction was made against text the user never
//!   fully saw and the candidate is rejected as untrainable.

use idiolect_adapter_opus::OpusCodec;
use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};
use idiolect_ports::audio::AudioSegment;
use idiolect_ports::codec::AudioCodecPort;
use idiolect_ports::storage::{AudioObjectRef, AudioStorePort};
use serde::Serialize;

const SOURCE_ACCEPTED_WITHOUT_EDIT: &str = "accepted_without_edit";
const SOURCE_ACCEPTED_WITH_EDIT: &str = "accepted_with_edit";

/// A pure-insertion run must be at least this many words before an edited
/// candidate is rejected: single-word differences are ordinary decode
/// variance, dropped phrases ("I don't want") are not.
const MIN_OMISSION_RUN_WORDS: usize = 2;

#[derive(Debug, PartialEq, Eq)]
pub enum RevalidationOutcome {
    /// The stored text is consistent with the audio: leave it alone.
    Unchanged,
    /// Replace every stored copy of the text with the whole-audio decode.
    Retranscribe { text: String },
    /// The text cannot be trusted against the audio: untrainable.
    Reject { reason: String },
}

/// Decides what to do with one candidate given its stored texts and the fresh
/// whole-audio decode.
#[must_use]
pub fn decide(source: &str, raw: &str, corrected: &str, revalidated: &str) -> RevalidationOutcome {
    let revalidated_text = revalidated.trim();
    let revalidated_words = normalize_words(revalidated_text);
    if revalidated_words.is_empty() || is_noise_only(revalidated_text) {
        return RevalidationOutcome::Reject {
            reason: format!(
                "revalidation heard no speech in the stored audio (decode: {revalidated_text:?})"
            ),
        };
    }
    match source {
        SOURCE_ACCEPTED_WITHOUT_EDIT => {
            if normalize_words(corrected) == revalidated_words {
                RevalidationOutcome::Unchanged
            } else {
                RevalidationOutcome::Retranscribe {
                    text: revalidated_text.to_owned(),
                }
            }
        }
        SOURCE_ACCEPTED_WITH_EDIT => {
            if has_inserted_run(
                &normalize_words(raw),
                &revalidated_words,
                MIN_OMISSION_RUN_WORDS,
            ) {
                RevalidationOutcome::Reject {
                    reason: format!(
                        "the audio contains words the take's text never had \
                         (snippet pipeline drop); the user corrected text they \
                         never fully saw. whole-audio decode: {revalidated_text:?}"
                    ),
                }
            } else {
                RevalidationOutcome::Unchanged
            }
        }
        _ => RevalidationOutcome::Unchanged,
    }
}

/// Whether a decode is only Whisper noise annotations — bracketed or
/// parenthesised markers like "[BLANK_AUDIO]" or "(mouse clicking)" with no
/// words outside them. (Same rule the daemon applies to live snippets.)
fn is_noise_only(text: &str) -> bool {
    let mut depth = 0_u32;
    for character in text.chars() {
        match character {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            _ if character.is_alphanumeric() => return false,
            _ => {}
        }
    }
    true
}

/// Lower-cased alphanumeric word stream: decode variance in caps and
/// punctuation must not count as a difference.
fn normalize_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|character| character.is_alphanumeric())
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect()
}

/// Whether `revalidated` contains an OMISSION from the stored text: an
/// alignment gap where the whole-audio decode added at least `min_run` more
/// words than the stored text lost in that same gap. A substitution region
/// (a re-decode wording things differently: "wrestler zero zero one" vs
/// "ressler 001") loses about as many words as it adds and is variance; a
/// dropped phrase ("I don't want") is a pure addition and is not.
fn has_inserted_run(stored: &[String], revalidated: &[String], min_run: usize) -> bool {
    let stored_len = stored.len();
    let revalidated_len = revalidated.len();
    // Longest-common-subsequence table, suffix form.
    let mut table = vec![vec![0usize; revalidated_len + 1]; stored_len + 1];
    for i in (0..stored_len).rev() {
        for j in (0..revalidated_len).rev() {
            table[i][j] = if stored[i] == revalidated[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    fn gap_is_omission(inserted: usize, deleted: usize, min_run: usize) -> bool {
        inserted.saturating_sub(deleted) >= min_run
    }
    let (mut i, mut j) = (0usize, 0usize);
    let (mut deleted, mut inserted) = (0usize, 0usize);
    while i < stored_len && j < revalidated_len {
        if stored[i] == revalidated[j] {
            if gap_is_omission(inserted, deleted, min_run) {
                return true;
            }
            deleted = 0;
            inserted = 0;
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
            deleted += 1;
        } else {
            j += 1;
            inserted += 1;
        }
    }
    inserted += revalidated_len - j;
    deleted += stored_len - i;
    gap_is_omission(inserted, deleted, min_run)
}

#[derive(Debug, Serialize)]
pub struct RevalidationEntry {
    pub candidate_id: i64,
    pub action: String,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct RevalidationReport {
    pub scanned: usize,
    pub retranscribed: usize,
    pub rejected: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub applied: bool,
    pub entries: Vec<RevalidationEntry>,
}

#[derive(Debug)]
pub struct RevalidationError {
    message: String,
}

impl std::fmt::Display for RevalidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for RevalidationError {}

impl RevalidationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Re-decodes every candidate's stored audio and repairs/rejects records whose
/// text disagrees with it. `apply = false` reports without writing. Candidates
/// whose audio cannot be read or decoded are skipped (and reported), never
/// fatal: one missing file must not block cleaning the rest.
pub fn revalidate_user<F>(
    store: &mut SqliteMetadataStore,
    audio_store: &FileAudioStore,
    transcribe: F,
    user_id: &str,
    apply: bool,
) -> Result<RevalidationReport, RevalidationError>
where
    F: Fn(&AudioSegment) -> Result<String, String>,
{
    let candidates = store
        .training_candidates_for_manifest_v2(user_id)
        .map_err(|error| RevalidationError::new(format!("listing candidates: {error}")))?;

    let codec = OpusCodec::new();
    let mut report = RevalidationReport {
        scanned: 0,
        retranscribed: 0,
        rejected: 0,
        unchanged: 0,
        skipped: 0,
        applied: apply,
        entries: Vec::new(),
    };

    for candidate in candidates {
        report.scanned += 1;
        let audio_ref = AudioObjectRef {
            object_key: candidate.audio_object_key.clone(),
            codec_name: "opus".to_owned(),
            sample_rate_hz: 16_000,
            channels: 1,
        };
        let revalidated = audio_store
            .read_source_audio(&audio_ref)
            .map_err(|error| format!("reading audio: {error}"))
            .and_then(|encoded| {
                codec
                    .decode(&encoded)
                    .map_err(|error| format!("decoding audio: {error}"))
            })
            .and_then(|segment| transcribe(&segment));
        let revalidated = match revalidated {
            Ok(text) => text,
            Err(detail) => {
                report.skipped += 1;
                report.entries.push(RevalidationEntry {
                    candidate_id: candidate.training_candidate_id,
                    action: "skipped".to_owned(),
                    detail,
                });
                continue;
            }
        };

        match decide(
            &candidate.source_label,
            &candidate.raw_transcript,
            &candidate.corrected_transcript,
            &revalidated,
        ) {
            RevalidationOutcome::Unchanged => {
                report.unchanged += 1;
            }
            RevalidationOutcome::Retranscribe { text } => {
                if apply {
                    store
                        .retranscribe_training_candidate(candidate.training_candidate_id, &text)
                        .map_err(|error| {
                            RevalidationError::new(format!("retranscribing: {error}"))
                        })?;
                }
                report.retranscribed += 1;
                report.entries.push(RevalidationEntry {
                    candidate_id: candidate.training_candidate_id,
                    action: "retranscribed".to_owned(),
                    detail: format!("{:?} -> {:?}", candidate.corrected_transcript, text),
                });
            }
            RevalidationOutcome::Reject { reason } => {
                if apply {
                    store
                        .reject_training_candidate(candidate.training_candidate_id, &reason)
                        .map_err(|error| RevalidationError::new(format!("rejecting: {error}")))?;
                }
                report.rejected += 1;
                report.entries.push(RevalidationEntry {
                    candidate_id: candidate.training_candidate_id,
                    action: "rejected".to_owned(),
                    detail: reason,
                });
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::{decide, has_inserted_run, normalize_words, RevalidationOutcome};

    fn words(text: &str) -> Vec<String> {
        normalize_words(text)
    }

    #[test]
    fn normalization_ignores_case_and_punctuation() {
        assert_eq!(words(" Restart   traffic. "), vec!["restart", "traffic"]);
        assert_eq!(words("E2E!"), vec!["e2e"]);
    }

    #[test]
    fn pure_insertion_runs_are_detected_but_substitutions_are_not() {
        // The real poisoning case: "I don't want" present in audio, absent in text.
        assert!(has_inserted_run(
            &words("actually works side cars and all sorts of mess"),
            &words("actually works I don't want side cars and all sorts of mess"),
            2
        ));
        // One inserted word is decode variance, not an omission.
        assert!(!has_inserted_run(
            &words("traffic"),
            &words("restart traffic"),
            2
        ));
        // A multi-word SUBSTITUTION ("wrestler zero zero one" vs "ressler 001")
        // is re-decode variance: stored words are consumed against it, so it
        // must not count as an omission.
        assert!(!has_inserted_run(
            &words("updating the wrestler zero zero one server"),
            &words("updating the ressler 001 server"),
            2
        ));
        // Stored text sharing NOTHING with a longer decode: the gap adds two
        // words for the one it loses — still below the omission threshold
        // (full-mismatch single-word records are variance, not drops).
        assert!(!has_inserted_run(
            &words("deploy"),
            &words("restart traffic"),
            2
        ));
        // But text that matches nothing AND lost nothing is a pure addition.
        assert!(has_inserted_run(&words("."), &words("restart traffic"), 2));
    }

    #[test]
    fn unproofread_candidates_are_retranscribed_on_any_material_difference() {
        assert_eq!(
            decide(
                "accepted_without_edit",
                "side cars",
                "side cars",
                " I don't want side cars. "
            ),
            RevalidationOutcome::Retranscribe {
                text: "I don't want side cars.".to_owned()
            }
        );
        // Caps/punctuation variance alone is NOT material.
        assert_eq!(
            decide(
                "accepted_without_edit",
                "restart traffic",
                "restart traffic",
                " Restart traffic. "
            ),
            RevalidationOutcome::Unchanged
        );
    }

    #[test]
    fn user_corrections_stay_unless_the_user_never_saw_the_words() {
        // Gold correction over text that matches the audio: untouchable.
        assert_eq!(
            decide(
                "accepted_with_edit",
                "restart traffic",
                "restart Traefik",
                "Restart traffic."
            ),
            RevalidationOutcome::Unchanged
        );
        // The audio contains a dropped run the user never saw: untrainable.
        assert!(matches!(
            decide(
                "accepted_with_edit",
                "this actually works side cars",
                "this actually works side-cars",
                "this actually works I don't want side cars"
            ),
            RevalidationOutcome::Reject { .. }
        ));
    }

    #[test]
    fn silent_audio_rejects_the_candidate() {
        assert!(matches!(
            decide("accepted_without_edit", "words", "words", "  "),
            RevalidationOutcome::Reject { .. }
        ));
        // A decode that is ONLY noise annotations is silence with a costume on:
        // "(mouse clicking)" must never become a training label.
        assert!(matches!(
            decide(
                "accepted_without_edit",
                "nothing",
                "nothing",
                "(mouse clicking)"
            ),
            RevalidationOutcome::Reject { .. }
        ));
    }
}
