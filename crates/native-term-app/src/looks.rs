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
    /// A dark editor's colours (slate and blue), round corners, more
    /// room: the design the person brought for the main window
    /// (`docs/ARCHITECTURE.md`, "The main window"), and what a window
    /// has when nothing else was chosen. Its light side is the same
    /// scales' light end.
    #[default]
    Modern,
    /// egui's own colours and spacing.
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
    pub const ALL: [Preset; 5] = [Preset::Modern, Preset::Plain, Preset::Accent, Preset::Dim, Preset::Compact];

    /// What is written in the settings (the modern look, which a window
    /// has anyway, writes nothing).
    #[must_use]
    pub fn setting(self) -> &'static str {
        match self {
            Preset::Modern => "",
            Preset::Plain => "plain",
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

/// The modern look's colours: the design's variables (its second
/// version has them for the dark and for the light theme), by what they
/// are for here.
struct Palette {
    /// The window (`--bg-main`), the bars around the page
    /// (`--bg-surface`), what is typed into and what is pressed
    /// (`--bg-card`), and what the pointer is on (`--bg-hover`).
    page: egui::Color32,
    bar: egui::Color32,
    card: egui::Color32,
    raised: egui::Color32,
    /// The lines between things and around them (`--border-color`), and
    /// around what the pointer is on.
    line: egui::Color32,
    near: egui::Color32,
    /// `--text-main`, `--text-muted`.
    text: egui::Color32,
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

/// `percent` of 100 as an alpha of 255, as CSS rounds it.
const fn alpha(percent: u32) -> u8 {
    ((percent * 255 + 50) / 100) as u8
}

const BLUE_500: egui::Color32 = rgb(0x3b82f6);
const BLUE_600: egui::Color32 = rgb(0x2563eb);

impl Palette {
    const fn of(dark: bool) -> Palette {
        if dark {
            Palette {
                page: rgb(0x121316),
                bar: rgb(0x16181d),
                card: rgb(0x1a1c22),
                raised: rgb(0x22252e),
                line: rgb(0x2a2e3b),
                near: rgb(0x475569),
                text: rgb(0xf1f5f9),
                weak: rgb(0x94a3b8),
                accent: BLUE_600,
                link: rgb(0x60a5fa),
            }
        } else {
            Palette {
                page: rgb(0xf8fafc),
                bar: rgb(0xffffff),
                card: rgb(0xf1f5f9),
                raised: rgb(0xe2e8f0),
                line: rgb(0xcbd5e1),
                near: rgb(0x94a3b8),
                text: rgb(0x0f172a),
                weak: rgb(0x64748b),
                accent: BLUE_600,
                link: BLUE_600,
            }
        }
    }

    /// egui's own widgets in these colours (the dialogs, the settings):
    /// a window is a bar, what is in it is a card.
    fn paint(&self, visuals: &mut egui::Visuals) {
        let round = egui::CornerRadius::same(8);
        visuals.panel_fill = self.page;
        visuals.window_fill = self.bar;
        visuals.extreme_bg_color = self.card;
        visuals.faint_bg_color = self.card;
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
            widget.fg_stroke = egui::Stroke::new(1.0_f32, self.text);
            widget.corner_radius = round;
            widget.expansion = 0.0;
        }
    }
}

/// How something is marked: a colour thinly under it, less thinly around
/// it, and its text in a shade that can be read on that (the skin's).
pub use native_term_skin::Tint;

/// `base` at `fill` percent under, at `line` percent around.
fn tint_of(base: egui::Color32, fill: u32, line: u32, text: egui::Color32) -> Tint {
    Tint { fill: thin(base, alpha(fill)), line: thin(base, alpha(line)), text }
}

/// The colours the main window is laid out in (`layout.rs`), whatever
/// the look: the modern look's are the design's own, the other looks'
/// are what they give egui.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tones {
    /// Where the rows are (`--bg-main`), the bars around it
    /// (`--bg-surface`: the header, the search, the bar below, the
    /// properties), the rail (`--rail-bg`) and the line at its side.
    pub page: egui::Color32,
    pub bar: egui::Color32,
    pub rail: egui::Color32,
    pub rail_line: egui::Color32,
    /// What is typed into and what is pressed (`--bg-card`), a filter
    /// that is off (`--chip-bg`), what the pointer is on (`--bg-hover`).
    pub card: egui::Color32,
    pub chip: egui::Color32,
    pub raised: egui::Color32,
    /// The lines (`--border-color`), and around what the pointer is on.
    pub line: egui::Color32,
    pub near: egui::Color32,
    /// `--text-main`, `--text-muted`.
    pub text: egui::Color32,
    pub weak: egui::Color32,
    /// What is shown, in the rail; a sign in the accent's colour (the
    /// design's blue 500, in both themes).
    pub accent: egui::Color32,
    /// The header's tile (`bg-blue-500/10 border-blue-500/30
    /// text-blue-500`), the filter that is on (`--chip-active-*`), the
    /// row that is chosen (`--item-selected-*`).
    pub tile: Tint,
    pub chip_on: Tint,
    pub chosen: Tint,
    /// The button that makes something new (`bg-blue-600`), the same
    /// under the pointer (`hover:bg-blue-500`), and what is on it.
    pub primary: egui::Color32,
    pub primary_near: egui::Color32,
    pub on_primary: egui::Color32,
    /// What takes something away (the design's red 500, thinly).
    pub danger: egui::Color32,
    /// What the pointer is on, in the rail (`hover:text-blue-600`; the
    /// light and the dark, `hover:text-amber-500`).
    pub rail_near: egui::Color32,
    pub sun: egui::Color32,
    /// The folders' and the hosts' pictures (amber and blue 500).
    pub folder: egui::Color32,
    pub host: egui::Color32,
    /// A mark that says nothing about how it goes (`bg-slate-500/10
    /// border-slate-500/20`), and: doing well, not yet, not at all.
    pub plain: Tint,
    pub good: Tint,
    pub busy: Tint,
    pub bad: Tint,
    /// The dot that says something is open (`bg-emerald-500`).
    pub alive: egui::Color32,
    /// The line down each level of the tree (`--guide-line`).
    pub guide: egui::Color32,
}

