//! The pictures of the systems a server may be of (`assets/os`, see the
//! README there for where they are from), as a row of the tree shows
//! them in place of the picture any host has.
//!
//! Each is kept 96 pixels a side and brought to the size it is shown at
//! (a row's picture, in the screen's own pixels) by the mean of what each
//! pixel covers, so it is as sharp as its size lets it be at any scale.
//! A logo that comes in its own colours is drawn as it is. One that is a
//! shape only is drawn in a colour: the system's own where it is known,
//! the colour of any host's picture otherwise.

use std::collections::HashMap;

use native_term_app::server::Os;

use crate::looks::Tones;

/// A picture's side as it is kept.
const KEPT: usize = 96;

/// (the picture, whether it is a shape only).
fn kept(os: Os) -> (&'static [u8], bool) {
    match os {
        Os::Arch => (include_bytes!("../assets/os/arch.png"), false),
        Os::CentOs => (include_bytes!("../assets/os/centos.png"), false),
        Os::Debian => (include_bytes!("../assets/os/debian.png"), false),
        Os::Fedora => (include_bytes!("../assets/os/fedora.png"), false),
        Os::FreeBsd => (include_bytes!("../assets/os/freebsd.png"), false),
        Os::Linux => (include_bytes!("../assets/os/linux.png"), false),
        Os::Manjaro => (include_bytes!("../assets/os/manjaro.png"), false),
        Os::Mint => (include_bytes!("../assets/os/linuxmint.png"), false),
        Os::Raspbian => (include_bytes!("../assets/os/raspbian.png"), false),
        Os::Rhel => (include_bytes!("../assets/os/rhel.png"), false),
        Os::Rocky => (include_bytes!("../assets/os/rocky.png"), false),
        Os::Ubuntu => (include_bytes!("../assets/os/ubuntu.png"), false),
        Os::Windows => (include_bytes!("../assets/os/windows.png"), false),
        Os::Zorin => (include_bytes!("../assets/os/zorin.png"), false),
        Os::AlmaLinux => (include_bytes!("../assets/os/almalinux.png"), true),
        Os::Alpine => (include_bytes!("../assets/os/alpine.png"), true),
        Os::Deepin => (include_bytes!("../assets/os/deepin.png"), true),
        Os::Elementary => (include_bytes!("../assets/os/elementary.png"), true),
        Os::EndeavourOs => (include_bytes!("../assets/os/endeavouros.png"), true),
        Os::Kali => (include_bytes!("../assets/os/kali.png"), true),
        Os::MacOs => (include_bytes!("../assets/os/macos.png"), true),
        Os::OpenBsd => (include_bytes!("../assets/os/openbsd.png"), true),
        Os::Suse => (include_bytes!("../assets/os/suse.png"), true),
    }
}

/// What a picture is drawn with: nothing of its own colours changed, or
/// the colour a shape is given. A shape has its system's colour where
/// that was read from the system's own logo, and the colour of any
/// host's picture otherwise:
///
/// - Deepin: its logo's two blues
///   (`/usr/share/deepin/distribution/distribution_logo.svg`, Deepin
///   25), the lighter on a dark row, the darker on a light one;
/// - SUSE: its logo's green and its dark (gilbarbara's `suse.svg`), the
///   green on a dark row;
/// - EndeavourOS: its logo's violet
///   (`/usr/share/pixmaps/endeavouros-logo.svg` there);
/// - macOS and elementary OS: their logos are of one colour, which is
///   the text's here (Apple's is black or white, elementary's dark).
#[must_use]
pub fn tint(os: Os, tones: &Tones, dark: bool) -> egui::Color32 {
    let rgb = |hex: u32| egui::Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
    match (kept(os).1, os) {
        (false, _) => egui::Color32::WHITE,
        (true, Os::Deepin) => rgb(if dark { 0x3e_a9fd } else { 0x26_59b8 }),
        (true, Os::Suse) => rgb(if dark { 0x02_d35f } else { 0x0d_2c40 }),
        (true, Os::EndeavourOs) => rgb(0x7d_7dff),
        (true, Os::MacOs | Os::Elementary) => tones.text,
        (true, _) => tones.host,
    }
}

/// The picture's pixels (red, green, blue and how much of them there is,
/// not multiplied), `KEPT` a side.
fn pixels(os: Os) -> Option<Vec<u8>> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(kept(os).0));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut pixels = vec![0; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut pixels).ok()?;
    let fits = frame.color_type == png::ColorType::Rgba
        && (frame.width as usize, frame.height as usize) == (KEPT, KEPT)
        && frame.buffer_size() == KEPT * KEPT * 4;
    fits.then_some(pixels)
}

