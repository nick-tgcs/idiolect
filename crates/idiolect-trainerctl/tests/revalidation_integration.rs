//! Integration contract for training-candidate revalidation: a real SQLite
//! store, a real `FileAudioStore` with real Opus-encoded audio, and the real
//! bundled Whisper fixture model re-transcribing it.
//!
//! Why this exists: the snippet pipeline used to drop words at pause
//! boundaries, so a candidate's stored text can omit words its stored audio
//! provably contains — a pair that TEACHES the model to skip words.
//! Revalidation re-decodes every candidate's audio whole and:
//!   - `accepted_without_edit`: replaces the stored text with the full decode
//!     (the user never proof-read it; the audio is the truth),
//!   - `accepted_with_edit`: rejects the candidate when the audio contains
//!     word runs the user-visible text never had (the correction was made
//!     against text the user never fully saw — the label can't be trusted),
//!   - leaves trustworthy candidates untouched.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use idiolect_adapter_opus::OpusCodec;
use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};
use idiolect_adapter_whisper::WhisperAsr;
use idiolect_ports::asr::AsrPort;
use idiolect_ports::codec::AudioCodecPort;
use idiolect_ports::storage::{AudioStorePort, MetadataStorePort};
use idiolect_test_support::fixtures::restart_traffic_fixture_16khz_mono;
use idiolect_trainerctl::revalidate_user;

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock");
        let root = env::temp_dir().join(format!(
            "idiolect-revalidation-{tag}-{}-{}",
            std::process::id(),
            now.as_nanos()
        ));
        fs::create_dir_all(&root).expect("fixture root");
        Self { root }
    }

    fn open_store(&self) -> SqliteMetadataStore {
        let mut store = SqliteMetadataStore::open_path(self.root.join("idiolect.sqlite"))
            .expect("store should open");
        store.migrate().expect("store should migrate");
        store
    }

    fn audio_store(&self) -> FileAudioStore {
        FileAudioStore::new(self.root.join("audio"), self.root.join("decoded"))
    }
}

/// Commits a session (raw → committed decides the candidate's source label)
/// and stores the restart-traffic fixture clip as its audio.
fn seed_candidate(
    store: &mut SqliteMetadataStore,
    audio_store: &FileAudioStore,
    raw: &str,
    committed: &str,
) {
    let session_id = store
        .create_session(Some(raw))
        .expect("session should be created");
    store
        .commit_session(session_id, committed, &format!("commit-{raw}-{committed}"))
        .expect("session should commit");
    let utterance_id = store
        .session_utterance_link_for_test(session_id)
        .expect("link should query")
        .expect("session should have an utterance")
        .utterance_id;
    let encoded = OpusCodec::new()
        .encode(&restart_traffic_fixture_16khz_mono())
        .expect("fixture should encode");
    audio_store
        .write_source_audio("default", &utterance_id, &encoded)
        .expect("audio should store");
}

fn fixture_transcriber() -> impl Fn(&idiolect_ports::audio::AudioSegment) -> Result<String, String> {
    let asr = WhisperAsr::load_fixture_model().expect("bundled fixture model should load");
    move |segment| {
        asr.transcribe(segment)
            .map(|draft| draft.text)
            .map_err(|error| error.to_string())
    }
}

#[test]
fn revalidation_repairs_rejects_and_keeps_the_right_candidates() {
    let fixture = Fixture::new("apply");
    let mut store = fixture.open_store();
    let audio_store = fixture.audio_store();

    // A: pipeline-dropped words, never seen by the user (accepted unchanged):
    // stored text "traffic", audio says "restart traffic" → repaired in place.
    seed_candidate(&mut store, &audio_store, "traffic", "traffic");
    // B: the pipeline dropped EVERYTHING (raw text is punctuation only) and
    // the user then "corrected" what they could see: the audio's words were
    // never in front of them → the label is untrustworthy.
    seed_candidate(&mut store, &audio_store, ".", "deploy nginx");
    // C: trustworthy gold: raw matches the audio, the user's correction stands.
    seed_candidate(&mut store, &audio_store, "restart traffic", "restart Traefik");

    let report = revalidate_user(
        &mut store,
        &audio_store,
        fixture_transcriber(),
        "default",
        true,
    )
    .expect("revalidation should run");

    assert_eq!(report.scanned, 3, "{report:?}");
    assert_eq!(report.retranscribed, 1, "{report:?}");
    assert_eq!(report.rejected, 1, "{report:?}");
    assert_eq!(report.unchanged, 1, "{report:?}");
    assert_eq!(report.skipped, 0, "{report:?}");

    let feed = store
        .training_candidates_for_manifest_v2("default")
        .expect("manifest feed should read");
    assert_eq!(feed.len(), 2, "the poisoned correction left the feed: {feed:?}");
    let repaired = &feed[0];
    let lowered = repaired.corrected_transcript.to_lowercase();
    assert!(
        lowered.contains("restart") && lowered.contains("traffic"),
        "the repaired label carries the words the audio contains: {repaired:?}"
    );
    assert_eq!(
        feed[1].corrected_transcript, "restart Traefik",
        "the trustworthy user correction is untouched"
    );
}

#[test]
fn a_dry_run_reports_but_writes_nothing() {
    let fixture = Fixture::new("dry");
    let mut store = fixture.open_store();
    let audio_store = fixture.audio_store();

    seed_candidate(&mut store, &audio_store, "traffic", "traffic");

    let report = revalidate_user(
        &mut store,
        &audio_store,
        fixture_transcriber(),
        "default",
        false,
    )
    .expect("dry run should run");
    assert_eq!(report.retranscribed, 1, "the dry run still reports the repair");

    let feed = store
        .training_candidates_for_manifest_v2("default")
        .expect("manifest feed should read");
    assert_eq!(
        feed[0].corrected_transcript, "traffic",
        "a dry run must not write anything"
    );
}
