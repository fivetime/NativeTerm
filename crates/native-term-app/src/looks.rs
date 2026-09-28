//! Named looks, on top of light and dark: how solid the colours are, how
//! much room a row takes, whether the highlight follows the Windows
//! accent colour. One setting (`theme.preset`), applied to every window
//! NativeTerm opens.
//!
//! A look changes nothing but `egui`'s style, so it costs nothing to
//! switch and nothing to keep.

use std::sync::Mutex;

use native_term_app::t;

/// Where the chosen look is kept (`settings.toml`, shared between
/// machines: it is a preference, not a property of this computer).
pub const SETTING: &str = "theme.preset";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Preset {
    /// egui's own colours and spacing.
    #[default]
    Plain,
    /// The Windows accent colour for what is selected, as the rest of
    /// the desktop uses it.
    Accent,
    /// Softer: less contrast, quieter lines — for a dark room.
    Dim,
    /// The same colours, less room per row: more hosts on screen.
    Compact,
    /// A dark editor's colours (slate and blue), round corners, more
    /// room, the tree's rows as such a program draws them: after the
    /// design the person brought (`docs/ARCHITECTURE.md`, "The modern
    /// look"). Its light side is the same scales' light end.
    Modern,
}

/// The look every window follows (the floating button has its own egui
/// context and reads it too).
static CHOSEN: Mutex<Option<Preset>> = Mutex::new(None);

impl Preset {
    pub const ALL: [Preset; 5] = [Preset::Plain, Preset::Accent, Preset::Dim, Preset::Compact, Preset::Modern];

    /// What is written in the settings (the plain look writes nothing).
    #[must_use]
    pub fn setting(self) -> &'static str {
        match self {
            Preset::Plain => "",
            Preset::Accent => "accent",
            Preset::Dim => "dim",
            Preset::Compact => "compact",
            Preset::Modern => "modern",
        }
    }

    #[must_use]
    pub fn from_setting(text: Option<&str>) -> Preset {
        Preset::ALL.into_iter().find(|p| p.setting() == text.unwrap_or("")).unwrap_or_default()
    }

    #[must_use]
    pub fn label(self) -> String {
        match self {
            Preset::Plain => t!("theme-look-plain"),
            Preset::Accent => t!("theme-look-accent"),
            Preset::Dim => t!("theme-look-dim"),
            Preset::Compact => t!("theme-look-compact"),
            Preset::Modern => t!("theme-look-modern"),
        }
    }

    /// Give this look to `ctx` (both themes, so switching light and dark
    /// keeps it).
    pub fn apply(self, ctx: &egui::Context) {
        let accent = accent_color();
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            let dark = theme == egui::Theme::Dark;
            let mut visuals = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
            let mut spacing = egui::style::Spacing::default();
            match self {
                Preset::Plain => {}
                Preset::Accent => {
                    if let Some(accent) = accent {
                        let accent = if dark { lighter(accent) } else { accent };
                        visuals.selection.bg_fill = accent;
                        visuals.selection.stroke.color = readable_on(accent);
                        visuals.hyperlink_color = if dark { lighter(accent) } else { accent };
                        visuals.widgets.hovered.bg_stroke.color = accent;
                    }
                }
                Preset::Dim => {
                    if dark {
                        visuals.panel_fill = gray(0x1a);
                        visuals.window_fill = gray(0x20);
                        visuals.extreme_bg_color = gray(0x14);
                        visuals.override_text_color = Some(gray(0xc4));
                    } else {
                        // not white: a page-white window is the glare
                        visuals.panel_fill = gray(0xee);
                        visuals.window_fill = gray(0xf4);
                        visuals.extreme_bg_color = gray(0xe4);
                        visuals.override_text_color = Some(gray(0x33));
                    }
                    visuals.widgets.noninteractive.bg_stroke.color = if dark { gray(0x30) } else { gray(0xd8) };
                }
                Preset::Compact => {
                    spacing.item_spacing = egui::vec2(6.0, 3.0);
                    spacing.button_padding = egui::vec2(4.0, 2.0);
                    spacing.interact_size.y = 20.0;
                    spacing.indent = 14.0;
                    spacing.menu_margin = egui::Margin::same(4);
                }
                Preset::Modern => {
                    Palette::of(dark).paint(&mut visuals);
                    spacing.item_spacing = egui::vec2(8.0, 6.0);
                    spacing.button_padding = egui::vec2(10.0, 5.0);
                    spacing.interact_size.y = 26.0;
                    spacing.menu_margin = egui::Margin::same(6);
                }
            }
            ctx.set_visuals_of(theme, visuals);
            ctx.style_mut_of(theme, |style| style.spacing = spacing.clone());
        }
    }

    /// Remember it for every window, and give it to this one.
    pub fn choose(self, ctx: &egui::Context) {
        *CHOSEN.lock().unwrap_or_else(|e| e.into_inner()) = Some(self);
        self.apply(ctx);
    }
}

