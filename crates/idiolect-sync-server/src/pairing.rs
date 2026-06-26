//! Device pairing (S3): turning a short, operator-shown code into a per-device bearer
//! token, so a new phone can enrol itself without the operator ever copying a long
//! secret by hand. This is the handshake that makes the per-device tokens of
//! [`crate::device_tokens`] safe to hand out over the tailnet.
//!
//! The flow:
//!   1. The operator mints a code on the PC (`idiolect-sync-server --pair`); the server
//!      prints a short grouped code like `7K9M-P2QW` and keeps serving.
//!   2. The phone POSTs `{code, device_id}` to `POST /v1/pair`. That route is *not*
//!      bearer-authenticated — the phone has no token yet; possession of the short code
//!      is the authorisation.
//!   3. On a correct, unexpired, un-burned code the server issues a per-device bearer
//!      token (via [`crate::device_tokens::DeviceTokenStore::issue`]) and returns it
//!      once. The phone then presents `Authorization: Bearer <token>` to `/v1/sync` and
//!      `/v1/model`, which share the same token store.
//!
//! Why this is safe to expose:
//!   * The code is 8 chars from a 32-symbol alphabet (2^40), stored **only as its
//!     SHA-256 hash** — a leaked state file can't be replayed, mirroring the token store.
//!   * At most **one** code is outstanding, it is **single-use**, expires after
//!     [`PAIRING_CODE_TTL_SECS`], and burns after [`PAIRING_MAX_ATTEMPTS`] wrong guesses.
//!     Redeem-and-issue happen in one critical section, so a replayed or concurrent
//!     correct code can never mint two tokens.
//!   * Every failure mode collapses to an opaque `401` so the endpoint is not an oracle
//!     for whether a code is outstanding/expired/wrong.
//!
//! Accepted trade-offs under the trusted-tailnet threat model (the code is the only
//! secret, so the route is deliberately open):
//!   * The attempt cap doubles as a denial lever — anyone who can reach `/v1/pair` can
//!     burn the outstanding code with [`PAIRING_MAX_ATTEMPTS`] wrong guesses before the
//!     real device redeems. Re-minting via `--pair` is cheap, so this is accepted.
//!   * The code carries no device binding, and the redeemer freely names its own
//!     `device_id`; because [`DeviceTokenStore::bind`](crate::device_tokens::DeviceTokenStore)
//!     is destructive per device id, a holder of a live code can rotate (evict) an
//!     existing device's token by reusing its id. This is what makes self-rotation work
//!     and is gated on first possessing the short code.
//!
//! All time enters as an injected `now_secs: u64`, so the state machine is fully
//! deterministic in tests; [`system_now`] is the single reader of the real wall clock.
//! No constant-time compare is needed: like [`crate::device_tokens::DeviceTokenStore::verify`],
//! the comparison is over SHA-256 hashes, not the secrets themselves.

use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use idiolect_common::digest::sha256_hex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::device_tokens::DeviceTokenStore;

/// Crockford-base32 minus the ambiguous `I`, `L`, `O`, `U` — 32 symbols, so a byte
/// folded with `% 32` is unbiased (256 is an exact multiple of 32).
const PAIRING_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Code length in characters; 8 symbols of a 32-symbol alphabet is 2^40 possibilities.
const PAIRING_CODE_LEN: usize = 8;
/// How long a freshly minted code stays valid — long enough to read off a PC screen and
/// type onto a phone; the attempt cap, not the window, carries the brute-force defence.
const PAIRING_CODE_TTL_SECS: u64 = 600;
/// Wrong guesses before the outstanding code is burned.
const PAIRING_MAX_ATTEMPTS: u32 = 5;

/// One outstanding pairing code. The code itself is never stored — only its hash.
#[derive(Debug)]
struct PendingCode {
    code_hash: String,
    expires_at_secs: u64,
    attempts_remaining: u32,
}

/// The mutable pairing state: at most one outstanding code at a time (minting a new one
/// supersedes the old). Stores no clock, so it derives `Debug` and unit tests drive it
/// with literal `now_secs`.
#[derive(Debug, Default)]
pub struct PairingState {
    pending: Option<PendingCode>,
}

