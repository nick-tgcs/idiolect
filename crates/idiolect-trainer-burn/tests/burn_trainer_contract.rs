use std::convert::Infallible;

use idiolect_ml_core::{TrainingArtifact, TrainingConfig, TrainingManifest, TrainingManifestItem};
use idiolect_ports::trainer::TrainerPort;
use idiolect_trainer_burn::{BurnTrainer, BurnTrainerError};

#[test]
fn burn_trainer_consumes_manifest_and_emits_candidate_artifact() {
    let trainer = BurnTrainer::new();
    assert_candidate_artifact_contract(&trainer, "candidate-adapter-001");
}

#[test]
fn fake_trainer_contract_matches_burn_trainer_contract() {
    let trainer = FakeTrainer;
    assert_candidate_artifact_contract(&trainer, "fake-adapter-001");
}

#[test]
fn candidate_artifact_records_base_model_manifest_and_backend() {
    let trainer = BurnTrainer::new();
    let artifact = trainer
        .train(
            fixture_manifest(),
            TrainingConfig::new("candidate-adapter-002"),
        )
        .expect("trainer should produce artifact");

    assert_eq!(artifact.base_model_id(), "whisper-medium-en");
    assert_eq!(artifact.manifest_digest(), "manifest-v2-digest");
    assert_eq!(artifact.backend_id(), "burn-ndarray-0.13.2");
}

#[test]
fn trainer_rejects_manifest_without_audio() {
    let trainer = BurnTrainer::new();
    let manifest = TrainingManifest::new(
        "manifest-v2-digest",
        "whisper-medium-en",
        vec![TrainingManifestItem::new("", "", "restart Traefik")],
    );

    let error = trainer
        .train(manifest, TrainingConfig::new("candidate-adapter-003"))
        .expect_err("missing audio should fail");

    assert_eq!(error, BurnTrainerError::MissingAudio { item_index: 0 });
}

fn assert_candidate_artifact_contract<T>(trainer: &T, candidate_id: &str)
where
    T: TrainerPort,
    T::Error: std::fmt::Debug,
{
    let artifact = trainer
        .train(fixture_manifest(), TrainingConfig::new(candidate_id))
        .expect("trainer should produce artifact");

    assert_eq!(artifact.manifest_digest(), "manifest-v2-digest");
    assert_eq!(artifact.candidate_id(), candidate_id);
    assert_eq!(artifact.digest().len(), 64);
}

struct FakeTrainer;

impl TrainerPort for FakeTrainer {
    type Error = Infallible;

    fn train(
        &self,
        manifest: TrainingManifest,
        config: TrainingConfig,
    ) -> Result<TrainingArtifact, Self::Error> {
        Ok(TrainingArtifact::new(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            manifest.digest(),
            manifest.base_model_id(),
            "fake-trainer",
            config.candidate_id(),
        ))
    }
}

fn fixture_manifest() -> TrainingManifest {
    TrainingManifest::new(
        "manifest-v2-digest",
        "whisper-medium-en",
        vec![TrainingManifestItem::new(
            "audio/1970/01/01/default/u001.ogg",
            "audio-digest-001",
            "restart Traefik",
        )],
    )
}
