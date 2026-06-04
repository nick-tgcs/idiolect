use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use idiolect_adapter_opus::OpusCodec;
use idiolect_adapter_sqlite::FileAudioStore;
use idiolect_ports::codec::AudioCodecPort;
use idiolect_ports::storage::{AudioRetentionMode, AudioStorePort};
use idiolect_test_support::fixtures::restart_traffic_fixture_16khz_mono;

#[test]
fn opus_audio_file_is_written_and_reopened_for_training() {
    let root = unique_temp_dir("reopen");
    let store = FileAudioStore::new(root.join("audio"), root.join("decoded"));
    let codec = OpusCodec::new();
    let segment = restart_traffic_fixture_16khz_mono();
    let encoded = codec.encode(&segment).expect("fixture should encode");

    let audio_ref = store
        .write_source_audio("default", "utt-reopen", &encoded)
        .expect("encoded audio should be written");

    assert_eq!(audio_ref.codec_name, "opus");
    assert!(store
        .source_audio_exists_for_test(&audio_ref)
        .expect("source existence should query"));
    assert_eq!(
        store
            .source_payload_for_test(&audio_ref)
            .expect("source payload should read"),
        encoded.payload
    );

    let reopened = store
        .read_source_audio(&audio_ref)
        .expect("encoded audio should reopen");
    let decoded = codec
        .decode(&reopened)
        .expect("reopened audio should decode");

    assert_eq!(decoded.sample_rate_hz, segment.sample_rate_hz);
    assert_eq!(decoded.channels, segment.channels);
    assert_eq!(decoded.duration_ms, segment.duration_ms);
    assert_eq!(
        decoded.samples_f32_mono.len(),
        segment.samples_f32_mono.len()
    );

    cleanup_dir(root);
}

#[test]
fn decoded_cache_is_deleted_on_privacy_delete() {
    let root = unique_temp_dir("privacy-delete");
    let store = FileAudioStore::new(root.join("audio"), root.join("decoded"));
    let codec = OpusCodec::new();
    let segment = restart_traffic_fixture_16khz_mono();
    let encoded = codec.encode(&segment).expect("fixture should encode");
    let audio_ref = store
        .write_source_audio("default", "utt-private", &encoded)
        .expect("encoded audio should be written");
    let cache_ref = store
        .write_decoded_cache("default", "utt-private", &segment)
        .expect("decoded cache should be written");

    assert!(store
        .source_audio_exists_for_test(&audio_ref)
        .expect("source existence should query"));
    assert!(store
        .decoded_cache_exists_for_test(&cache_ref)
        .expect("decoded cache existence should query"));

    store
        .privacy_delete_user("default")
        .expect("privacy delete should remove audio and decoded cache");

    assert!(!store
        .source_audio_exists_for_test(&audio_ref)
        .expect("source existence should query"));
    assert!(!store
        .decoded_cache_exists_for_test(&cache_ref)
        .expect("decoded cache existence should query"));

    cleanup_dir(root);
}

#[test]
fn retention_minimal_deletes_audio_after_classification() {
    let root = unique_temp_dir("minimal-retention");
    let store = FileAudioStore::new(root.join("audio"), root.join("decoded"));
    let codec = OpusCodec::new();
    let segment = restart_traffic_fixture_16khz_mono();
    let encoded = codec.encode(&segment).expect("fixture should encode");
    let audio_ref = store
        .write_source_audio("default", "utt-minimal", &encoded)
        .expect("encoded audio should be written");

    assert!(store
        .source_audio_exists_for_test(&audio_ref)
        .expect("source existence should query"));

    store
        .apply_retention(&audio_ref, AudioRetentionMode::Minimal)
        .expect("minimal retention should delete source audio");

    assert!(!store
        .source_audio_exists_for_test(&audio_ref)
        .expect("source existence should query"));

    cleanup_dir(root);
}