/// The result of presenting a code to [`PairingState::redeem`]. The handler maps
/// [`RedeemOutcome::Issued`] to `201` and every other variant to an opaque `401`.
#[derive(Debug, PartialEq, Eq)]
pub enum RedeemOutcome {
    /// Correct, unexpired, un-burned code — the caller may now issue a token.
    Issued,
    /// No code is currently outstanding.
    NoPendingCode,
    /// A code was outstanding but has passed its expiry.
    Expired,
    /// Wrong code; the outstanding code survives (with one fewer attempt).
    WrongCode,
    /// The outstanding code ran out of attempts and was burned.
    TooManyAttempts,
}

impl PairingState {
    /// Mint a fresh code valid for [`PAIRING_CODE_TTL_SECS`] from `now_secs`, superseding
    /// any prior one. Returns the plaintext code to show the operator once; only its hash
    /// is retained.
    pub fn generate(&mut self, now_secs: u64) -> String {
        let code = random_code();
        self.pending = Some(PendingCode {
            code_hash: sha256_hex(code.as_bytes()),
            expires_at_secs: now_secs.saturating_add(PAIRING_CODE_TTL_SECS),
            attempts_remaining: PAIRING_MAX_ATTEMPTS,
        });
        code
    }

    /// Present `presented` against the outstanding code at time `now_secs`. The code is
    /// normalised (uppercased, separators stripped) before hashing, so the operator may
    /// type it lower-case or with/without the grouping dash. A correct code returns
    /// [`RedeemOutcome::Issued`] but is **not** consumed: the caller commits the single-use
    /// [`consume`](Self::consume) only after a durable follow-through (a token issued), so
    /// a failed issue leaves the code redeemable for a retry instead of burning it.
    /// Expired or attempt-exhausted codes are cleared.
    pub fn redeem(&mut self, presented: &str, now_secs: u64) -> RedeemOutcome {
        let Some(pending) = self.pending.as_mut() else {
            return RedeemOutcome::NoPendingCode;
        };
        if now_secs >= pending.expires_at_secs {
            self.pending = None;
            return RedeemOutcome::Expired;
        }
        if sha256_hex(normalise_code(presented).as_bytes()) == pending.code_hash {
            return RedeemOutcome::Issued;
        }
        pending.attempts_remaining -= 1;
        if pending.attempts_remaining == 0 {
            self.pending = None;
            RedeemOutcome::TooManyAttempts
        } else {
            RedeemOutcome::WrongCode
        }
    }

    /// Commit the single-use consume of the outstanding code, after a [`redeem`](Self::redeem)
    /// returned [`RedeemOutcome::Issued`] and the caller durably acted on it. Idempotent;
    /// a no-op if nothing is outstanding. Must be called under the same lock that guarded
    /// the redeem so the check-and-consume is atomic.
    pub fn consume(&mut self) {
        self.pending = None;
    }

    /// Whether an unexpired code is currently outstanding at `now_secs`.
    #[must_use]
    pub fn has_active_code(&self, now_secs: u64) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| now_secs < pending.expires_at_secs)
    }
}

/// Shared state for the `/v1/pair` router: the **same** token store the model and ingest
/// routers share (so a paired device's token authenticates everywhere) plus the pairing
/// state. The pairing mutex serialises redeem-and-issue so single-use is atomic under
/// concurrent requests.
#[derive(Debug)]
pub struct PairingServerState {
    tokens: Arc<Mutex<DeviceTokenStore>>,
    pairing: Mutex<PairingState>,
}

impl PairingServerState {
    /// Wrap the shared token store behind the pairing API, with no code outstanding.
    #[must_use]
    pub fn new(tokens: Arc<Mutex<DeviceTokenStore>>) -> Self {
        Self {
            tokens,
            pairing: Mutex::new(PairingState::default()),
        }
    }

    /// Mint a fresh pairing code against the live state (the operator path). Returns the
    /// plaintext code to display; only its hash is retained.
    pub fn generate_code(&self, now_secs: u64) -> String {
        self.pairing
            .lock()
            .expect("pairing mutex poisoned")
            .generate(now_secs)
    }