/// `a` with `t` of `b` in it.
fn mix(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let one = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    egui::Color32::from_rgb(one(a.r(), b.r()), one(a.g(), b.g()), one(a.b(), b.b()))
}

impl Tones {
    /// The modern look's: the design's, in the dark or the light.
    #[must_use]
    pub fn modern(dark: bool) -> Tones {
        let p = Palette::of(dark);
        let shade = |dark_one: u32, light_one: u32| if dark { rgb(dark_one) } else { rgb(light_one) };
        let slate = rgb(0x64748b);
        // (a mark's text: the design has one shade for both themes, 600,
        // which on its own thin colour over white is a sign's contrast
        // and not a text's: 400 in the dark, 700 in the light, amber 800)
        let mark = |base: u32, dark_one: u32, light_one: u32| tint_of(rgb(base), 15, 30, shade(dark_one, light_one));
        Tones {
            page: p.page,
            bar: p.bar,
            rail: shade(0x111216, 0xf1f5f9),
            rail_line: shade(0x22252e, 0xe2e8f0),
            card: p.card,
            chip: shade(0x1a1c22, 0xe2e8f0),
            raised: p.raised,
            line: p.line,
            near: p.near,
            text: p.text,
            weak: p.weak,
            accent: BLUE_500,
            tile: tint_of(BLUE_500, 10, 30, BLUE_500),
            chip_on: if dark {
                tint_of(BLUE_500, 20, 40, rgb(0x60a5fa))
            } else {
                tint_of(BLUE_600, 12, 35, rgb(0x1d4ed8))
            },
            chosen: if dark {
                tint_of(BLUE_500, 18, 40, rgb(0xffffff))
            } else {
                tint_of(BLUE_600, 12, 35, rgb(0x1e3a8a))
            },
            primary: BLUE_600,
            primary_near: BLUE_500,
            on_primary: egui::Color32::WHITE,
            danger: rgb(0xef4444),
            rail_near: BLUE_600,
            sun: rgb(0xf59e0b),
            folder: rgb(0xf59e0b),
            host: BLUE_500,
            plain: tint_of(slate, 10, 20, p.weak),
            good: mark(0x10b981, 0x34d399, 0x047857),
            busy: mark(0xf59e0b, 0xfbbf24, 0x92400e),
            bad: mark(0xef4444, 0xf87171, 0xb91c1c),
            alive: rgb(0x10b981),
            guide: if dark { thin(egui::Color32::WHITE, alpha(8)) } else { thin(rgb(0x0f172a), alpha(12)) },
        }
    }

