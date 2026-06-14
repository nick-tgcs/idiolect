//! Contract for training-candidate revalidation writes: re-transcribing a
//! candidate from its stored audio must update EVERY stored copy of its text
//! (candidate raw/corrected/transcript AND the utterance's raw STT text — the
//! manifest feed coalesces from both), and rejecting a candidate must remove
//! it from the manifest feed so a poisoned pair can never be trained on.

use idiolect_adapter_sqlite::SqliteMetadataStore;
use idiolect_common::ids::ImeSessionId;
use idiolect_ports::storage::MetadataStorePort;

fn migrated_store() -> SqliteMetadataStore {
    let mut store = SqliteMetadataStore::open_in_memory().expect("store should open");
    store.migrate().expect("migration should apply");
    store
}

fn seed_committed(store: &mut SqliteMetadataStore, raw: &str, committed: &str) -> ImeSessionId {
    let session_id = store
        .create_session(Some(raw))
        .expect("session should be created");
    store
        .commit_session(session_id, committed, &format!("commit-{raw}-{committed}"))
        .expect("session should commit");
    session_id
}

#[test]
fn retranscribing_a_candidate_rewrites_every_stored_text_copy() {
    let mut store = migrated_store();
    // A streamed take whose snippet pipeline dropped words: stored text says
    // "side cars" while the audio provably contains "I don't want side cars".
    seed_committed(&mut store, "side cars", "side cars");

    let before = store
        .training_candidates_for_manifest_v2("default")
        .expect("manifest feed should read");
    assert_eq!(before.len(), 1);
    let candidate_id = before[0].training_candidate_id;

    store
        .retranscribe_training_candidate(candidate_id, "I don't want side cars")
        .expect("retranscription should persist");

    let after = store
        .training_candidates_for_manifest_v2("default")
        .expect("manifest feed should read");
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].raw_transcript, "I don't want side cars",
        "the manifest's raw transcript (coalesced from the utterance) must carry the recovered words"
    );
    assert_eq!(
        after[0].corrected_transcript, "I don't want side cars",
        "the trainable label must carry the recovered words"
    );
}

#[test]
fn rejected_candidates_leave_the_manifest_feed() {
    let mut store = migrated_store();
    seed_committed(&mut store, "restart traffic", "restart traffic");
    seed_committed(&mut store, "deploy", "deploy!");

    let before = store
        .training_candidates_for_manifest_v2("default")
        .expect("manifest feed should read");
    assert_eq!(before.len(), 2);
    let poisoned_id = before[1].training_candidate_id;

    store
        .reject_training_candidate(
            poisoned_id,
            "audio contains words the stored text never had",
        )
        .expect("rejection should persist");

    let after = store
        .training_candidates_for_manifest_v2("default")
        .expect("manifest feed should read");
    assert_eq!(
        after.len(),
        1,
        "the rejected candidate must not be trainable"
    );
    assert_eq!(
        after[0].training_candidate_id,
        before[0].training_candidate_id
    );
}