    /// Mint a fresh code and build the full [`PairingOffer`] in one call: the code,
    /// its display form, the deep-link URI, the QR bool matrix, and the expiry
    /// timestamp. The caller supplies `base_url` (phone-facing URL) and optionally a
    /// `fingerprint` (SPKI sha256 hex, absent for `--no-tls`). QR encoding errors are
    /// surfaced as `Err` so the caller can degrade to the text-only fallback.
    pub fn mint_offer(
        &self,
        base_url: &str,
        fingerprint: Option<&str>,
        now_secs: u64,
    ) -> Result<PairingOffer, String> {
        let code = self.generate_code(now_secs);
        let display_code = group(&code);
        let uri = crate::pairing_qr::pairing_uri(base_url, &code, fingerprint);
        let (qr_matrix, qr_width) = crate::pairing_qr::qr_matrix(&uri)?;
        Ok(PairingOffer {
            display_code,
            uri,
            qr_matrix,
            qr_width,
            expires_at_secs: now_secs.saturating_add(PAIRING_CODE_TTL_SECS),
            code,
        })
    }
}

/// The full description of an outstanding pairing invitation: everything the dashboard
/// needs to render the QR, show the typed fallback, and display the countdown.
#[derive(Debug, Clone)]
pub struct PairingOffer {
    /// The raw 8-character code from the pairing alphabet (for typed entry).
    pub code: String,
    /// The operator-friendly display form: `XXXX-XXXX`.
    pub display_code: String,
    /// The deep-link URI encoded in the QR: `idiolect://pair?u=…&c=…[&f=…]`.
    pub uri: String,
    /// The QR module matrix (row-major, `true` = dark), without quiet zone.
    pub qr_matrix: Vec<bool>,
    /// Side length of the square [`qr_matrix`].
    pub qr_width: usize,
    /// Unix timestamp (seconds) when this code expires.
    pub expires_at_secs: u64,
}

/// The phone's pairing request: the short code and the device id it proposes for itself.
#[derive(Debug, Deserialize)]
struct PairRequest {
    code: String,
    device_id: String,
}

/// The pairing response: the per-device bearer token (returned exactly once) and the
/// identity it was bound to.
#[derive(Debug, Serialize)]
struct PairResponse {
    token: String,
    device_id: String,
    user_id: String,
}

/// Build the pairing router: `POST /v1/pair`, intentionally *not* bearer-guarded.
pub fn pair_router(state: Arc<PairingServerState>) -> Router {
    Router::new()
        .route("/v1/pair", post(pair))
        .with_state(state)
}

/// Bind the pairing router to `listener` and serve until the process ends.
pub async fn serve_pair(
    listener: tokio::net::TcpListener,
    state: Arc<PairingServerState>,
) -> std::io::Result<()> {
    axum::serve(listener, pair_router(state)).await
}