/// The look the windows should be showing, if one was chosen.
#[must_use]
pub fn chosen() -> Option<Preset> {
    *CHOSEN.lock().unwrap_or_else(|e| e.into_inner())
}

/// The modern look's colours. The dark ones are the design's own (its
/// `nexus` colours, and Tailwind's slate and blue, which it names the
/// rest by); the design has no light side, which is here the same two
/// scales from their other end.
struct Palette {
    /// The window, the bars along its sides, what is typed into.
    page: egui::Color32,
    bar: egui::Color32,
    field: egui::Color32,
    /// A button, and one the pointer is on.
    card: egui::Color32,
    raised: egui::Color32,
    /// The lines between things, and around what the pointer is on.
    line: egui::Color32,
    near: egui::Color32,
    text: egui::Color32,
    strong: egui::Color32,
    weak: egui::Color32,
    /// What is chosen, and what leads somewhere.
    accent: egui::Color32,
    link: egui::Color32,
}

const fn rgb(hex: u32) -> egui::Color32 {
    egui::Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// `color` as thin as `alpha` of 255 says.
fn thin(color: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

impl Palette {
    const fn of(dark: bool) -> Palette {
        if dark {
            Palette {
                page: rgb(0x121316),
                bar: rgb(0x16181d),
                field: rgb(0x1a1c22),
                card: rgb(0x22252e),
                raised: rgb(0x2e323e),
                line: rgb(0x2a2e3b),
                near: rgb(0x475569),
                text: rgb(0xcbd5e1),
                strong: rgb(0xffffff),
                weak: rgb(0x94a3b8),
                accent: rgb(0x2563eb),
                link: rgb(0x60a5fa),
            }
        } else {
            Palette {
                page: rgb(0xf8fafc),
                bar: rgb(0xf1f5f9),
                field: rgb(0xffffff),
                card: rgb(0xffffff),
                raised: rgb(0xe2e8f0),
                line: rgb(0xe2e8f0),
                near: rgb(0x94a3b8),
                text: rgb(0x334155),
                strong: rgb(0x0f172a),
                // (slate 600: 500 is too pale to read on the bars)
                weak: rgb(0x475569),
                accent: rgb(0x2563eb),
                link: rgb(0x2563eb),
            }
        }
    }

    fn paint(&self, visuals: &mut egui::Visuals) {
        let round = egui::CornerRadius::same(8);
        visuals.panel_fill = self.page;
        visuals.window_fill = self.field;
        visuals.extreme_bg_color = self.field;
        visuals.faint_bg_color = self.bar;
        visuals.code_bg_color = self.card;
        visuals.window_stroke = egui::Stroke::new(1.0_f32, self.line);
        visuals.window_corner_radius = egui::CornerRadius::same(12);
        visuals.menu_corner_radius = egui::CornerRadius::same(10);
        visuals.weak_text_color = Some(self.weak);
        visuals.hyperlink_color = self.link;
        visuals.selection.bg_fill = thin(self.accent, 0x59);
        visuals.selection.stroke = egui::Stroke::new(1.0_f32, self.link);
        let widgets = &mut visuals.widgets;
        widgets.noninteractive.bg_fill = self.page;
        widgets.noninteractive.weak_bg_fill = self.page;
        widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0_f32, self.line);
        widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, self.text);
        widgets.noninteractive.corner_radius = round;
        widgets.inactive.bg_fill = self.card;
        widgets.inactive.weak_bg_fill = self.card;
        widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, self.line);
        widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, self.text);
        widgets.inactive.corner_radius = round;
        for widget in [&mut widgets.hovered, &mut widgets.active, &mut widgets.open] {
            widget.bg_fill = self.raised;
            widget.weak_bg_fill = self.raised;
            widget.bg_stroke = egui::Stroke::new(1.0_f32, self.near);
            widget.fg_stroke = egui::Stroke::new(1.0_f32, self.strong);
            widget.corner_radius = round;
            widget.expansion = 0.0;
        }
    }
}

/// How the session tree's rows look, where the look has them its own way
/// (the modern one): round, what is chosen in the accent's colour thinly
/// with a line around it, the folders and the hosts each in a colour,
/// lines down the tree's levels.
pub struct Rows {
    pub chosen: egui::Color32,
    pub chosen_line: egui::Color32,
    pub under_pointer: egui::Color32,
    pub chosen_text: egui::Color32,
    pub folder: egui::Color32,
    pub host: egui::Color32,
    pub guide: egui::Color32,
    pub radius: u8,
}

