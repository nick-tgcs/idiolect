//! Per-device bearer tokens (S3): the personal sync server's auth, replacing the single
//! shared-secret constant-compare both the ingest and model routers used in v1.
//!
//! A device is paired once and handed an opaque, high-entropy bearer token; the server
//! keeps a small persisted map from that token to the device's identity. The token is
//! stored **only as its SHA-256 hash**, so a leaked token file can't be replayed — the
//! same posture the model file and audio digests already use. A personal server pairs a
//! handful of devices, so the whole map lives in memory and is rewritten on each change.
//!
//! Issuance is driven by the pairing handshake (next slice); the routers only ever
//! [`DeviceTokenStore::verify`].

use std::collections::HashMap;
use std::path::PathBuf;

use axum::http::{header, HeaderMap};
use idiolect_common::digest::sha256_hex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// The identity a verified bearer token resolves to: which device presented it and which
/// user's learnings it may ship.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub device_id: String,
    pub user_id: String,
    pub issued_at: Option<String>,
}

/// A device that has been paired with this server: its id, the user it belongs
/// to, and the ISO 8601 timestamp when its token was issued (if recorded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedDevice {
    pub device_id: String,
    pub user_id: String,
    /// Present when the token was issued after this field was added; absent for
    /// tokens migrated from an older store that lacked it.
    pub issued_at: Option<String>,
}

/// One persisted token binding. The token itself is never stored — only its hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenRecord {
    token_hash: String,
    device_id: String,
    user_id: String,
    #[serde(default)]
    issued_at: Option<String>,
}

/// A persistent map from a per-device bearer token (by hash) to the device it was issued
/// to. Open it once at server start and share it (read-only for [`DeviceTokenStore::verify`])
/// across requests; pairing mutates it through [`DeviceTokenStore::issue`].
#[derive(Debug)]
pub struct DeviceTokenStore {
    path: PathBuf,
    by_hash: HashMap<String, DeviceIdentity>,
}

impl DeviceTokenStore {
    /// Open the store at `path`, loading any previously issued tokens (an absent file is
    /// an empty store — the pre-pairing state).
    pub fn open(path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        let by_hash = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice::<Vec<TokenRecord>>(&bytes)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?
                .into_iter()
                .map(|record| {
                    (
                        record.token_hash,
                        DeviceIdentity {
                            device_id: record.device_id,
                            user_id: record.user_id,
                            issued_at: record.issued_at,
                        },
                    )
                })
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
            Err(error) => return Err(error),
        };
        Ok(Self { path, by_hash })
    }

    /// All currently paired devices (one entry per unique device id). Order is
    /// unspecified (the backing map does not track insertion order).
    pub fn devices(&self) -> Vec<PairedDevice> {
        self.by_hash
            .values()
            .map(|identity| PairedDevice {
                device_id: identity.device_id.clone(),
                user_id: identity.user_id.clone(),
                issued_at: identity.issued_at.clone(),
            })
            .collect()
    }

    /// Mint a fresh bearer token for `device_id`/`user_id`, persist its binding, and
    /// return the plaintext token (shown to the device once, during pairing). Re-issuing
    /// for a device that already has a token revokes the old one.
    pub fn issue(&mut self, device_id: &str, user_id: &str) -> std::io::Result<String> {
        let token = mint_token();
        self.bind(&token, device_id, user_id)?;
        Ok(token)
    }

    /// Bind a specific plaintext `token` to a device (stored only as its hash), revoking
    /// any previous token for that device. Used by the pairing handshake to register a
    /// freshly minted token, and by the server binary to honour a manually configured
    /// shared token until pairing lands.
    pub fn bind(&mut self, token: &str, device_id: &str, user_id: &str) -> std::io::Result<()> {
        self.by_hash
            .retain(|_, identity| identity.device_id != device_id);
        self.by_hash.insert(
            sha256_hex(token.as_bytes()),
            DeviceIdentity {
                device_id: device_id.to_owned(),
                user_id: user_id.to_owned(),
                issued_at: Some(iso8601_now()),
            },
        );
        self.persist()
    }

    /// Revoke all tokens for `device_id` and persist the change. Returns `Ok(())` even if the
    /// device was not present (idempotent).
    pub fn revoke(&mut self, device_id: &str) -> std::io::Result<()> {
        self.by_hash
            .retain(|_, identity| identity.device_id != device_id);
        self.persist()
    }

    /// Resolve a presented bearer token to its device identity, or `None` if unknown.
    #[must_use]
    pub fn verify(&self, presented: &str) -> Option<DeviceIdentity> {
        self.by_hash.get(&sha256_hex(presented.as_bytes())).cloned()
    }

    /// How many devices currently hold a token.
    #[must_use]
    pub fn device_count(&self) -> usize {
        self.by_hash.len()
    }

    /// Rewrite the on-disk record set atomically (temp + rename), so a crash never leaves
    /// a truncated token file.
    fn persist(&self) -> std::io::Result<()> {
        let records: Vec<TokenRecord> = self
            .by_hash
            .iter()
            .map(|(hash, identity)| TokenRecord {
                token_hash: hash.clone(),
                device_id: identity.device_id.clone(),
                user_id: identity.user_id.clone(),
                issued_at: identity.issued_at.clone(),
            })
            .collect();
        let json = serde_json::to_vec_pretty(&records)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, json)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

