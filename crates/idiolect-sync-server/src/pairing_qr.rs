//! Rendering the one-time pairing code (S3, see [`crate::pairing`]) as a QR the phone's
//! camera can scan, so enrolment is "open the app, point it at the PC" instead of typing
//! an 8-char code and a URL by hand. The typed code stays as the fallback — this only
//! changes how the operator *presents* the same code.
//!
//! The QR carries a [`pairing_uri`]: `idiolect://pair?u=<percent-encoded base>&c=<code>`,
//! so one scan delivers both the endpoint and the code. The phone parses it with
//! `PairingUri.parse` and feeds `(baseUrl, code)` straight into `PairingClient.pair`.
//!
//! Rendering targets a terminal, so scannability must not depend on the operator's colour
//! scheme: [`render_qr`] emits forced black-on-white cells via explicit ANSI background
//! colours (a dark terminal would otherwise invert a default-foreground QR and many
//! readers refuse the inverse). Two stacked modules share one `▀` glyph — foreground is
//! the upper module, background the lower — so the QR is half as tall as it is wide in
//! character cells, i.e. roughly square on screen. A 4-module light quiet zone is added
//! so a reader can lock onto the finder patterns.
//!
//! Everything here is pure: the URI builder, the matrix extraction, and the renderer are
//! unit-tested in this file, and the encode→decode round-trip (an independent reader
//! decoding the rasterised pixels) is the integration test in `tests/pairing_qr.rs`.

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use qrcode::types::Color;
use qrcode::QrCode;

use crate::pairing::group;

/// The light border (in modules) around the QR. Four is the spec minimum a reader needs
/// to isolate the symbol from surrounding terminal text.
const QUIET_ZONE: usize = 4;

/// Build the pairing URI a scan delivers: `idiolect://pair?u=<base>&c=<code>`, with an
/// optional `&f=<fingerprint>` when the server serves TLS (the default). The base URL is
/// percent-encoded (it carries `:` and `/`) so the query is unambiguous; the code is from
/// the pairing alphabet (`[0-9A-Z]` minus ambiguous letters) and the fingerprint is
/// lowercase hex, so both are URL-safe and appended raw. The fingerprint is the SHA-256 of
/// the server cert's DER `SubjectPublicKeyInfo` — the phone pins it (TOFU keyed by this
/// out-of-band scan), defeating a LAN MITM against the self-signed cert. The Android
/// `PairingUri.parse` is the exact inverse — keep the two in lockstep.
#[must_use]
pub fn pairing_uri(base_url: &str, code: &str, fingerprint: Option<&str>) -> String {
    let encoded = utf8_percent_encode(base_url, NON_ALPHANUMERIC);
    let pin = fingerprint.map(|fp| format!("&f={fp}")).unwrap_or_default();
    format!("idiolect://pair?u={encoded}&c={code}{pin}")
}

/// Encode `data` as a QR and return its dark-module matrix (row-major, `true` = dark) and
/// the side length in modules, with **no** quiet zone. Errors if the data is too large to
/// encode. Both the renderer and the round-trip test consume this, so the QR a phone sees
/// and the QR we print are provably the same symbol.
pub fn qr_matrix(data: &str) -> Result<(Vec<bool>, usize), String> {
    let code = QrCode::new(data).map_err(|error| format!("encode pairing QR: {error}"))?;
    let width = code.width();
    let dark = code
        .to_colors()
        .into_iter()
        .map(|color| matches!(color, Color::Dark))
        .collect();
    Ok((dark, width))
}

/// Render a square module matrix to forced-colour terminal half-blocks. Each `▀` packs
/// two vertically-adjacent modules: foreground paints the upper, background the lower.
/// Dark → black (`30`/`40`), light → white (`37`/`47`), so the symbol scans regardless of
/// the terminal theme. An odd final row pairs against a light (white) bottom half.
fn render_dense(modules: &[bool], side: usize) -> String {
    let mut out = String::new();
    let mut row = 0;
    while row < side {
        for col in 0..side {
            let top = modules[row * side + col];
            let bottom = row + 1 < side && modules[(row + 1) * side + col];
            let fg = if top { 30 } else { 37 };
            let bg = if bottom { 40 } else { 47 };
            out.push_str(&format!("\u{1b}[{fg};{bg}m\u{2580}"));
        }
        out.push_str("\u{1b}[0m\n");
        row += 2;
    }
    out
}

