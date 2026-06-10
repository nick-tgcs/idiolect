//! Custom tray icons rendered at runtime with `tiny-skia` (pure Rust, no system
//! deps), so the tray shows a crisp, modern line-art microphone instead of a
//! generic theme glyph. Idle is a monochrome outline; recording fills it with
//! the accent and adds a red dot; error tints it red.

use idiolect_ports::storage::TrayIcon;
use tiny_skia::{
    Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform,
};

const SIZE: u32 = 64;
const IDLE: (u8, u8, u8) = (228, 230, 240);
const ACCENT: (u8, u8, u8) = (124, 131, 253);
const RECORD_DOT: (u8, u8, u8) = (244, 63, 94);
const ERROR: (u8, u8, u8) = (244, 63, 94);

fn rgb(c: (u8, u8, u8)) -> Color {
    Color::from_rgba8(c.0, c.1, c.2, 255)
}

/// Render the given tray state to an `ksni::Icon` (ARGB32, network byte order).
pub(crate) fn render(kind: TrayIcon) -> ksni::Icon {
    let mut pixmap = Pixmap::new(SIZE, SIZE).expect("64x64 pixmap allocates");

    let (stroke_color, fill) = match kind {
        TrayIcon::Idle => (IDLE, None),
        TrayIcon::Recording => (ACCENT, Some(ACCENT)),
        TrayIcon::Error => (ERROR, None),
    };

    // Microphone capsule (body).
    let body = capsule(22.0, 8.0, 20.0, 30.0);
    if let Some(fill_color) = fill {
        let mut paint = Paint::default();
        paint.set_color(rgb(fill_color));
        paint.anti_alias = true;
        pixmap.fill_path(
            &body,
            &paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let mut stroke_paint = Paint::default();
    stroke_paint.set_color(rgb(stroke_color));
    stroke_paint.anti_alias = true;
    let stroke = Stroke {
        width: 4.0,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    };
    pixmap.stroke_path(&body, &stroke_paint, &stroke, Transform::identity(), None);

    // Cradle (the U under the mic), stem, and base — always stroked.
    let cradle = {
        let mut pb = PathBuilder::new();
        pb.move_to(14.0, 34.0);
        // Open-bottom arc approximated with a cubic.
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

    // Recording: a red dot in the top-right corner.
    if matches!(kind, TrayIcon::Recording) {
        if let Some(dot) = PathBuilder::from_circle(50.0, 14.0, 8.0) {
            let mut paint = Paint::default();
            paint.set_color(rgb(RECORD_DOT));
            paint.anti_alias = true;
            pixmap.fill_path(&dot, &paint, FillRule::Winding, Transform::identity(), None);
        }
    }

    ksni::Icon {
        width: SIZE as i32,
        height: SIZE as i32,
        data: to_argb_network(&pixmap),
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

/// tiny-skia stores premultiplied RGBA; the SNI host wants straight ARGB32 in
/// network (big-endian) byte order, i.e. bytes `[A, R, G, B]` per pixel.
fn to_argb_network(pixmap: &Pixmap) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixmap.data().len());
    for px in pixmap.pixels() {
        let a = px.alpha();
        let (r, g, b) = if a == 0 {
            (0, 0, 0)
        } else {
            let unmul = |c: u8| ((u16::from(c) * 255) / u16::from(a)) as u8;
            (unmul(px.red()), unmul(px.green()), unmul(px.blue()))
        };
        out.extend_from_slice(&[a, r, g, b]);
    }
    out
}
