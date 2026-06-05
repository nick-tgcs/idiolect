use std::error::Error;
use std::fmt::{Display, Formatter};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use idiolect_ports::audio::{AudioSegment, EncodedAudio};
use idiolect_ports::storage::{
    AudioObjectRef, AudioRetentionMode, AudioStorePort, DecodedAudioCacheRef,
};

const DECODED_MAGIC: &[u8] = b"IDL_SEG_PCMF";
const AUDIO_KEY_PREFIX: &str = "audio/";
const DECODED_KEY_PREFIX: &str = "decoded/";
const FIXTURE_DATE_PATH: &str = "1970/01/01";

#[derive(Debug)]
pub struct FileAudioStore {
    audio_root: PathBuf,
    decoded_cache_root: PathBuf,
}

#[derive(Debug)]
pub enum FileAudioStoreError {
    InvalidIdentifier {
        kind: &'static str,
        value: String,
    },
    InvalidObjectKey {
        object_key: String,
    },
    Io {
        op: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    IntegerOverflow(&'static str),
    UnsafePath {
        path: PathBuf,
    },
}

impl Display for FileAudioStoreError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidIdentifier { kind, value } => {
                write!(f, "invalid {kind} identifier: {value}")
            }
            Self::InvalidObjectKey { object_key } => {
                write!(f, "invalid audio object key: {object_key}")
            }
            Self::Io { op, path, source } => {
                write!(f, "{op} failed for {}: {source}", path.display())
            }
            Self::IntegerOverflow(field) => {
                write!(f, "integer overflow while encoding field: {field}")
            }
            Self::UnsafePath { path } => {
                write!(f, "unsafe audio store path: {}", path.display())
            }
        }
    }
}

impl Error for FileAudioStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl FileAudioStore {
    #[must_use]
    pub fn new(audio_root: PathBuf, decoded_cache_root: PathBuf) -> Self {
        Self {
            audio_root,
            decoded_cache_root,
        }
    }

    pub fn source_audio_exists_for_test(
        &self,
        audio_ref: &AudioObjectRef,
    ) -> Result<bool, FileAudioStoreError> {
        Ok(self.source_path_from_ref(audio_ref)?.exists())
    }

    pub fn source_payload_for_test(
        &self,
        audio_ref: &AudioObjectRef,
    ) -> Result<Vec<u8>, FileAudioStoreError> {
        self.read_source_payload(audio_ref)
    }

    pub fn decoded_cache_exists_for_test(
        &self,
        cache_ref: &DecodedAudioCacheRef,
    ) -> Result<bool, FileAudioStoreError> {
        Ok(self.decoded_cache_path_from_ref(cache_ref)?.exists())
    }

    fn validate_identifier(value: &str, kind: &'static str) -> Result<(), FileAudioStoreError> {
        if value.is_empty()
            || value == "."
            || value.contains('/')
            || value.contains("\\")
            || value.contains("..")
        {
            return Err(FileAudioStoreError::InvalidIdentifier {
                kind,
                value: value.to_owned(),
            });
        }

        Ok(())
    }

    fn source_object_key(user_id: &str, utterance_id: &str) -> Result<String, FileAudioStoreError> {
        Self::validate_identifier(user_id, "user_id")?;
        Self::validate_identifier(utterance_id, "utterance_id")?;
        Ok(format!(
            "{AUDIO_KEY_PREFIX}{FIXTURE_DATE_PATH}/{user_id}/{utterance_id}.ogg"
        ))
    }

    fn decoded_cache_object_key(
        user_id: &str,
        utterance_id: &str,
    ) -> Result<String, FileAudioStoreError> {
        Self::validate_identifier(user_id, "user_id")?;
        Self::validate_identifier(utterance_id, "utterance_id")?;
        Ok(format!(
            "{DECODED_KEY_PREFIX}{user_id}/{utterance_id}.pcmf32"
        ))
    }