/// A new random bearer token: 244 bits from two v4 UUIDs, hex, no separators.
fn mint_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// Current time as an ISO 8601 UTC timestamp for recording `issued_at`.
fn iso8601_now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    let millis = d.subsec_millis();
    // Format as YYYY-MM-DDTHH:MM:SS.mmmZ from unix seconds.
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400; // days since 1970-01-01
    let (y, mo, d) = days_to_ymd(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}.{millis:03}Z")
}

fn is_leap_year(y: u64) -> bool {
    y.is_multiple_of(4) && (!y.is_multiple_of(100) || y.is_multiple_of(400))
}

fn days_to_ymd(mut days: u64) -> (u64, u64, u64) {
    let mut y = 1970u64;
    loop {
        let dy = if is_leap_year(y) { 366 } else { 365 };
        if days < dy {
            break;
        }
        days -= dy;
        y += 1;
    }
    let leap = is_leap_year(y);
    let months = if leap {
        [31u64, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31u64, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut mo = 1u64;
    for dim in months {
        if days < dim {
            break;
        }
        days -= dim;
        mo += 1;
    }
    (y, mo, days + 1)
}

/// Resolve an `Authorization: Bearer <token>` header against `tokens`, or `None` if the
/// header is missing/malformed or the token is unknown. The single entry point both the
/// ingest and model routers gate on, so they authenticate identically.
#[must_use]
pub fn authenticate(headers: &HeaderMap, tokens: &DeviceTokenStore) -> Option<DeviceIdentity> {
    let presented = headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?;
    tokens.verify(presented)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, DeviceTokenStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = DeviceTokenStore::open(dir.path().join("tokens.json")).expect("open");
        (dir, store)
    }

    #[test]
    fn an_issued_token_verifies_to_its_device() {
        let (_dir, mut store) = store();
        let token = store.issue("pixel", "default").expect("issue");

        let identity = store.verify(&token).expect("must verify");
        assert_eq!(identity.device_id, "pixel");
        assert_eq!(identity.user_id, "default");
        assert_eq!(store.device_count(), 1);
    }

    #[test]
    fn an_unknown_or_altered_token_does_not_verify() {
        let (_dir, mut store) = store();
        let token = store.issue("pixel", "default").expect("issue");

        assert_eq!(store.verify("not-a-real-token"), None);
        assert_eq!(store.verify(""), None);
        // A near-miss (truncated) is still rejected.
        assert_eq!(store.verify(&token[..token.len() - 1]), None);
    }

    #[test]
    fn each_device_gets_a_distinct_token_resolving_to_its_own_identity() {
        let (_dir, mut store) = store();
        let pixel = store.issue("pixel", "default").expect("issue pixel");
        let tablet = store.issue("tablet", "default").expect("issue tablet");

        assert_ne!(pixel, tablet, "tokens must be unique per device");
        assert_eq!(store.verify(&pixel).unwrap().device_id, "pixel");
        assert_eq!(store.verify(&tablet).unwrap().device_id, "tablet");
        assert_eq!(store.device_count(), 2);
    }

    #[test]
    fn tokens_persist_across_a_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tokens.json");
        let token = DeviceTokenStore::open(&path)
            .expect("open")
            .issue("pixel", "default")
            .expect("issue");

        let reopened = DeviceTokenStore::open(&path).expect("reopen");
        assert_eq!(reopened.verify(&token).unwrap().device_id, "pixel");
        assert_eq!(reopened.device_count(), 1);
    }

    #[test]
    fn the_plaintext_token_is_never_written_to_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tokens.json");
        let token = DeviceTokenStore::open(&path)
            .expect("open")
            .issue("pixel", "default")
            .expect("issue");

        let on_disk = std::fs::read_to_string(&path).expect("read token file");
        assert!(
            !on_disk.contains(&token),
            "the file must hold only the token hash, never the plaintext",
        );
        assert!(
            on_disk.contains(&sha256_hex(token.as_bytes())),
            "the file holds the token hash",
        );
    }

    #[test]
    fn a_bound_token_verifies_to_its_device() {
        let (_dir, mut store) = store();
        store
            .bind("known-secret", "pixel", "default")
            .expect("bind");
        assert_eq!(store.verify("known-secret").unwrap().device_id, "pixel");
    }

    #[test]
    fn authenticate_resolves_a_valid_bearer_header_and_rejects_the_rest() {
        let (_dir, mut store) = store();
        let token = store.issue("pixel", "default").expect("issue");

        let bearer = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::AUTHORIZATION, value.parse().unwrap());
            headers
        };
        assert_eq!(
            authenticate(&bearer(&format!("Bearer {token}")), &store)
                .unwrap()
                .device_id,
            "pixel",
        );
        assert!(authenticate(&bearer("Bearer nope"), &store).is_none());
        assert!(authenticate(&bearer(&token), &store).is_none(), "no scheme");
        assert!(
            authenticate(&HeaderMap::new(), &store).is_none(),
            "absent header",
        );
    }

    #[test]
    fn reissuing_for_a_device_revokes_its_previous_token() {
        let (_dir, mut store) = store();
        let old = store.issue("pixel", "default").expect("issue");
        let new = store.issue("pixel", "default").expect("reissue");

        assert_ne!(old, new);
        assert_eq!(store.verify(&old), None, "the old token is revoked");
        assert_eq!(store.verify(&new).unwrap().device_id, "pixel");
        assert_eq!(store.device_count(), 1, "still one device");
    }

    // ---- devices() listing ----

    #[test]
    fn devices_is_empty_before_any_pairing() {
        let (_dir, store) = store();
        assert!(store.devices().is_empty());
    }

    #[test]
    fn devices_lists_each_paired_device_once() {
        let (_dir, mut store) = store();
        store.issue("pixel", "default").expect("pixel");
        store.issue("tablet", "default").expect("tablet");

        let mut ids: Vec<_> = store.devices().into_iter().map(|d| d.device_id).collect();
        ids.sort();
        assert_eq!(ids, ["pixel", "tablet"]);
    }

    #[test]
    fn devices_reflects_user_id_and_records_issued_at() {
        let (_dir, mut store) = store();
        store.issue("pixel", "default").expect("issue");
        let devices = store.devices();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].user_id, "default");
        assert!(
            devices[0].issued_at.is_some(),
            "issued_at must be recorded after binding"
        );
    }

    #[test]
    fn devices_listing_survives_a_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tokens.json");
        DeviceTokenStore::open(&path)
            .expect("open")
            .issue("pixel", "default")
            .expect("issue");

        let reopened = DeviceTokenStore::open(&path).expect("reopen");
        let devices = reopened.devices();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device_id, "pixel");
    }

    #[test]
    fn issued_at_is_an_iso8601_utc_timestamp() {
        let (_dir, mut store) = store();
        store.issue("pixel", "default").expect("issue");
        let issued_at = store.devices()[0].issued_at.clone().expect("issued_at");
        // Must end in 'Z' and contain 'T'.
        assert!(issued_at.ends_with('Z'), "must be UTC: {issued_at}");
        assert!(issued_at.contains('T'), "must be ISO 8601: {issued_at}");
        // Must have at least the date portion (YYYY-MM-DD).
        let parts: Vec<_> = issued_at.split('T').collect();
        assert_eq!(parts[0].len(), 10, "date portion: {}", parts[0]);
    }

    // ---- revoke() ----

    #[test]
    fn revoke_removes_device_and_its_token() {
        let (_dir, mut store) = store();
        let token = store.issue("pixel", "default").expect("issue");
        store.revoke("pixel").expect("revoke");
        assert_eq!(store.verify(&token), None, "token must be invalidated");
        assert!(store.devices().is_empty(), "device must be removed");
    }

    #[test]
    fn revoking_nonexistent_device_is_a_noop() {
        let (_dir, mut store) = store();
        store.revoke("phantom").expect("revoke noop");
        assert!(store.devices().is_empty());
    }

    #[test]
    fn revoke_only_removes_targeted_device() {
        let (_dir, mut store) = store();
        store.issue("pixel", "default").expect("pixel");
        let tablet_token = store.issue("tablet", "default").expect("tablet");
        store.revoke("pixel").expect("revoke pixel");
        assert!(store.verify(&tablet_token).is_some(), "tablet token intact");
        let ids: Vec<_> = store.devices().into_iter().map(|d| d.device_id).collect();
        assert_eq!(ids, ["tablet"]);
    }
}
