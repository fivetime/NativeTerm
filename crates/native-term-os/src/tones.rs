//! Chrome's colours for a tab under the pointer on a desktop theme
//! (`kColorSysStateHeaderHover`, `kColorSysStateOnHeaderHover`, see
//! chromium's native_chrome_color_mixer_linux.cc and sys_color_mixer.cc):
//! tones of the Material palette seeded by the desktop's accent — tone 80
//! of the primary palette in light, 30 of the secondary in dark — or,
//! without an accent, Chrome's fixed baseline (a light blue).
//!
//! Chrome's palettes are HCT ("tonal spot": primary chroma 40, secondary
//! 16). HCT's tone is CIELAB's L*, so a tone here is the accent's hue at
//! that L* with that chroma in CIELAB, pulled in until it fits sRGB: close
//! to Chrome's, without CAM16.

use crate::titlebar::Rgb;

/// Chrome's baseline tones (ref_color_mixer.cc): primary 80 and 20,
/// secondary 30 and 90.
const PRIMARY_80: Rgb = (0xA8, 0xC7, 0xFA);
const PRIMARY_20: Rgb = (0x06, 0x2E, 0x6F);
const SECONDARY_30: Rgb = (0x00, 0x4A, 0x77);
const SECONDARY_90: Rgb = (0xC2, 0xE7, 0xFF);

/// The tonal-spot scheme's chroma for the primary and secondary palettes.
const PRIMARY_CHROMA: f64 = 40.;
const SECONDARY_CHROMA: f64 = 16.;

/// The fill of a tab under the pointer and its text, for a light or dark
/// theme, from the desktop's accent if it has one.
pub fn header_hover(accent: Option<Rgb>, dark: bool) -> (Rgb, Rgb) {
    match (accent, dark) {
        (None, false) => (PRIMARY_80, PRIMARY_20),
        (None, true) => (SECONDARY_30, SECONDARY_90),
        (Some(seed), false) => (tone(seed, 80., PRIMARY_CHROMA), tone(seed, 20., PRIMARY_CHROMA)),
        (Some(seed), true) => (tone(seed, 30., SECONDARY_CHROMA), tone(seed, 90., SECONDARY_CHROMA)),
    }
}

/// Whether a colour is a dark one (its tone under 50), as Chrome decides
/// a theme is dark from its colours.
pub fn is_dark(c: Rgb) -> bool {
    lab(c).0 < 50.
}

/// `seed`'s hue at tone `l` with `chroma`, as much of it as sRGB holds (a
/// grey seed stays grey).
pub fn tone(seed: Rgb, l: f64, chroma: f64) -> Rgb {
    let (_, a, b) = lab(seed);
    let seed_chroma = a.hypot(b);
    let chroma = if seed_chroma < 1. { 0. } else { chroma };
    let hue = b.atan2(a);
    let mut c = chroma;
    loop {
        if let Some(rgb) = from_lab(l, c * hue.cos(), c * hue.sin()) {
            return rgb;
        }
        c -= 0.5;
        if c <= 0. {
            return from_lab(l, 0., 0.).unwrap_or((0, 0, 0));
        }
    }
}

fn linear(v: u8) -> f64 {
    let v = f64::from(v) / 255.;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn gamma(v: f64) -> f64 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1. / 2.4) - 0.055
    }
}

/// D65 white.
const WHITE: (f64, f64, f64) = (0.95047, 1., 1.08883);

fn lab((r, g, b): Rgb) -> (f64, f64, f64) {
    let (r, g, b) = (linear(r), linear(g), linear(b));
    let x = 0.4124 * r + 0.3576 * g + 0.1805 * b;
    let y = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let z = 0.0193 * r + 0.1192 * g + 0.9505 * b;
    let f = |t: f64| if t > 216. / 24389. { t.cbrt() } else { (24389. / 27. * t + 16.) / 116. };
    let (fx, fy, fz) = (f(x / WHITE.0), f(y / WHITE.1), f(z / WHITE.2));
    (116. * fy - 16., 500. * (fx - fy), 200. * (fy - fz))
}

/// The sRGB colour at these CIELAB coordinates, `None` outside sRGB.
fn from_lab(l: f64, a: f64, b: f64) -> Option<Rgb> {
    let fy = (l + 16.) / 116.;
    let (fx, fz) = (fy + a / 500., fy - b / 200.);
    let inv = |t: f64| if t.powi(3) > 216. / 24389. { t.powi(3) } else { (116. * t - 16.) * 27. / 24389. };
    let (x, y, z) = (inv(fx) * WHITE.0, inv(fy) * WHITE.1, inv(fz) * WHITE.2);
    let r = 3.2406 * x - 1.5372 * y - 0.4986 * z;
    let g = -0.9689 * x + 1.8758 * y + 0.0415 * z;
    let b = 0.0557 * x - 0.2040 * y + 1.0570 * z;
    let channel = |v: f64| {
        let v = gamma(v);
        (-0.001..=1.001).contains(&v).then(|| (v.clamp(0., 1.) * 255.).round() as u8)
    };
    Some((channel(r)?, channel(g)?, channel(b)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chromes_baseline_without_an_accent() {
        assert_eq!(header_hover(None, false), ((0xA8, 0xC7, 0xFA), (0x06, 0x2E, 0x6F)));
        assert_eq!(header_hover(None, true), ((0x00, 0x4A, 0x77), (0xC2, 0xE7, 0xFF)));
    }

    #[test]
    fn tones_keep_the_accents_hue_at_their_lightness() {
        // GNOME's blue accent: a light blue in light, a dark blue in dark
        let blue = (0x35, 0x84, 0xE4);
        let (light, text) = header_hover(Some(blue), false);
        assert!((lab(light).0 - 80.).abs() < 1., "{light:?}");
        assert!(light.2 > light.0 && light.2 > light.1, "still blue: {light:?}");
        assert!((lab(text).0 - 20.).abs() < 1., "{text:?}");
        let (dark, text) = header_hover(Some(blue), true);
        assert!((lab(dark).0 - 30.).abs() < 1., "{dark:?}");
        assert!(dark.2 > dark.0, "{dark:?}");
        assert!((lab(text).0 - 90.).abs() < 1.);
        // a grey accent stays grey
        let (grey, _) = header_hover(Some((0x80, 0x80, 0x80)), false);
        assert!(grey.0.abs_diff(grey.2) <= 1, "{grey:?}");
    }

    #[test]
    fn dark_and_light() {
        assert!(is_dark((0x24, 0x24, 0x24)));
        assert!(!is_dark((0xEB, 0xEB, 0xED)));
    }

    #[test]
    fn lab_round_trips() {
        for c in [(0, 0, 0), (255, 255, 255), (0x35, 0x84, 0xE4), (0xC0, 0x1C, 0x28)] {
            let (l, a, b) = lab(c);
            assert_eq!(from_lab(l, a, b), Some(c));
        }
    }
}
