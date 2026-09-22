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
}

/// The look every window follows (the floating button has its own egui
/// context and reads it too).
static CHOSEN: Mutex<Option<Preset>> = Mutex::new(None);

impl Preset {
    pub const ALL: [Preset; 4] = [Preset::Plain, Preset::Accent, Preset::Dim, Preset::Compact];

    /// What is written in the settings (the plain look writes nothing).
    #[must_use]
    pub fn setting(self) -> &'static str {
        match self {
            Preset::Plain => "",
            Preset::Accent => "accent",
            Preset::Dim => "dim",
            Preset::Compact => "compact",
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
    fn a_dark_accent_is_lightened_and_labelled_readably() {
        let navy = egui::Color32::from_rgb(0x00, 0x2b, 0x5c);
        let lifted = lighter(navy);
        assert!(lifted.r() > navy.r() && lifted.b() > navy.b());
        assert_eq!(readable_on(navy), egui::Color32::WHITE);
        assert_eq!(readable_on(egui::Color32::from_rgb(0x9c, 0xd6, 0xff)), egui::Color32::BLACK);
        assert_eq!(lighter(egui::Color32::WHITE), egui::Color32::WHITE);
    }
}