async fn pair(State(state): State<Arc<PairingServerState>>, body: Bytes) -> Response {
    // Read the body ourselves (like `ingest_batch`) so a malformed payload is a
    // deterministic 400 rather than axum's `Json` extractor 422.
    let request: PairRequest = match serde_json::from_slice(body.as_ref()) {
        Ok(request) => request,
        Err(_) => return StatusCode::BAD_REQUEST.into_response(),
    };
    if !valid_device_id(&request.device_id) {
        return StatusCode::BAD_REQUEST.into_response();
    }

    let now = system_now();
    // Hold the pairing mutex across redeem-issue-consume: a concurrent or replayed correct
    // code serialises here and finds the code already consumed, so at most one token is
    // ever minted. The lock order is always pairing-then-tokens; `authenticate` only ever
    // locks tokens, so there is no cycle.
    let mut pairing = state.pairing.lock().expect("pairing mutex poisoned");
    if pairing.redeem(&request.code, now) != RedeemOutcome::Issued {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let mut tokens = state.tokens.lock().expect("token store mutex poisoned");
    match tokens.issue(&request.device_id, "default") {
        Ok(token) => {
            // Commit the single-use consume only now the token is durably persisted; on a
            // failed issue the code is left intact (redeem did not consume) so the device
            // can retry rather than the operator having to re-mint.
            pairing.consume();
            (
                StatusCode::CREATED,
                Json(PairResponse {
                    token,
                    device_id: request.device_id,
                    user_id: "default".to_owned(),
                }),
            )
                .into_response()
        }
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

/// A new random pairing code: 8 symbols from a CSPRNG-seeded v4 UUID **whitened through
/// SHA-256**. The raw v4 UUID bytes are not uniform — the version nibble and variant bits
/// are fixed — so folding them directly would pin some code positions to a subset of the
/// alphabet (2^39). Hashing first makes every output byte uniform, so each position spans
/// the full 32-symbol alphabet and the keyspace is the intended 32^8 = 2^40.
fn random_code() -> String {
    let whitened = Sha256::digest(Uuid::new_v4().as_bytes());
    whitened[..PAIRING_CODE_LEN]
        .iter()
        .map(|byte| PAIRING_ALPHABET[(byte % 32) as usize] as char)
        .collect()
}

/// Normalise an operator-typed code before hashing: drop whitespace and the grouping
/// dash, then uppercase, so `7k9m-p2qw` and `7K9MP2QW` both match.
fn normalise_code(code: &str) -> String {
    code.chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_ascii_uppercase()
}

/// Group a raw 8-char code as `XXXX-XXXX` for display; passes anything else through.
#[must_use]
pub fn group(code: &str) -> String {
    if code.len() == PAIRING_CODE_LEN && code.is_ascii() {
        format!("{}-{}", &code[..4], &code[4..])
    } else {
        code.to_owned()
    }
}

/// Whether the phone-proposed device id is acceptable: non-empty, at most 64 chars, and
/// limited to `[A-Za-z0-9._-]`. It is a label, never a security boundary (the code is).
fn valid_device_id(device_id: &str) -> bool {
    (1..=64).contains(&device_id.len())
        && device_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The current unix time in whole seconds — the sole reader of the real wall clock in
/// this module, so all pairing logic stays deterministic under an injected `now_secs`.
#[must_use]
pub fn system_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    // ---- the code state machine (deterministic, literal `now_secs`) ----

    #[test]
    fn a_generated_code_is_eight_chars_from_the_pairing_alphabet() {
        let code = PairingState::default().generate(0);
        assert_eq!(code.len(), PAIRING_CODE_LEN);
        assert!(code.bytes().all(|b| PAIRING_ALPHABET.contains(&b)));
    }

    #[test]
    fn the_alphabet_excludes_ambiguous_letters() {
        assert_eq!(PAIRING_ALPHABET.len(), 32);
        for ambiguous in [b'I', b'L', b'O', b'U'] {
            assert!(
                !PAIRING_ALPHABET.contains(&ambiguous),
                "{} must not be in the alphabet",
                ambiguous as char
            );
        }
    }

    #[test]
    fn the_plaintext_code_is_never_retained() {
        let mut state = PairingState::default();
        let code = state.generate(0);
        let hash = &state.pending.as_ref().expect("pending").code_hash;
        assert_ne!(*hash, code, "the hash must not be the plaintext");
        assert_eq!(*hash, sha256_hex(code.as_bytes()));
    }

    #[test]
    fn two_generated_codes_differ() {
        let first = PairingState::default().generate(0);
        let second = PairingState::default().generate(0);
        assert_ne!(first, second, "codes must carry real entropy");
    }

    #[test]
    fn every_code_position_can_reach_the_whole_alphabet() {
        // Regression: folding raw v4 UUID bytes pinned the version-byte position to the
        // low half of the alphabet (2^39, not the documented 2^40). After whitening,
        // every position must be able to reach the high half (G..Z) too. The chance a
        // uniform position misses the high half across 512 draws is 2^-512, so this is
        // deterministic in practice, not flaky.
        let high_half: std::collections::HashSet<u8> =
            PAIRING_ALPHABET[16..].iter().copied().collect();
        let mut reached = [false; PAIRING_CODE_LEN];
        for _ in 0..512 {
            for (position, byte) in random_code().bytes().enumerate() {
                if high_half.contains(&byte) {
                    reached[position] = true;
                }
            }
        }
        assert!(
            reached.iter().all(|&hit| hit),
            "every position must reach the high half of the alphabet: {reached:?}"
        );
    }

    #[test]
    fn a_correct_code_redeems_idempotently_until_consumed() {
        let mut state = PairingState::default();
        let code = state.generate(0);
        // Redeem reports success without consuming, so a failed follow-through (e.g. a
        // token-issue I/O error) can retry the same code...
        assert_eq!(state.redeem(&code, 0), RedeemOutcome::Issued);
        assert_eq!(state.redeem(&code, 0), RedeemOutcome::Issued);
        // ...until the caller commits the single-use consume.
        state.consume();
        assert_eq!(state.redeem(&code, 0), RedeemOutcome::NoPendingCode);
    }

    #[test]
    fn a_wrong_code_does_not_consume_the_pending_code() {
        let mut state = PairingState::default();
        let code = state.generate(0);
        assert_eq!(state.redeem("WRONGCOD", 0), RedeemOutcome::WrongCode);
        assert_eq!(state.redeem(&code, 0), RedeemOutcome::Issued);
    }

    #[test]
    fn a_wrong_code_decrements_attempts_and_then_burns() {
        let mut state = PairingState::default();
        let code = state.generate(0);
        for _ in 0..PAIRING_MAX_ATTEMPTS - 1 {
            assert_eq!(state.redeem("WRONGCOD", 0), RedeemOutcome::WrongCode);
        }
        assert_eq!(state.redeem("WRONGCOD", 0), RedeemOutcome::TooManyAttempts);
        // Burned: even the correct code no longer redeems.
        assert_eq!(state.redeem(&code, 0), RedeemOutcome::NoPendingCode);
    }

    #[test]
    fn an_expired_code_does_not_redeem() {
        let mut state = PairingState::default();
        let code = state.generate(0);
        assert_eq!(
            state.redeem(&code, PAIRING_CODE_TTL_SECS),
            RedeemOutcome::Expired
        );
        assert!(!state.has_active_code(PAIRING_CODE_TTL_SECS));
    }

    #[test]
    fn a_code_is_valid_one_second_before_expiry_and_invalid_at_it() {
        let mut just_valid = PairingState::default();
        let code = just_valid.generate(0);
        assert!(just_valid.has_active_code(PAIRING_CODE_TTL_SECS - 1));
        assert_eq!(
            just_valid.redeem(&code, PAIRING_CODE_TTL_SECS - 1),
            RedeemOutcome::Issued
        );

        let mut at_deadline = PairingState::default();
        let code = at_deadline.generate(0);
        assert_eq!(
            at_deadline.redeem(&code, PAIRING_CODE_TTL_SECS),
            RedeemOutcome::Expired
        );
    }

    #[test]
    fn generating_a_new_code_supersedes_the_old() {
        let mut state = PairingState::default();
        let first = state.generate(0);
        let second = state.generate(0);
        assert_ne!(first, second);
        assert_ne!(
            state.redeem(&first, 0),
            RedeemOutcome::Issued,
            "the superseded code must not redeem"
        );
        assert_eq!(state.redeem(&second, 0), RedeemOutcome::Issued);
    }

    #[test]
    fn a_code_is_normalised_before_hashing() {
        let mut state = PairingState::default();
        let code = state.generate(0);
        // The operator types it grouped and lower-cased; it must still match.
        let typed = group(&code).to_ascii_lowercase();
        assert_eq!(state.redeem(&typed, 0), RedeemOutcome::Issued);
    }

    #[test]
    fn group_inserts_a_separator_for_legibility() {
        assert_eq!(group("ABCDEFGH"), "ABCD-EFGH");
        assert_eq!(group("SHORT"), "SHORT", "non-8-char input passes through");
    }

    #[test]
    fn an_empty_or_oversized_device_id_is_rejected() {
        assert!(!valid_device_id(""));
        assert!(!valid_device_id(&"x".repeat(65)));
        assert!(!valid_device_id("has space"));
        assert!(!valid_device_id("bad/slash"));
        assert!(valid_device_id("pixel-7a.2_b"));
    }

    #[test]
    fn redeem_then_consume_is_atomic_under_real_thread_contention() {
        use std::sync::Barrier;
        use std::thread;

        // Sixteen OS threads race to redeem-and-consume the SAME code through one shared
        // lock — the exact critical section the handler runs. Because consume happens under
        // the same held lock as the redeem, only the first thread sees Issued-then-clears;
        // every other thread finds NoPendingCode. A non-atomic check/consume (or a dropped
        // lock between them) would let two threads through. Unlike the HTTP join! test this
        // genuinely contends on real threads with a start barrier.
        const THREADS: usize = 16;
        let state = Arc::new(Mutex::new(PairingState::default()));
        let code = state.lock().expect("lock").generate(0);
        let barrier = Arc::new(Barrier::new(THREADS));

        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let state = Arc::clone(&state);
                let barrier = Arc::clone(&barrier);
                let code = code.clone();
                thread::spawn(move || {
                    barrier.wait();
                    let mut guard = state.lock().expect("lock");
                    if guard.redeem(&code, 0) == RedeemOutcome::Issued {
                        guard.consume();
                        true
                    } else {
                        false
                    }
                })
            })
            .collect();
        let winners = handles
            .into_iter()
            .map(|handle| handle.join().expect("join"))
            .filter(|&won| won)
            .count();
        assert_eq!(
            winners, 1,
            "exactly one thread may redeem-and-consume the code"
        );
    }

    // ---- the HTTP handler (driven through the router via tower::oneshot) ----

    fn server_state() -> (tempfile::TempDir, Arc<PairingServerState>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let tokens = DeviceTokenStore::open(dir.path().join("tokens.json")).expect("tokens");
        let state = Arc::new(PairingServerState::new(Arc::new(Mutex::new(tokens))));
        (dir, state)
    }

    fn pair_request(code: &str, device_id: &str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/v1/pair")
            .body(Body::from(
                serde_json::json!({ "code": code, "device_id": device_id }).to_string(),
            ))
            .expect("request")
    }

    fn device_count(state: &Arc<PairingServerState>) -> usize {
        state.tokens.lock().expect("tokens").device_count()
    }

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = response
            .into_body()
            .collect()
            .await
            .expect("collect")
            .to_bytes();
        serde_json::from_slice(&bytes).expect("json")
    }

    /// The full observable response (status, sorted headers, body) — so two rejections
    /// can be compared for byte-identity, proving the endpoint leaks no oracle.
    async fn full_response(
        state: Arc<PairingServerState>,
        request: Request<Body>,
    ) -> (StatusCode, Vec<(String, String)>, Vec<u8>) {
        let response = pair_router(state).oneshot(request).await.expect("router");
        let status = response.status();
        let mut headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_owned(),
                    value.to_str().unwrap_or_default().to_owned(),
                )
            })
            .collect();
        headers.sort();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("collect")
            .to_bytes()
            .to_vec();
        (status, headers, body)
    }

    #[tokio::test]
    async fn pairing_with_a_correct_code_issues_a_per_device_token() {
        let (_dir, state) = server_state();
        let code = state.generate_code(system_now());

        let response = pair_router(state.clone())
            .oneshot(pair_request(&code, "pixel-7a"))
            .await
            .expect("router");
        assert_eq!(response.status(), StatusCode::CREATED);

        let body = body_json(response).await;
        assert_eq!(body["device_id"], "pixel-7a");
        assert_eq!(body["user_id"], "default");
        let token = body["token"].as_str().expect("token");
        // The issued token resolves to the proposed device in the shared store.
        assert_eq!(
            state
                .tokens
                .lock()
                .expect("tokens")
                .verify(token)
                .expect("verifies")
                .device_id,
            "pixel-7a"
        );
        assert_eq!(device_count(&state), 1);
    }

    #[tokio::test]
    async fn pairing_with_a_wrong_code_is_unauthorized_and_issues_nothing() {
        let (_dir, state) = server_state();
        let _code = state.generate_code(system_now());

        let response = pair_router(state.clone())
            .oneshot(pair_request("WRONGCOD", "pixel"))
            .await
            .expect("router");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(device_count(&state), 0);
    }

    #[tokio::test]
    async fn pairing_with_an_expired_code_is_unauthorized() {
        let (_dir, state) = server_state();
        // Minted at the unix epoch, so it is long expired by the time the handler reads
        // the real clock — the deterministic way to exercise expiry over HTTP.
        let code = state.generate_code(0);

        let response = pair_router(state.clone())
            .oneshot(pair_request(&code, "pixel"))
            .await
            .expect("router");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(device_count(&state), 0);
    }

    #[tokio::test]
    async fn every_rejection_is_an_indistinguishable_401() {
        // No code ever generated (the binary's default before `--pair`), a wrong code, and
        // an expired code must produce byte-identical responses, so `/v1/pair` is not an
        // oracle for whether a code is outstanding / expired / wrong.
        let (_no_code_dir, no_code) = server_state();

        let (_wrong_dir, wrong) = server_state();
        let _ = wrong.generate_code(system_now());

        let (_expired_dir, expired) = server_state();
        let expired_code = expired.generate_code(0);

        let none = full_response(no_code, pair_request("ANYCODE0", "pixel")).await;
        let bad = full_response(wrong, pair_request("WRONGCOD", "pixel")).await;
        let old = full_response(expired, pair_request(&expired_code, "pixel")).await;

        assert_eq!(none.0, StatusCode::UNAUTHORIZED);
        assert_eq!(
            none, bad,
            "no-code and wrong-code rejections must be identical"
        );
        assert_eq!(
            bad, old,
            "wrong-code and expired-code rejections must be identical"
        );
    }

    #[tokio::test]
    async fn a_redeemed_code_cannot_be_replayed_over_http() {
        let (_dir, state) = server_state();
        let code = state.generate_code(system_now());

        let first = pair_router(state.clone())
            .oneshot(pair_request(&code, "pixel"))
            .await
            .expect("router");
        assert_eq!(first.status(), StatusCode::CREATED);

        let second = pair_router(state.clone())
            .oneshot(pair_request(&code, "pixel"))
            .await
            .expect("router");
        assert_eq!(second.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(device_count(&state), 1, "the replay issued no second token");
    }

    #[tokio::test]
    async fn a_malformed_pair_body_is_a_bad_request() {
        let (_dir, state) = server_state();
        let request = Request::builder()
            .method("POST")
            .uri("/v1/pair")
            .body(Body::from("not a json object"))
            .expect("request");
        let response = pair_router(state).oneshot(request).await.expect("router");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn an_invalid_device_id_is_a_bad_request_and_spares_the_code() {
        let (_dir, state) = server_state();
        let code = state.generate_code(system_now());

        // More bad attempts than PAIRING_MAX_ATTEMPTS: if device_id validation ever moved
        // after redeem, these would each burn an attempt and exhaust the code, turning the
        // final redeem into a 401 — so this pins validate-before-redeem against the cap.
        let bad_ids = [
            "",
            "has space",
            "bad/slash",
            "tab\tbad",
            "semi;colon",
            "uni\u{00a9}ode",
            &"x".repeat(65),
        ];
        assert!(bad_ids.len() > PAIRING_MAX_ATTEMPTS as usize);
        for bad in bad_ids {
            let response = pair_router(state.clone())
                .oneshot(pair_request(&code, bad))
                .await
                .expect("router");
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "device_id {bad:?}"
            );
        }
        // device_id is validated before redeem, so the code survives all the bad attempts.
        let ok = pair_router(state.clone())
            .oneshot(pair_request(&code, "pixel"))
            .await
            .expect("router");
        assert_eq!(ok.status(), StatusCode::CREATED);
        assert_eq!(device_count(&state), 1);
    }

    #[tokio::test]
    async fn a_failed_token_issue_after_redeem_leaves_the_code_redeemable() {
        let dir = tempfile::tempdir().expect("tempdir");
        // `open` sees an absent (empty) store because `sub/` does not exist yet; we then
        // plant a regular FILE where that parent dir must be created, so issue -> persist
        // -> create_dir_all fails deterministically and portably.
        let store_path = dir.path().join("sub").join("tokens.json");
        let tokens = DeviceTokenStore::open(&store_path).expect("open empty");
        let state = Arc::new(PairingServerState::new(Arc::new(Mutex::new(tokens))));
        let code = state.generate_code(system_now());
        std::fs::write(dir.path().join("sub"), b"blocker").expect("blocker file");

        let failed = pair_router(state.clone())
            .oneshot(pair_request(&code, "pixel"))
            .await
            .expect("router");
        assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            state
                .pairing
                .lock()
                .expect("pairing")
                .has_active_code(system_now()),
            "a failed issue must not burn the code"
        );
    }

    #[tokio::test]
    async fn re_pairing_a_device_rotates_its_token_and_revokes_the_old() {
        let (_dir, state) = server_state();

        let code = state.generate_code(system_now());
        let first = pair_router(state.clone())
            .oneshot(pair_request(&code, "pixel"))
            .await
            .expect("router");
        assert_eq!(first.status(), StatusCode::CREATED);
        let old_token = body_json(first).await["token"]
            .as_str()
            .expect("token")
            .to_owned();

        let code = state.generate_code(system_now());
        let second = pair_router(state.clone())
            .oneshot(pair_request(&code, "pixel"))
            .await
            .expect("router");
        assert_eq!(second.status(), StatusCode::CREATED);
        let new_token = body_json(second).await["token"]
            .as_str()
            .expect("token")
            .to_owned();

        let store = state.tokens.lock().expect("tokens");
        assert_ne!(old_token, new_token);
        assert!(
            store.verify(&old_token).is_none(),
            "re-pairing revokes the prior token, so a stolen old token can't outlive it"
        );
        assert_eq!(
            store.verify(&new_token).expect("new verifies").device_id,
            "pixel"
        );
        assert_eq!(
            store.device_count(),
            1,
            "re-pair rotates, never accumulates"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_redeem_of_one_code_issues_at_most_one_token() {
        let (_dir, state) = server_state();
        let code = state.generate_code(system_now());

        // Race the full handler (redeem + issue + consume across both mutexes) on real
        // worker threads via tokio::spawn — not a single-threaded join! that would pass
        // even without the mutex. The pairing lock must admit exactly one redeemer.
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let state = state.clone();
                let code = code.clone();
                tokio::spawn(async move {
                    pair_router(state)
                        .oneshot(pair_request(&code, "pixel"))
                        .await
                        .expect("router")
                        .status()
                })
            })
            .collect();

        let mut created = 0;
        let mut unauthorized = 0;
        for task in tasks {
            match task.await.expect("join") {
                StatusCode::CREATED => created += 1,
                StatusCode::UNAUTHORIZED => unauthorized += 1,
                other => panic!("unexpected status {other}"),
            }
        }
        assert_eq!(created, 1, "exactly one redeem wins");
        assert_eq!(unauthorized, 7, "every other racer is rejected");
        assert_eq!(device_count(&state), 1, "single-use under real concurrency");
    }
}