    /// Another look's: what it gave egui, and the marks' colours as the
    /// modern look has them.
    fn of(visuals: &egui::Visuals) -> Tones {
        let modern = Tones::modern(visuals.dark_mode);
        let page = visuals.panel_fill;
        let text = visuals.text_color();
        let chosen = visuals.selection.bg_fill.to_opaque();
        let link = visuals.hyperlink_color;
        let line = visuals.widgets.noninteractive.bg_stroke.color;
        Tones {
            page,
            bar: mix(page, text, 0.04),
            rail: mix(page, text, 0.07),
            rail_line: line,
            card: visuals.widgets.inactive.weak_bg_fill,
            chip: visuals.widgets.inactive.weak_bg_fill,
            raised: visuals.widgets.hovered.weak_bg_fill,
            line,
            near: visuals.widgets.hovered.bg_stroke.color,
            text,
            weak: visuals.weak_text_color(),
            accent: link,
            tile: tint_of(chosen, 10, 30, link),
            chip_on: tint_of(chosen, 20, 40, link),
            chosen: tint_of(chosen, 18, 40, visuals.strong_text_color()),
            primary: chosen,
            primary_near: mix(chosen, text, 0.15),
            on_primary: readable_on(chosen),
            rail_near: link,
            ..modern
        }
    }
}

/// The colours of the lists' rows (the skin's `ItemRow`).
#[must_use]
pub fn item_colors(t: &Tones) -> native_term_skin::ItemColors {
    native_term_skin::ItemColors {
        card: t.card,
        line: t.line,
        near: t.near,
        primary: t.primary,
        primary_near: t.primary_near,
        on_primary: t.on_primary,
        chosen: t.chosen,
        raised: t.raised,
        accent: t.accent,
        guide: t.guide,
        text: t.text,
        weak: t.weak,
        plain: t.plain,
    }
}

/// The skin every window is drawn with (`native-term-skin`): the
/// look's colours, the window buttons' names in the person's language.
#[must_use]
pub fn skin(visuals: &egui::Visuals) -> native_term_skin::Skin {
    let t = tones(visuals);
    native_term_skin::Skin {
        palette: native_term_skin::Palette {
            page: t.page,
            bar: t.bar,
            line: t.line,
            card: t.card,
            raised: t.raised,
            text: t.text,
            weak: t.weak,
            danger: t.danger,
            primary: t.primary,
            on_primary: t.on_primary,
            tile: t.tile,
            rail: t.rail,
            rail_line: t.rail_line,
            accent: t.accent,
            rail_near: t.rail_near,
            backdrop: egui::Color32::from_black_alpha(if visuals.dark_mode { 140 } else { 90 }),
        },
        hints: native_term_skin::Hints {
            minimize: native_term_app::t!("window-minimize"),
            maximize: native_term_app::t!("window-maximize"),
            restore: native_term_app::t!("window-restore"),
            close: native_term_app::t!("window-close"),
        },
        order: button_order(),
    }
}

/// Where a dialog's main button goes: first on Windows and KDE (OK
/// Cancel), last on macOS and the GTK desktops (Cancel OK).
fn button_order() -> native_term_skin::Order {
    use native_term_skin::Order;
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_ascii_lowercase();
        if desktop.split(':').any(|d| matches!(d, "kde" | "lxqt")) {
            return Order::PrimaryFirst;
        }
    }
    Order::platform()
}