/// The rows of the look chosen, in the dark or the light.
#[must_use]
pub fn rows(dark: bool) -> Option<Rows> {
    if chosen() != Some(Preset::Modern) {
        return None;
    }
    let palette = Palette::of(dark);
    Some(Rows {
        chosen: thin(palette.accent, 0x26),
        chosen_line: thin(rgb(0x3b82f6), 0x4d),
        under_pointer: palette.field,
        chosen_text: palette.strong,
        // (amber and blue 400 in the dark, 600 in the light, where 400 is pale)
        folder: if dark { rgb(0xfbbf24) } else { rgb(0xd97706) },
        host: if dark { rgb(0x60a5fa) } else { rgb(0x2563eb) },
        guide: if dark { thin(egui::Color32::WHITE, 0x0f) } else { thin(egui::Color32::BLACK, 0x14) },
        radius: 8,
    })
}

fn gray(v: u8) -> egui::Color32 {
    egui::Color32::from_gray(v)
}

/// The Windows accent colour, as the taskbar and window borders use it.
fn accent_color() -> Option<egui::Color32> {
    let (r, g, b) = native_term_os::desktop::accent()?;
    Some(egui::Color32::from_rgb(r, g, b))
}

/// Two fifths of the way to white, so a dark accent still shows on a
/// dark window (Windows lightens its own palette the same way).
fn lighter(c: egui::Color32) -> egui::Color32 {
    let mix = |v: u8| v.saturating_add(((255 - v) as u16 * 2 / 5) as u8);
    egui::Color32::from_rgb(mix(c.r()), mix(c.g()), mix(c.b()))
}

/// Black or white, whichever can be read on this colour.
fn readable_on(c: egui::Color32) -> egui::Color32 {
    let light = (u32::from(c.r()) * 299 + u32::from(c.g()) * 587 + u32::from(c.b()) * 114) / 1000 > 140;
    if light {
        egui::Color32::BLACK
    } else {
        egui::Color32::WHITE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_look_survives_the_settings_file() {
        for preset in Preset::ALL {
            assert_eq!(Preset::from_setting(Some(preset.setting())), preset);
        }
        assert_eq!(Preset::from_setting(None), Preset::Plain);
        assert_eq!(Preset::from_setting(Some("")), Preset::Plain);
        assert_eq!(Preset::from_setting(Some("something else")), Preset::Plain, "an unknown look is the plain one");
        assert_eq!(Preset::Plain.setting(), "", "and the plain one writes nothing");
    }

    #[test]
    fn the_modern_look_can_be_read() {
        // the text against what it is on, as WCAG counts contrast: 4.5
        // for text, 3 for what is only a sign
        fn light(c: egui::Color32) -> f32 {
            let one = |v: u8| {
                let v = f32::from(v) / 255.0;
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * one(c.r()) + 0.7152 * one(c.g()) + 0.0722 * one(c.b())
        }
        fn contrast(a: egui::Color32, b: egui::Color32) -> f32 {
            let (a, b) = (light(a), light(b));
            (a.max(b) + 0.05) / (a.min(b) + 0.05)
        }
        for dark in [true, false] {
            let p = Palette::of(dark);
            for ground in [p.page, p.bar, p.field, p.card] {
                assert!(contrast(p.text, ground) >= 4.5, "text, dark {dark}: {}", contrast(p.text, ground));
                assert!(contrast(p.weak, ground) >= 4.5, "weak text, dark {dark}: {}", contrast(p.weak, ground));
                assert!(contrast(p.link, ground) >= 4.5, "a link, dark {dark}: {}", contrast(p.link, ground));
            }
            assert!(contrast(p.strong, p.raised) >= 4.5, "under the pointer, dark {dark}");
        }
        let mut visuals = egui::Visuals::dark();
        Palette::of(true).paint(&mut visuals);
        assert_eq!(visuals.panel_fill, rgb(0x121316), "the design's page");
        assert_eq!(visuals.widgets.inactive.bg_fill, rgb(0x22252e), "its buttons");
        assert_eq!(visuals.widgets.inactive.corner_radius, egui::CornerRadius::same(8), "its rounded-lg");
    }

    #[test]
    fn rows_of_their_own_with_the_modern_look_only() {
        *CHOSEN.lock().unwrap() = Some(Preset::Compact);
        assert!(rows(true).is_none());
        *CHOSEN.lock().unwrap() = Some(Preset::Modern);
        let rows = rows(true).expect("the modern look's rows");
        assert_eq!(rows.folder, rgb(0xfbbf24));
        assert_eq!(rows.chosen.a(), 0x26, "thin: what is under it shows");
        *CHOSEN.lock().unwrap() = None;
    }

    #[test]
    fn a_dark_accent_is_lightened_and_labelled_readably() {
        let navy = egui::Color32::from_rgb(0x00, 0x2b, 0x5c);
        let lifted = lighter(navy);
        assert!(lifted.r() > navy.r() && lifted.b() > navy.b());
        assert_eq!(readable_on(navy), egui::Color32::WHITE);
        assert_eq!(readable_on(egui::Color32::from_rgb(0x9c, 0xd6, 0xff)), egui::Color32::BLACK);
        assert_eq!(lighter(egui::Color32::WHITE), egui::Color32::WHITE);
    }
}
