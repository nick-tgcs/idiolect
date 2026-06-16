//! The operator-facing pairing QR, end to end: the URI we encode, the QR a phone
//! camera would see, and the human-readable fallback printed beside it.
//!
//! The e2e here is a genuine encode→decode round-trip: we rasterise the rendered QR to
//! a greyscale buffer (what a camera sensor delivers) and decode it with an independent
//! QR reader (`rqrr`), asserting the bytes come back as exactly the pairing URI. That
//! proves the QR a phone scans carries the working `(baseUrl, code)` — the contract the
//! Android `PairingUri.parse` consumes. The unit level (URI format, the dense ANSI
//! renderer, the quiet zone) lives beside the code in `src/pairing_qr.rs`.

use idiolect_sync_server::pairing_qr::{pairing_announcement, pairing_uri, qr_matrix};

/// Rasterise a module matrix to a greyscale image (0 = black/dark, 255 = white/light)
/// with `scale` pixels per module and a `quiet`-module white border — the quiet zone a
/// decoder needs to lock on. Returns `(width_px, height_px, pixels)` row-major.
fn rasterise(dark: &[bool], width: usize, scale: usize, quiet: usize) -> (usize, usize, Vec<u8>) {
    let side = width + 2 * quiet;
    let px = side * scale;
    let mut pixels = vec![255u8; px * px];
    for (i, &is_dark) in dark.iter().enumerate() {
        if !is_dark {
            continue;
        }
        let (mx, my) = (i % width + quiet, i / width + quiet);
        for dy in 0..scale {
            for dx in 0..scale {
                let (x, y) = (mx * scale + dx, my * scale + dy);
                pixels[y * px + x] = 0;
            }
        }
    }
    (px, px, pixels)
}

#[test]
fn the_rendered_qr_decodes_back_to_the_exact_pairing_uri() {
    let base = "http://100.64.0.7:8765";
    let code = "7K9MP2QW";
    let uri = pairing_uri(base, code);

    let (dark, width) = qr_matrix(&uri).expect("encode the pairing URI as a QR");
    let (w, h, pixels) = rasterise(&dark, width, 8, 4);

    let mut image = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| pixels[y * w + x]);
    let grids = image.detect_grids();
    assert_eq!(grids.len(), 1, "exactly one QR is present in the render");
    let (_meta, decoded) = grids[0].decode().expect("decode the rendered QR");

    assert_eq!(
        decoded, uri,
        "what a phone camera reads is exactly the pairing URI we encoded"
    );
}

#[test]
fn the_announcement_carries_a_scannable_qr_and_a_typeable_fallback() {
    let base = "http://100.64.0.7:8765";
    // The raw 8-char code the pairing state mints; the announcement groups it for reading.
    let code = "7K9MP2QW";
    let announcement = pairing_announcement(base, code);

    // The QR block (forced-colour half blocks) is present for scanning...
    assert!(
        announcement.contains('\u{2580}'),
        "the announcement embeds the QR (upper-half-block glyphs): {announcement}"
    );
    // ...and the manual fallback gives a human the clean URL and the grouped code, so a
    // device with no camera can still pair by hand.
    assert!(
        announcement.contains(base),
        "the clean URL is printed for manual entry: {announcement}"
    );
    assert!(
        announcement.contains("7K9M-P2QW"),
        "the code is grouped for legibility: {announcement}"
    );
}