/// The colours of the look chosen, as `visuals` has it (dark or light).
#[must_use]
pub fn tones(visuals: &egui::Visuals) -> Tones {
    match chosen() {
        Some(Preset::Modern) | None => Tones::modern(visuals.dark_mode),
        Some(_) => Tones::of(visuals),
    }
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
        assert_eq!(Preset::from_setting(None), Preset::Modern);
        assert_eq!(Preset::from_setting(Some("")), Preset::Modern);
        assert_eq!(Preset::from_setting(Some("something else")), Preset::Modern, "an unknown look is the modern one");
        assert_eq!(Preset::from_setting(Some("modern")), Preset::Modern, "as it was written while it was chosen");
        assert_eq!(Preset::Modern.setting(), "", "and the modern one writes nothing");
    }

    /// The text against what it is on, as WCAG counts contrast: 4.5
    /// for text, 3 for what is only a sign.
    fn contrast(a: egui::Color32, b: egui::Color32) -> f32 {
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
        let (a, b) = (light(a), light(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// `over` (thin) on `under`, as it is seen.
    fn seen(over: egui::Color32, under: egui::Color32) -> egui::Color32 {
        let [r, g, b, a] = over.to_srgba_unmultiplied();
        mix(under, egui::Color32::from_rgb(r, g, b), f32::from(a) / 255.0)
    }

    #[test]
    fn the_modern_look_can_be_read() {
        for dark in [true, false] {
            let tones = Tones::modern(dark);
            for ground in [tones.page, tones.bar, tones.rail, tones.card, tones.chip, tones.raised] {
                assert!(contrast(tones.text, ground) >= 4.5, "text, dark {dark}: {}", contrast(tones.text, ground));
            }
            // the muted text: a text's contrast on the page and the
            // bars; on a card and on a filter it is the design's, which
            // in the light is less (4.3 and 3.8)
            for ground in [tones.page, tones.bar] {
                assert!(contrast(tones.weak, ground) >= 4.5, "muted, dark {dark}: {}", contrast(tones.weak, ground));
            }
            for ground in [tones.card, tones.chip, tones.rail] {
                assert!(contrast(tones.weak, ground) >= 3.5, "muted, dark {dark}: {}", contrast(tones.weak, ground));
            }
            // the marks, on their own thin colour over the page and the bars
            for tint in [tones.good, tones.busy, tones.bad, tones.chip_on, tones.chosen] {
                for ground in [tones.page, tones.bar] {
                    let under = seen(tint.fill, ground);
                    assert!(contrast(tint.text, under) >= 4.5, "a mark, dark {dark}: {}", contrast(tint.text, under));
                }
            }
            assert!(contrast(tones.on_primary, tones.primary) >= 4.5, "the button that makes something new");
            // what is only a sign
            for sign in [tones.accent, tones.folder, tones.host, tones.danger] {
                assert!(contrast(sign, tones.page) >= 2.0, "a sign, dark {dark}: {}", contrast(sign, tones.page));
            }
        }
        let mut visuals = egui::Visuals::dark();
        Palette::of(true).paint(&mut visuals);
        assert_eq!(visuals.panel_fill, rgb(0x121316), "the design's page");
        assert_eq!(visuals.widgets.inactive.bg_fill, rgb(0x1a1c22), "its buttons");
        assert_eq!(visuals.widgets.inactive.corner_radius, egui::CornerRadius::same(8), "its rounded-lg");
    }

    #[test]
    fn the_layout_has_its_colours_with_every_look() {
        // the design's own, in the dark
        let tones = Tones::modern(true);
        assert_eq!(tones.page, rgb(0x121316));
        assert_eq!(tones.bar, rgb(0x16181d), "its header, its properties");
        assert_eq!(tones.rail, rgb(0x111216));
        assert_eq!(tones.line, rgb(0x2a2e3b));
        assert_eq!(tones.text, rgb(0xf1f5f9));
        assert_eq!(tones.accent, rgb(0x3b82f6), "what is shown, in the rail");
        assert_eq!(tones.primary, rgb(0x2563eb), "its blue button");
        assert_eq!(tones.chosen.fill, thin(rgb(0x3b82f6), 46), "18%: what is under it shows");
        assert_eq!(tones.chosen.text, egui::Color32::WHITE);
        // and in the light
        let tones = Tones::modern(false);
        assert_eq!(tones.page, rgb(0xf8fafc));
        assert_eq!(tones.bar, rgb(0xffffff));
        assert_eq!(tones.rail, rgb(0xf1f5f9));
        assert_eq!(tones.card, rgb(0xf1f5f9));
        assert_eq!(tones.line, rgb(0xcbd5e1));
        assert_eq!(tones.text, rgb(0x0f172a));
        assert_eq!(tones.weak, rgb(0x64748b));
        assert_eq!(tones.chip_on.text, rgb(0x1d4ed8));
        assert_eq!(tones.chosen.fill, thin(rgb(0x2563eb), 31), "12%");
        assert_eq!(tones.chosen.text, rgb(0x1e3a8a));
        // another look's are what egui was given
        let visuals = egui::Visuals::dark();
        let plain = Tones::of(&visuals);
        assert_eq!(plain.page, visuals.panel_fill);
        assert_ne!(plain.bar, plain.page, "the bars can be told from the page");
        assert_eq!(plain.good, Tones::modern(true).good, "the marks are the same");
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
