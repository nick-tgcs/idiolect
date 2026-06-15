//! Shared streaming-take text logic — the pure decisions that turn live snippet
//! decodes and a stop-time whole-take decode into the one string the take stores.
//!
//! Lifted out of the desktop `run_loop` (M2) so the **same** rules govern the
//! Android path: snippet decodes are short-context previews that drop words at
//! pause boundaries, so the whole-recording decode at stop is authoritative — but
//! only when it heard usable words, else the previewed text wins (a real take
//! lost "I don't want" to a snippet-only path; this module is that guardrail).
//! These are pure functions; the capture/VAD/segmenter plumbing stays in the
//! daemon (and the future Android audio adapter) and feeds them.

/// Whether a transcription is only non-speech markers. Whisper labels noise with
/// bracketed/parenthesised annotations — `[BLANK_AUDIO]`, `(knocking)`,
/// `[silence]` — and a snippet that contains nothing else (a knock, a cough, a
/// breath that fooled the VAD) must be dropped, not typed into the document.
#[must_use]
pub fn is_noise_transcript(text: &str) -> bool {
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

/// The text a snippet contributes to the take: trimmed (Whisper pads snippet
/// decodes with whitespace, which would double the join space — a real take
/// stored "…  …" this way), carrying its joining space when the take already has
/// text. `None` when the trimmed decode is empty or noise-only markers — nothing
/// worth typing.
#[must_use]
pub fn snippet_chunk(take_so_far: &str, decoded: &str) -> Option<String> {
    let text = decoded.trim();
    if text.is_empty() || is_noise_transcript(text) {
        return None;
    }
    Some(if take_so_far.is_empty() {
        text.to_owned()
    } else {
        format!(" {text}")
    })
}

/// Picks the take's final text at stop: the decode of the WHOLE recording when it
/// produced usable words (short-context snippet decodes drop words at pause
/// boundaries — a real take lost "I don't want" that way), otherwise the glued
/// snippet previews — never lose text the user already saw typed.
#[must_use]
pub fn choose_final_take_text(full_decode: String, previewed: String) -> String {
    let text = full_decode.trim();
    if text.is_empty() || is_noise_transcript(text) {
        previewed
    } else {
        text.to_owned()
    }
}

/// Applies an engine-reported in-place correction to a take's text. The engine's
/// correction window only ever tracks the take's final snippet (`tail`), so when
/// the take's text ends with that snippet, the correction replaces just that
/// suffix. Batch takes (`tail` = None) — where the window held the whole
/// transcript — replace the full text. A tail that no longer suffixes the text
/// (the stored text is the stop-time whole-recording decode, which can differ
/// from the last snippet preview) keeps the take unchanged: one snippet's
/// correction must never overwrite the whole take.
#[must_use]
pub fn merge_tail_correction(current: &str, tail: Option<&str>, corrected: &str) -> String {
    match tail {
        Some(tail) if current.ends_with(tail) => {
            format!("{}{corrected}", &current[..current.len() - tail.len()])
        }
        Some(_) => current.to_owned(),
        None => corrected.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        choose_final_take_text, is_noise_transcript, merge_tail_correction, snippet_chunk,
    };

    #[test]
    fn snippet_chunks_are_trimmed_and_joined_with_one_space() {
        // First snippet: no joining space.
        assert_eq!(
            snippet_chunk("", "restart traffic"),
            Some("restart traffic".to_owned())
        );
        // Later snippets carry exactly one joining space — even when the decode
        // arrives whitespace-padded (a real take stored a double space because a
        // padded decode was glued verbatim).
        assert_eq!(
            snippet_chunk("take", " so far "),
            Some(" so far".to_owned())
        );
        // Empty or whitespace-only decodes contribute nothing — no bare space.
        assert_eq!(snippet_chunk("take", ""), None);
        assert_eq!(snippet_chunk("take", "   "), None);
        // Noise-only markers contribute nothing.
        assert_eq!(snippet_chunk("take", "[BLANK_AUDIO]"), None);
    }

    #[test]
    fn the_stop_time_decode_wins_unless_it_heard_nothing() {
        // Usable whole-recording decode: authoritative (and trimmed).
        assert_eq!(
            choose_final_take_text(" the full take ".to_owned(), "the full".to_owned()),
            "the full take"
        );
        // Empty or noise-only decode: keep the previewed snippet text — never lose
        // words the user already saw typed.
        assert_eq!(
            choose_final_take_text(String::new(), "previewed".to_owned()),
            "previewed"
        );
        assert_eq!(
            choose_final_take_text("[BLANK_AUDIO]".to_owned(), "previewed".to_owned()),
            "previewed"
        );
    }

    #[test]
    fn tail_corrections_replace_only_the_final_snippet() {
        // Streamed take: the engine's correction window held only the last
        // snippet, so the fix lands on that suffix of the merged string.
        assert_eq!(
            merge_tail_correction(
                "restart traffic deploy nginx",
                Some("deploy nginx"),
                "deploy Nginx"
            ),
            "restart traffic deploy Nginx"
        );
        // Batch take: the window held the whole transcript.
        assert_eq!(
            merge_tail_correction("restart traffic", None, "restart Traefik"),
            "restart Traefik"
        );
        // A tail that no longer suffixes the text: the take's text may be the
        // stop-time whole-recording decode, which can legitimately differ from the
        // last snippet preview the engine's window held. Replacing the whole take
        // with one snippet's correction would destroy it — keep the take and drop
        // the unplaceable correction instead.
        assert_eq!(
            merge_tail_correction("something else", Some("deploy nginx"), "deploy Nginx"),
            "something else"
        );
    }

    #[test]
    fn noise_only_transcripts_are_recognised() {
        // Whisper labels non-speech with bracketed/parenthesised markers; a
        // snippet that is ONLY such markers (a knock, a breath) must be dropped,
        // not typed into the user's document.
        assert!(is_noise_transcript("[BLANK_AUDIO]"));
        assert!(is_noise_transcript("(knocking)"));
        assert!(is_noise_transcript(" [silence] (keyboard clacking) "));
        assert!(is_noise_transcript(""));
        assert!(is_noise_transcript("  ."));
        // Real words survive, even alongside a marker.
        assert!(!is_noise_transcript("restart traffic"));
        assert!(!is_noise_transcript("(sighs) restart traffic"));
    }
}
