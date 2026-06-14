//! The settings window's taskbar / alt-tab icon: the same Idiolect line-art
//! microphone the tray shows, drawn here filled in the accent colour so it reads
//! as an app icon at small sizes. Rendered at runtime with `tiny-skia` so it
//! needs no image asset and stays a pure-Rust dependency, mirroring the tray
//! glyph in `idiolect-adapter-ksni`'s `icons.rs` (the geometry is intentionally
//! the same family; this is a separate render target — straight RGBA, filled —
//! not shared business logic).

use eframe::egui;
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform,
};

const SIZE: u32 = 64;
const ACCENT: (u8, u8, u8) = (124, 131, 253);

fn rgb(c: (u8, u8, u8)) -> Color {
    Color::from_rgba8(c.0, c.1, c.2, 255)
}

/// The window icon: a filled accent microphone on a transparent background, as
/// the straight (non-premultiplied) RGBA `eframe`/`egui` expects.
pub(crate) fn window_icon() -> egui::IconData {
    let mut pixmap = Pixmap::new(SIZE, SIZE).expect("64x64 pixmap allocates");

    let mut fill = Paint::default();
    fill.set_color(rgb(ACCENT));
    fill.anti_alias = true;
    let mut stroke_paint = Paint::default();
    stroke_paint.set_color(rgb(ACCENT));
    stroke_paint.anti_alias = true;
    let stroke = Stroke {
        width: 4.0,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    };

    // Microphone capsule (body) — filled and outlined.
    let body = capsule(22.0, 8.0, 20.0, 30.0);
    pixmap.fill_path(&body, &fill, FillRule::Winding, Transform::identity(), None);
    pixmap.stroke_path(&body, &stroke_paint, &stroke, Transform::identity(), None);

    // Cradle (the U under the mic), stem, and base — stroked.
    let cradle = {
        let mut pb = PathBuilder::new();
        pb.move_to(14.0, 34.0);
        pb.cubic_to(14.0, 50.0, 50.0, 50.0, 50.0, 34.0);
        pb.finish().expect("cradle path")
    };
    pixmap.stroke_path(&cradle, &stroke_paint, &stroke, Transform::identity(), None);

    let stem = {
        let mut pb = PathBuilder::new();
        pb.move_to(32.0, 47.0);
        pb.line_to(32.0, 54.0);
        pb.finish().expect("stem path")
    };
    pixmap.stroke_path(&stem, &stroke_paint, &stroke, Transform::identity(), None);

    let base = {
        let mut pb = PathBuilder::new();
        pb.move_to(23.0, 55.0);
        pb.line_to(41.0, 55.0);
        pb.finish().expect("base path")
    };
    pixmap.stroke_path(&base, &stroke_paint, &stroke, Transform::identity(), None);

    egui::IconData {
        rgba: to_straight_rgba(&pixmap),
        width: SIZE,
        height: SIZE,
    }
}

/// A vertical capsule (rounded rect with semicircular ends) as a path.
fn capsule(x: f32, y: f32, w: f32, h: f32) -> tiny_skia::Path {
    let r = w / 2.0;
    let mut pb = PathBuilder::new();
    pb.move_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.close();
    pb.finish().expect("capsule path")
}

/// tiny-skia stores premultiplied RGBA; egui wants straight RGBA bytes
/// `[R, G, B, A]` per pixel, so un-premultiply each opaque-ish pixel.
fn to_straight_rgba(pixmap: &Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixmap.data().len());
    for px in pixmap.pixels() {
        let a = px.alpha();
        let (r, g, b) = if a == 0 {
            (0, 0, 0)
        } else {
            let unmul = |c: u8| ((u16::from(c) * 255) / u16::from(a)) as u8;
            (unmul(px.red()), unmul(px.green()), unmul(px.blue()))
        };
        out.extend_from_slice(&[r, g, b, a]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_icon_is_64px_rgba_with_a_drawn_glyph_on_transparency() {
        let icon = window_icon();
        assert_eq!((icon.width, icon.height), (SIZE, SIZE));
        assert_eq!(icon.rgba.len() as u32, SIZE * SIZE * 4, "RGBA, 4 bytes/px");

        // The corners are background — fully transparent.
        let corner_alpha = icon.rgba[3];
        assert_eq!(corner_alpha, 0, "top-left corner must be transparent");

        // The glyph is actually drawn: some pixels are (near-)opaque accent.
        let opaque_accent = icon
            .rgba
            .chunks_exact(4)
            .any(|p| p[3] > 200 && p[0].abs_diff(ACCENT.0) < 24 && p[2].abs_diff(ACCENT.2) < 24);
        assert!(
            opaque_accent,
            "icon must contain the filled accent microphone"
        );
    }
}