/// A picture `from` a side brought to `to` a side: each pixel the mean of
/// what it covers, by how much of each it covers. Colours are multiplied
/// by how much of them there is, before and after (as egui has them).
#[must_use]
pub fn sized(pixels: &[u8], from: usize, to: usize) -> Vec<u8> {
    let step = from as f32 / to as f32;
    // what a pixel covers along one side: (which, how much of it)
    let covered = |at: usize| {
        let (start, end) = (at as f32 * step, (at as f32 + 1.0) * step);
        let last = (end.ceil() as usize).min(from);
        (start.floor() as usize..last).map(move |i| (i, (end.min(i as f32 + 1.0) - start.max(i as f32)).max(0.0)))
    };
    let mut out = Vec::with_capacity(to * to * 4);
    for y in 0..to {
        for x in 0..to {
            let mut sum = [0.0_f32; 4];
            let mut whole = 0.0;
            for (row, high) in covered(y) {
                for (column, wide) in covered(x) {
                    let pixel = &pixels[(row * from + column) * 4..][..4];
                    let (share, there) = (high * wide, f32::from(pixel[3]) / 255.0);
                    for (sum, colour) in sum.iter_mut().zip(pixel).take(3) {
                        *sum += share * there * f32::from(*colour);
                    }
                    sum[3] += share * f32::from(pixel[3]);
                    whole += share;
                }
            }
            out.extend(sum.map(|sum| (sum / whole.max(f32::EPSILON)).round().clamp(0.0, 255.0) as u8));
        }
    }
    out
}

/// The pictures made so far, by system and size.
#[derive(Default)]
pub struct Logos {
    made: HashMap<(Os, usize), Option<egui::TextureHandle>>,
}

impl Logos {
    /// The picture of `os`, to be drawn `points` a side.
    pub fn picture(&mut self, ctx: &egui::Context, os: Os, points: f32) -> Option<egui::TextureId> {
        let side = ((points * ctx.pixels_per_point()).round() as usize).clamp(1, KEPT);
        let made = self.made.entry((os, side)).or_insert_with(|| {
            let image = egui::ColorImage::from_rgba_premultiplied([side, side], &sized(&pixels(os)?, KEPT, side));
            let name = format!("os-{}-{side}", os.name());
            Some(ctx.load_texture(name, image, egui::TextureOptions::LINEAR))
        });
        made.as_ref().map(egui::TextureHandle::id)
    }
}

/// Where a picture `points` a side is drawn with its middle at `middle`:
/// on the screen's own pixels, so that none of its pixels is spread over
/// two.
#[must_use]
pub fn place(ctx: &egui::Context, middle: egui::Pos2, points: f32) -> egui::Rect {
    let scale = ctx.pixels_per_point();
    let side = (points * scale).round() / scale;
    let corner = middle - egui::Vec2::splat(side / 2.0);
    let corner = egui::pos2((corner.x * scale).round() / scale, (corner.y * scale).round() / scale);
    egui::Rect::from_min_size(corner, egui::Vec2::splat(side))
}

/// The picture drawn.
pub fn paint(painter: &egui::Painter, picture: egui::TextureId, place: egui::Rect, tint: egui::Color32) {
    let whole = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    painter.image(picture, place, whole, tint);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_system_has_its_picture() {
        for os in Os::ALL {
            let pixels = pixels(os).unwrap_or_else(|| panic!("{os:?}: not a picture of {KEPT} a side"));
            let there = pixels.chunks(4).filter(|p| p[3] > 0).count();
            assert!(there > KEPT * KEPT / 8, "{os:?}: something is drawn ({there})");
            if kept(os).1 {
                // a shape is white, to take the colour it is drawn with
                assert!(pixels.chunks(4).all(|p| p[3] == 0 || p[..3] == [255, 255, 255]), "{os:?}");
            }
            // as a row shows it at 100 %, 125 %, 150 %, 200 %
            for side in [16, 20, 24, 32] {
                let small = sized(&pixels, KEPT, side);
                assert_eq!(small.len(), side * side * 4);
                assert!(small.chunks(4).any(|p| p[3] > 128), "{os:?} at {side}");
                assert!(small.chunks(4).all(|p| p[..3].iter().all(|c| *c <= p[3])), "{os:?} at {side}: multiplied");
            }
        }
    }

    #[test]
    fn a_picture_is_brought_to_its_size_by_the_mean() {
        // four pixels a side: the left half red and all there, the right nothing
        let mut pixels = Vec::new();
        for _ in 0..4 {
            pixels.extend([255, 0, 0, 255, 255, 0, 0, 255, 0, 0, 0, 0, 0, 0, 0, 0]);
        }
        assert_eq!(sized(&pixels, 4, 2), [255, 0, 0, 255, 0, 0, 0, 0, 255, 0, 0, 255, 0, 0, 0, 0]);
        // one pixel: half of it there, its colour half as much with it
        assert_eq!(sized(&pixels, 4, 1), [128, 0, 0, 128]);
        // three a side: the middle one covers as much of red as of nothing
        let three = sized(&pixels, 4, 3);
        assert_eq!(three[..4], [255, 0, 0, 255]);
        // (half, as nearly as a third can be said in numbers)
        assert!(matches!(three[4..8], [127..=128, 0, 0, 127..=128]), "{:?}", &three[4..8]);
        assert_eq!(three[8..12], [0, 0, 0, 0]);
        // and as it is where nothing changes
        assert_eq!(sized(&pixels, 4, 4), pixels);
    }
}