#[cfg(test)]
mod pairing_offer_tests {
    use super::*;

    fn state() -> (tempfile::TempDir, Arc<PairingServerState>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let tokens = crate::device_tokens::DeviceTokenStore::open(dir.path().join("tokens.json"))
            .expect("tokens");
        let state = Arc::new(PairingServerState::new(Arc::new(Mutex::new(tokens))));
        (dir, state)
    }

    #[test]
    fn mint_offer_returns_an_eight_char_code_with_grouped_display() {
        let (_dir, state) = state();
        let offer = state
            .mint_offer("https://10.0.0.1:8765", None, system_now())
            .expect("offer");
        assert_eq!(offer.code.len(), 8);
        assert_eq!(
            offer.display_code,
            format!("{}-{}", &offer.code[..4], &offer.code[4..])
        );
    }

    #[test]
    fn mint_offer_uri_encodes_the_base_url_and_code() {
        let (_dir, state) = state();
        let offer = state
            .mint_offer("https://10.0.0.1:8765", None, 0)
            .expect("offer");
        assert!(
            offer.uri.contains(&offer.code),
            "uri must contain the raw code"
        );
        assert!(offer.uri.starts_with("idiolect://pair?"));
    }

    #[test]
    fn mint_offer_uri_carries_fingerprint_when_supplied() {
        let (_dir, state) = state();
        let fp = "0123456789abcdef".repeat(4);
        let offer = state
            .mint_offer("https://10.0.0.1:8765", Some(&fp), 0)
            .expect("offer");
        assert!(offer.uri.contains(&fp), "uri must carry the fingerprint");
    }

    #[test]
    fn mint_offer_qr_matrix_is_a_square_of_the_stated_width() {
        let (_dir, state) = state();
        let offer = state
            .mint_offer("https://10.0.0.1:8765", None, 0)
            .expect("offer");
        assert_eq!(
            offer.qr_matrix.len(),
            offer.qr_width * offer.qr_width,
            "qr_matrix must be a square"
        );
        assert!(offer.qr_width >= 21, "QR widths are at least 21 modules");
    }

    #[test]
    fn mint_offer_expires_at_is_ten_minutes_after_now() {
        let (_dir, state) = state();
        let now = 1_000_000u64;
        let offer = state
            .mint_offer("https://10.0.0.1:8765", None, now)
            .expect("offer");
        assert_eq!(offer.expires_at_secs, now + PAIRING_CODE_TTL_SECS);
    }

    #[test]
    fn consecutive_mint_offers_produce_distinct_codes() {
        let (_dir, state) = state();
        let a = state
            .mint_offer("https://10.0.0.1:8765", None, 0)
            .expect("a");
        let b = state
            .mint_offer("https://10.0.0.1:8765", None, 0)
            .expect("b");
        assert_ne!(a.code, b.code, "each mint must generate fresh entropy");
    }
}
