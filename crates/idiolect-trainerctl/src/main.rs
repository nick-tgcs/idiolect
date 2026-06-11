//! Trainer control binary. `revalidate` re-decodes every training candidate's
//! stored audio and repairs/rejects records whose text disagrees with it (see
//! the `revalidate` module for the rules). Dry-run by default; `--apply`
//! writes. The Whisper model defaults to the bundled fixture model; pass
//! `--model <path>` (and `--gpu`) to revalidate with the production model.

use idiolect_adapter_sqlite::{FileAudioStore, SqliteMetadataStore};
use idiolect_adapter_whisper::{WhisperAsr, WhisperOptions};
use idiolect_ports::asr::AsrPort;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((command, rest)) if command == "revalidate" => match run_revalidate(rest) {
            Ok(output) => println!("{output}"),
            Err(message) => {
                eprintln!("revalidate failed: {message}");
                std::process::exit(1);
            }
        },
        Some((command, rest)) if command == "train" => match run_train_cli(rest) {
            Ok(output) => println!("{output}"),
            Err(message) => {
                eprintln!("train failed: {message}");
                std::process::exit(1);
            }
        },
        _ => println!("{}", idiolect_trainerctl::crate_name()),
    }
}

struct RevalidateFlags {
    db: Option<String>,
    audio_root: Option<String>,
    user: String,
    model: Option<String>,
    gpu: bool,
    apply: bool,
    json: bool,
}

fn parse_flags(args: &[String]) -> Result<RevalidateFlags, String> {
    let mut flags = RevalidateFlags {
        db: None,
        audio_root: None,
        user: "default".to_owned(),
        model: None,
        gpu: false,
        apply: false,
        json: false,
    };
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--db" => flags.db = Some(value(&mut iter, "--db")?),
            "--audio-root" => flags.audio_root = Some(value(&mut iter, "--audio-root")?),
            "--user" => flags.user = value(&mut iter, "--user")?,
            "--model" => flags.model = Some(value(&mut iter, "--model")?),
            "--gpu" => flags.gpu = true,
            "--apply" => flags.apply = true,
            "--json" => flags.json = true,
            other => return Err(format!("unknown flag: {other}")),
        }
    }
    Ok(flags)
}

fn value(iter: &mut std::slice::Iter<'_, String>, flag: &str) -> Result<String, String> {
    iter.next()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn run_train_cli(args: &[String]) -> Result<String, String> {
    let mut flags = idiolect_trainerctl::train_command::TrainFlags {
        db: String::new(),
        audio_root: String::new(),
        user: "default".to_owned(),
        base_model: String::new(),
        output: String::new(),
        epochs: 2,
        learning_rate: 1e-3,
        rank: 8,
        max_samples: None,
        gpu: false,
    };
    let mut iter = args.iter();
    while let Some(flag) = iter.next() {
        match flag.as_str() {
            "--db" => flags.db = value(&mut iter, "--db")?,
            "--audio-root" => flags.audio_root = value(&mut iter, "--audio-root")?,
            "--user" => flags.user = value(&mut iter, "--user")?,
            "--base-model" => flags.base_model = value(&mut iter, "--base-model")?,
            "--output" => flags.output = value(&mut iter, "--output")?,
            "--epochs" => {
                flags.epochs = value(&mut iter, "--epochs")?
                    .parse()
                    .map_err(|_| "--epochs needs a number".to_owned())?;
            }
            "--lr" => {
                flags.learning_rate = value(&mut iter, "--lr")?
                    .parse()
                    .map_err(|_| "--lr needs a number".to_owned())?;
            }
            "--rank" => {
                flags.rank = value(&mut iter, "--rank")?
                    .parse()
                    .map_err(|_| "--rank needs a number".to_owned())?;
            }
            "--max-samples" => {
                flags.max_samples = Some(
                    value(&mut iter, "--max-samples")?
                        .parse()
                        .map_err(|_| "--max-samples needs a number".to_owned())?,
                );
            }
            "--gpu" => flags.gpu = true,
            other => return Err(format!("unknown flag: {other}")),
        }
    }
    for (name, field) in [
        ("--db", &flags.db),
        ("--audio-root", &flags.audio_root),
        ("--base-model", &flags.base_model),
        ("--output", &flags.output),
    ] {
        if field.is_empty() {
            return Err(format!("{name} is required"));
        }
    }
    let report = idiolect_trainerctl::train_command::run_train(&flags)?;
    serde_json::to_string_pretty(&report).map_err(|error| error.to_string())
}

fn run_revalidate(args: &[String]) -> Result<String, String> {
    let flags = parse_flags(args)?;
    let db = flags.db.ok_or("--db is required")?;
    let audio_root = flags.audio_root.ok_or("--audio-root is required")?;

    let mut store = SqliteMetadataStore::open_path(&db).map_err(|error| error.to_string())?;
    store.migrate().map_err(|error| error.to_string())?;
    let audio_root = std::path::PathBuf::from(audio_root);
    let decoded_cache = audio_root.with_file_name("decoded-cache");
    let audio_store = FileAudioStore::new(audio_root, decoded_cache);

    let asr = match &flags.model {
        Some(model) => WhisperAsr::load(
            model,
            WhisperOptions {
                use_gpu: flags.gpu,
                ..WhisperOptions::default()
            },
        )
        .map_err(|error| error.to_string())?,
        None => WhisperAsr::load_fixture_model().map_err(|error| error.to_string())?,
    };
    let transcribe = |segment: &idiolect_ports::audio::AudioSegment| {
        asr.transcribe(segment)
            .map(|draft| draft.text)
            .map_err(|error| error.to_string())
    };

    let report = idiolect_trainerctl::revalidate_user(
        &mut store,
        &audio_store,
        transcribe,
        &flags.user,
        flags.apply,
    )
    .map_err(|error| error.to_string())?;

    if flags.json {
        serde_json::to_string_pretty(&report).map_err(|error| error.to_string())
    } else {
        let mut lines = vec![format!(
            "scanned {} candidate(s): {} retranscribed, {} rejected, {} unchanged, {} skipped{}",
            report.scanned,
            report.retranscribed,
            report.rejected,
            report.unchanged,
            report.skipped,
            if report.applied { " (applied)" } else { " (dry run — pass --apply to write)" },
        )];
        for entry in &report.entries {
            lines.push(format!(
                "  #{} {}: {}",
                entry.candidate_id, entry.action, entry.detail
            ));
        }
        Ok(lines.join("\n"))
    }
}