    fn relative_key<'a>(
        object_key: &'a str,
        prefix: &'static str,
    ) -> Result<&'a str, FileAudioStoreError> {
        let Some(relative) = object_key.strip_prefix(prefix) else {
            return Err(FileAudioStoreError::InvalidObjectKey {
                object_key: object_key.to_owned(),
            });
        };
        if relative.is_empty()
            || relative.starts_with('/')
            || relative.contains('\\')
            || relative
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(FileAudioStoreError::InvalidObjectKey {
                object_key: object_key.to_owned(),
            });
        }
        Ok(relative)
    }

    fn path_from_key(
        root: &Path,
        object_key: &str,
        prefix: &'static str,
    ) -> Result<PathBuf, FileAudioStoreError> {
        Ok(root.join(Self::relative_key(object_key, prefix)?))
    }

    fn source_path_from_ref(
        &self,
        audio_ref: &AudioObjectRef,
    ) -> Result<PathBuf, FileAudioStoreError> {
        Self::path_from_key(&self.audio_root, &audio_ref.object_key, AUDIO_KEY_PREFIX)
    }

    fn decoded_cache_path_from_ref(
        &self,
        cache_ref: &DecodedAudioCacheRef,
    ) -> Result<PathBuf, FileAudioStoreError> {
        Self::path_from_key(
            &self.decoded_cache_root,
            &cache_ref.object_key,
            DECODED_KEY_PREFIX,
        )
    }

    fn prepare_path_for_create(path: &Path, root: &Path) -> Result<(), FileAudioStoreError> {
        let relative = path
            .strip_prefix(root)
            .map_err(|_| FileAudioStoreError::UnsafePath {
                path: path.to_owned(),
            })?;
        let parent = relative
            .parent()
            .ok_or_else(|| FileAudioStoreError::UnsafePath {
                path: path.to_owned(),
            })?;

        Self::ensure_directory(root)?;
        let mut current = root.to_owned();
        for component in parent {
            current.push(component);
            if let Ok(metadata) = fs::symlink_metadata(&current) {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(FileAudioStoreError::UnsafePath { path: current });
                }
            } else {
                fs::create_dir(&current).map_err(|err| FileAudioStoreError::Io {
                    op: "create-dir",
                    path: current.clone(),
                    source: err,
                })?;
                Self::ensure_directory(&current)?;
            }
        }

        if let Ok(metadata) = fs::symlink_metadata(path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(FileAudioStoreError::UnsafePath {
                    path: path.to_owned(),
                });
            }
        }
        Ok(())
    }

    fn ensure_directory(path: &Path) -> Result<(), FileAudioStoreError> {
        if let Ok(metadata) = fs::symlink_metadata(path) {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(FileAudioStoreError::UnsafePath {
                    path: path.to_owned(),
                });
            }
            return Ok(());
        }

        fs::create_dir_all(path).map_err(|err| FileAudioStoreError::Io {
            op: "create-dir",
            path: path.to_owned(),
            source: err,
        })?;
        let metadata = fs::symlink_metadata(path).map_err(|err| FileAudioStoreError::Io {
            op: "metadata",
            path: path.to_owned(),
            source: err,
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(FileAudioStoreError::UnsafePath {
                path: path.to_owned(),
            });
        }
        Ok(())
    }

    fn create_new_file(path: &Path, bytes: &[u8]) -> Result<(), FileAudioStoreError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|err| FileAudioStoreError::Io {
                op: "create",
                path: path.to_owned(),
                source: err,
            })?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|err| FileAudioStoreError::Io {
                op: "write",
                path: path.to_owned(),
                source: err,
            })
    }

    fn read_file(path: &Path) -> Result<Vec<u8>, FileAudioStoreError> {
        if let Ok(metadata) = fs::symlink_metadata(path) {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(FileAudioStoreError::UnsafePath {
                    path: path.to_owned(),
                });
            }
        }
        let mut file = File::open(path).map_err(|err| FileAudioStoreError::Io {
            op: "open",
            path: path.to_owned(),
            source: err,
        })?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|err| FileAudioStoreError::Io {
                op: "read",
                path: path.to_owned(),
                source: err,
            })?;
        Ok(bytes)
    }

    fn read_source_payload(
        &self,
        audio_ref: &AudioObjectRef,
    ) -> Result<Vec<u8>, FileAudioStoreError> {
        Self::read_file(&self.source_path_from_ref(audio_ref)?)
    }

    fn remove_decoded_cache(
        &self,
        cache_ref: &DecodedAudioCacheRef,
    ) -> Result<(), FileAudioStoreError> {
        Self::remove_file_if_exists(&self.decoded_cache_path_from_ref(cache_ref)?)
    }

    fn remove_file_if_exists(path: &Path) -> Result<(), FileAudioStoreError> {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_file() => {
                fs::remove_file(path).map_err(|err| FileAudioStoreError::Io {
                    op: "delete",
                    path: path.to_owned(),
                    source: err,
                })
            }
            Ok(_) => Err(FileAudioStoreError::UnsafePath {
                path: path.to_owned(),
            }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(FileAudioStoreError::Io {
                op: "metadata",
                path: path.to_owned(),
                source: err,
            }),
        }
    }

    fn remove_directory_or_symlink_if_exists(path: &Path) -> Result<(), FileAudioStoreError> {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                fs::remove_file(path).map_err(|err| FileAudioStoreError::Io {
                    op: "delete",
                    path: path.to_owned(),
                    source: err,
                })
            }
            Ok(metadata) if metadata.is_dir() => {
                fs::remove_dir_all(path).map_err(|err| FileAudioStoreError::Io {
                    op: "delete",
                    path: path.to_owned(),
                    source: err,
                })
            }
            Ok(_) => Err(FileAudioStoreError::UnsafePath {
                path: path.to_owned(),
            }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(FileAudioStoreError::Io {
                op: "metadata",
                path: path.to_owned(),
                source: err,
            }),
        }
    }

    fn remove_directory_under_root_if_exists(
        path: &Path,
        root: &Path,
    ) -> Result<(), FileAudioStoreError> {
        if !Self::delete_ancestors_are_safe(path, root)? {
            return Ok(());
        }
        Self::remove_directory_or_symlink_if_exists(path)
    }

    fn delete_ancestors_are_safe(path: &Path, root: &Path) -> Result<bool, FileAudioStoreError> {
        let relative = path
            .strip_prefix(root)
            .map_err(|_| FileAudioStoreError::UnsafePath {
                path: path.to_owned(),
            })?;
        let parent = relative
            .parent()
            .ok_or_else(|| FileAudioStoreError::UnsafePath {
                path: path.to_owned(),
            })?;

        match fs::symlink_metadata(root) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(FileAudioStoreError::UnsafePath {
                    path: root.to_owned(),
                });
            }
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(err) => {
                return Err(FileAudioStoreError::Io {
                    op: "metadata",
                    path: root.to_owned(),
                    source: err,
                });
            }
        }

        let mut current = root.to_owned();
        for component in parent {
            current.push(component);
            match fs::symlink_metadata(&current) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    return Err(FileAudioStoreError::UnsafePath { path: current });
                }
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
                Err(err) => {
                    return Err(FileAudioStoreError::Io {
                        op: "metadata",
                        path: current,
                        source: err,
                    });
                }
            }
        }

        Ok(true)
    }

    fn write_decoded_cache_file(
        path: &Path,
        root: &Path,
        segment: &AudioSegment,
    ) -> Result<(), FileAudioStoreError> {
        let sample_count = u32::try_from(segment.samples_f32_mono.len())
            .map_err(|_| FileAudioStoreError::IntegerOverflow("sample_count"))?;

        let mut bytes = Vec::new();
        bytes.extend_from_slice(DECODED_MAGIC);
        bytes.extend_from_slice(&segment.sample_rate_hz.to_le_bytes());
        bytes.extend_from_slice(&segment.channels.to_le_bytes());
        bytes.extend_from_slice(&segment.duration_ms.to_le_bytes());
        bytes.extend_from_slice(&sample_count.to_le_bytes());
        for sample in &segment.samples_f32_mono {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }

        Self::prepare_path_for_create(path, root)?;
        Self::create_new_file(path, &bytes)
    }
}