#[test]
fn audio_store_rejects_symlink_escape_from_user_audio_dir() {
    let root = unique_temp_dir("symlink-escape");
    let audio_root = root.join("audio");
    let outside_root = root.join("outside");
    let dated_audio_root = audio_root.join("1970").join("01").join("01");
    fs::create_dir_all(&dated_audio_root).expect("dated audio root should be created");
    fs::create_dir_all(&outside_root).expect("outside root should be created");
    std::os::unix::fs::symlink(&outside_root, dated_audio_root.join("default"))
        .expect("symlink should be created");

    let store = FileAudioStore::new(audio_root, root.join("decoded"));
    let codec = OpusCodec::new();
    let segment = restart_traffic_fixture_16khz_mono();
    let encoded = codec.encode(&segment).expect("fixture should encode");

    let result = store.write_source_audio("default", "utt-symlink", &encoded);

    assert!(result.is_err());
    assert!(!outside_root.join("utt-symlink.ogg").exists());

    cleanup_dir(root);
}

#[test]
fn audio_store_rejects_symlink_escape_from_source_file() {
    let root = unique_temp_dir("symlink-file-escape");
    let audio_root = root.join("audio");
    let user_audio_root = audio_root
        .join("1970")
        .join("01")
        .join("01")
        .join("default");
    let outside_root = root.join("outside");
    let victim = outside_root.join("victim.ogg");
    fs::create_dir_all(&user_audio_root).expect("user audio root should be created");
    fs::create_dir_all(&outside_root).expect("outside root should be created");
    fs::write(&victim, b"do-not-touch").expect("victim should be written");
    std::os::unix::fs::symlink(&victim, user_audio_root.join("utt-symlink-file.ogg"))
        .expect("source symlink should be created");

    let store = FileAudioStore::new(audio_root, root.join("decoded"));
    let codec = OpusCodec::new();
    let segment = restart_traffic_fixture_16khz_mono();
    let encoded = codec.encode(&segment).expect("fixture should encode");

    let result = store.write_source_audio("default", "utt-symlink-file", &encoded);

    assert!(result.is_err());
    assert_eq!(
        fs::read(&victim).expect("victim should remain readable"),
        b"do-not-touch"
    );

    cleanup_dir(root);
}

#[test]
fn privacy_delete_rejects_dot_user_identifier() {
    let root = unique_temp_dir("dot-user-delete");
    let store = FileAudioStore::new(root.join("audio"), root.join("decoded"));
    fs::create_dir_all(root.join("audio").join("1970").join("01").join("01"))
        .expect("dated audio root should be created");
    fs::create_dir_all(root.join("decoded")).expect("decoded root should be created");

    let result = store.privacy_delete_user(".");

    assert!(result.is_err());
    assert!(root
        .join("audio")
        .join("1970")
        .join("01")
        .join("01")
        .exists());
    assert!(root.join("decoded").exists());

    cleanup_dir(root);
}

#[test]
fn privacy_delete_rejects_symlink_escape_from_decoded_cache_ancestor() {
    let root = unique_temp_dir("privacy-delete-symlink-ancestor");
    let decoded_root = root.join("decoded");
    let outside_root = root.join("outside");
    let victim = outside_root.join("default");
    fs::create_dir_all(&outside_root).expect("outside root should be created");
    fs::create_dir_all(&victim).expect("victim dir should be created");
    fs::write(victim.join("keep.pcmf32"), b"do-not-delete").expect("victim file should write");
    std::os::unix::fs::symlink(&outside_root, &decoded_root)
        .expect("decoded root symlink should be created");

    let store = FileAudioStore::new(root.join("audio"), decoded_root);

    let result = store.privacy_delete_user("default");

    assert!(result.is_err());
    assert!(victim.join("keep.pcmf32").exists());

    cleanup_dir(root);
}

fn unique_temp_dir(tag: &str) -> PathBuf {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock");
    let root = env::temp_dir().join(format!(
        "idiolect-audio-store-{tag}-{}-{}",
        std::process::id(),
        now.as_nanos()
    ));
    fs::create_dir_all(&root).expect("temp dir should be created");
    root
}

fn cleanup_dir(path: PathBuf) {
    let _ = fs::remove_dir_all(path);
}