/// Re-lay a `width`×`width` matrix into a `(width + 2·quiet)`-square one with a light
/// border, so the rendered QR has its quiet zone. The border is light (`false`).
fn padded(modules: &[bool], width: usize, quiet: usize) -> (Vec<bool>, usize) {
    let side = width + 2 * quiet;
    let mut out = vec![false; side * side];
    for (index, &dark) in modules.iter().enumerate() {
        let (x, y) = (index % width + quiet, index / width + quiet);
        out[y * side + x] = dark;
    }
    (out, side)
}

/// Render `data` as a scannable, forced-colour terminal QR (with a quiet zone). Errors
/// only if `data` is too large for any QR version.
pub fn render_qr(data: &str) -> Result<String, String> {
    let (matrix, width) = qr_matrix(data)?;
    let (bordered, side) = padded(&matrix, width, QUIET_ZONE);
    Ok(render_dense(&bordered, side))
}

/// The full `--pair` announcement: the scannable QR, the actual pairing link as copyable
/// text, and the typed-by-hand fallback (the clean URL and the grouped code), so a device
/// with no camera can still pair — by tapping the link or typing the code. When TLS is on
/// (the default), `fingerprint` is `Some`, so the QR/link carry the `&f=` pin one scan
/// delivers and the fallback prints it for out-of-band verification; `--no-tls` passes
/// `None`. On the rare encode failure the QR line degrades to a note and the rest stands.
///
/// The printed `link:` is byte-identical to what the QR encodes ([`pairing_uri`]): it's the
/// same deep link, just rendered as text so it can be tapped from a message on a real phone,
/// or pasted for headless/emulator testing — not a second mechanism.
#[must_use]
pub fn pairing_announcement(base_url: &str, code: &str, fingerprint: Option<&str>) -> String {
    let uri = pairing_uri(base_url, code, fingerprint);
    let qr = render_qr(&uri).unwrap_or_else(|error| format!("(QR unavailable: {error})\n"));
    let pin_line = fingerprint
        .map(|fp| format!("  pin:  {fp}\n"))
        .unwrap_or_default();
    format!(
        "Pair a device — scan this QR with the idiolect app:\n\n{qr}\n…or tap this link on the phone (the same thing the QR encodes):\n  {uri}\n\n…or enter these by hand:\n  URL:  {base_url}\n  code: {grouped}\n{pin_line}",
        grouped = group(code),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pairing_uri_percent_encodes_the_base_and_appends_the_raw_code() {
        // This exact literal is the cross-language contract: Android's `PairingUri.parse`
        // must invert it byte for byte. `.` `:` `/` are all percent-encoded. With no pin
        // (the `--no-tls` fallback) there is no trailing `&f=`.
        assert_eq!(
            pairing_uri("http://10.0.2.2:8765", "ABCD1234", None),
            "idiolect://pair?u=http%3A%2F%2F10%2E0%2E2%2E2%3A8765&c=ABCD1234",
        );
    }

    #[test]
    fn the_pairing_uri_appends_the_spki_fingerprint_when_pinned() {
        // With TLS on (the default), the QR also carries `&f=<hex sha256 of the server's
        // DER SubjectPublicKeyInfo>` so the phone can pin the self-signed cert it is about
        // to talk to. The fingerprint is lowercase hex — URL-safe, so it needs no
        // percent-encoding and is simply appended. Android's `PairingUri.parse` reads `f`.
        let fingerprint = "0123456789abcdef".repeat(4); // 64 hex chars, like a real sha256
        assert_eq!(
            pairing_uri("https://10.0.2.2:8765", "ABCD1234", Some(&fingerprint)),
            format!(
                "idiolect://pair?u=https%3A%2F%2F10%2E0%2E2%2E2%3A8765&c=ABCD1234&f={fingerprint}"
            ),
        );
    }

    #[test]
    fn the_pairing_uri_round_trips_through_a_naive_decoder() {
        // A tiny inverse of `pairing_uri`, proving the format is self-consistent (the real
        // inverse is the host-tested Kotlin `PairingUri.parse`).
        let uri = pairing_uri("https://pc.example:443", "7K9MP2QW", None);
        let query = uri.strip_prefix("idiolect://pair?").expect("scheme + path");
        let mut base = None;
        let mut code = None;
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=').expect("k=v");
            match key {
                "u" => {
                    base = Some(
                        percent_encoding::percent_decode_str(value)
                            .decode_utf8()
                            .expect("utf8")
                            .into_owned(),
                    );
                }
                "c" => code = Some(value.to_owned()),
                other => panic!("unexpected key {other}"),
            }
        }
        assert_eq!(base.as_deref(), Some("https://pc.example:443"));
        assert_eq!(code.as_deref(), Some("7K9MP2QW"));
    }

    #[test]
    fn qr_matrix_is_a_square_with_finder_modules() {
        let (matrix, width) = qr_matrix("idiolect://pair?u=x&c=ABCD1234").expect("encode");
        assert_eq!(matrix.len(), width * width, "the matrix is square");
        assert!(
            width >= 21 && width % 2 == 1,
            "QR widths are odd, ≥21: {width}"
        );
        assert!(matrix.iter().any(|&dark| dark), "finder patterns are dark");
        assert!(matrix.iter().any(|&dark| !dark), "data modules vary");
    }

    #[test]
    fn the_announcement_carries_the_pin_when_tls_is_on() {
        // TLS on (the default): the QR encodes `u`+`c`+`f`, and the human-readable fallback
        // also prints the pin so the operator can verify it out-of-band.
        let fingerprint = "0123456789abcdef".repeat(4);
        let announcement =
            pairing_announcement("https://10.0.2.2:8765", "ABCD1234", Some(&fingerprint));
        assert!(
            announcement.contains('\u{2580}'),
            "the scannable QR is present"
        );
        assert!(
            announcement.contains("https://10.0.2.2:8765"),
            "the URL is printed for manual entry"
        );
        assert!(announcement.contains("ABCD-1234"), "the code is grouped");
        assert!(
            announcement.contains(&fingerprint),
            "the pin is shown for verification: {announcement}"
        );
    }

    #[test]
    fn the_announcement_prints_the_tappable_link_verbatim() {
        // The QR already carries the pairing URI, but a headless/emulator operator — or anyone
        // sharing the link to *tap* on a phone instead of scanning — needs it as copyable text,
        // not only as a QR. The printed link must be byte-identical to what the QR encodes
        // (pairing_uri), so a scan and a tap enrol the same device the same way.
        let fingerprint = "0123456789abcdef".repeat(4);
        let uri = pairing_uri("https://10.0.2.2:8765", "ABCD1234", Some(&fingerprint));
        let announcement =
            pairing_announcement("https://10.0.2.2:8765", "ABCD1234", Some(&fingerprint));
        assert!(
            announcement.contains(&uri),
            "the announcement prints the exact pairing link the QR encodes: {announcement}"
        );
    }

    #[test]
    fn the_announcement_omits_the_pin_without_tls() {
        // `--no-tls`: cleartext fallback, no fingerprint to carry or show.
        let announcement = pairing_announcement("http://10.0.2.2:8765", "ABCD1234", None);
        assert!(
            !announcement.to_lowercase().contains("pin"),
            "no pin line in the cleartext fallback: {announcement}"
        );
    }

    #[test]
    fn render_dense_packs_two_modules_per_glyph_with_forced_colours() {
        // A 2×2 checker: top row [dark, light], bottom row [light, dark]. One glyph line.
        //   col 0: upper dark → fg black(30), lower light → bg white(47)
        //   col 1: upper light → fg white(37), lower dark → bg black(40)
        let modules = [true, false, false, true];
        assert_eq!(
            render_dense(&modules, 2),
            "\u{1b}[30;47m\u{2580}\u{1b}[37;40m\u{2580}\u{1b}[0m\n",
        );
    }

    #[test]
    fn render_dense_pairs_an_odd_final_row_against_white() {
        // A single dark module (1×1): the absent lower half is light, so background white.
        assert_eq!(render_dense(&[true], 1), "\u{1b}[30;47m\u{2580}\u{1b}[0m\n");
    }

    #[test]
    fn padding_frames_the_matrix_in_a_light_border() {
        // One dark module, quiet zone 2 → a 5×5 with only the centre dark.
        let (bordered, side) = padded(&[true], 1, 2);
        assert_eq!(side, 5);
        assert_eq!(bordered.iter().filter(|&&dark| dark).count(), 1);
        assert!(bordered[2 * 5 + 2], "the lone module sits at the centre");
        for corner in [0, 4, 20, 24] {
            assert!(!bordered[corner], "corners are quiet (light)");
        }
    }

    #[test]
    fn the_rendered_qr_opens_with_an_all_light_quiet_zone() {
        // The top quiet rows render to all-white glyphs — no black (no `30`/`40`) appears
        // until the finder pattern starts, which is how a reader finds the border.
        let rendered = render_qr("idiolect://pair?u=x&c=ABCD1234").expect("render");
        let first_line = rendered.lines().next().expect("a first line");
        assert!(
            !first_line.contains("30") && !first_line.contains("40"),
            "the quiet zone's first line is fully light: {first_line:?}"
        );
        assert!(first_line.contains('\u{2580}'), "it is made of half-blocks");
    }
}