impl AudioStorePort for FileAudioStore {
    type Error = FileAudioStoreError;

    fn write_source_audio(
        &self,
        user_id: &str,
        utterance_id: &str,
        encoded_audio: &EncodedAudio,
    ) -> Result<AudioObjectRef, Self::Error> {
        let object_key = Self::source_object_key(user_id, utterance_id)?;
        let path = Self::path_from_key(&self.audio_root, &object_key, AUDIO_KEY_PREFIX)?;
        Self::prepare_path_for_create(&path, &self.audio_root)?;
        Self::create_new_file(&path, &encoded_audio.payload)?;

        Ok(AudioObjectRef {
            object_key,
            codec_name: encoded_audio.codec_name.clone(),
            sample_rate_hz: encoded_audio.sample_rate_hz,
            channels: encoded_audio.channels,
        })
    }

    fn read_source_audio(&self, audio_ref: &AudioObjectRef) -> Result<EncodedAudio, Self::Error> {
        Ok(EncodedAudio {
            codec_name: audio_ref.codec_name.clone(),
            sample_rate_hz: audio_ref.sample_rate_hz,
            channels: audio_ref.channels,
            payload: self.read_source_payload(audio_ref)?,
        })
    }

    fn write_decoded_cache(
        &self,
        user_id: &str,
        utterance_id: &str,
        segment: &AudioSegment,
    ) -> Result<DecodedAudioCacheRef, Self::Error> {
        let object_key = Self::decoded_cache_object_key(user_id, utterance_id)?;
        let path = Self::path_from_key(&self.decoded_cache_root, &object_key, DECODED_KEY_PREFIX)?;
        Self::write_decoded_cache_file(&path, &self.decoded_cache_root, segment)?;
        Ok(DecodedAudioCacheRef { object_key })
    }

    fn privacy_delete_user(&self, user_id: &str) -> Result<(), Self::Error> {
        Self::validate_identifier(user_id, "user_id")?;
        let audio_user_root = self.audio_root.join(FIXTURE_DATE_PATH).join(user_id);
        let decoded_user_root = self.decoded_cache_root.join(user_id);
        Self::remove_directory_under_root_if_exists(&audio_user_root, &self.audio_root)?;
        Self::remove_directory_under_root_if_exists(&decoded_user_root, &self.decoded_cache_root)?;
        Ok(())
    }

    fn apply_retention(
        &self,
        audio_ref: &AudioObjectRef,
        cache_ref: &DecodedAudioCacheRef,
        mode: AudioRetentionMode,
    ) -> Result<(), Self::Error> {
        self.remove_decoded_cache(cache_ref)?;
        match mode {
            AudioRetentionMode::Minimal | AudioRetentionMode::StrictPrivate => {
                Self::remove_file_if_exists(&self.source_path_from_ref(audio_ref)?)
            }
            AudioRetentionMode::Balanced | AudioRetentionMode::Research => Ok(()),
        }
    }
}
