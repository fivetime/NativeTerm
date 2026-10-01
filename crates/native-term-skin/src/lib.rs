//! One look for every window, as SkinUI's `CSkinDialog` gave a Win32
//! program one: the program's main window, its other windows and its
//! modal dialogs inside a window all get the same title bar, the same
//! window buttons where the platform has them (three dots at the left on
//! macOS; minimize, maximize and close at the right elsewhere), the same
//! edges to take a window by, and the same colours. A window says what it
//! is (its title, its icon, which buttons, modal or not, how high its
//! title bar if not the usual) and draws what is inside; the rest is
//! here, in one place.
//!
//! Only egui: the window's own commands (move, resize, minimize, close)
//! go out as `egui::ViewportCommand`s, which eframe and egui-winit carry
//! out alike. The window itself is made without the system's title bar
//! ([`undecorated`]).
//!
//! The numbers are those of NativeTerm's design (Tailwind classes in the
//! design's page, noted where they are used); the edges are Chromium's
//! for a frame of its own.

mod caption;
mod dialog;
mod frame;
mod modal;
mod title;

pub use caption::{caption_buttons, caption_dots, Buttons, Caption};
pub use dialog::{body, button, footer, Choice, Message, MessageShown, Notice, Order, Role};
pub use frame::{edges, frame_cursor, frame_hit, FRAME_BAND, FRAME_CORNER};
pub use modal::{as_window, close_asked, room, set_room, Modal, ModalShown};
pub use title::{TitleBar, TitleShown};

/// The title bar's height unless a window says otherwise: the main
/// window's header (`h-14`).
pub const TITLE_BAR: f32 = 56.0;
/// The space at the title bar's ends (`px-4`).
pub const TITLE_PAD: i8 = 16;
/// The icon's tile in the title bar (an icon of 16 with `p-1.5`).
pub const TILE: f32 = 28.0;
/// The title's text (`text-sm`).
pub const TITLE_TEXT: f32 = 14.0;

/// Whether the window buttons are macOS's (three dots at the title
/// bar's start) rather than the others' (at its end).
pub const MAC: bool = cfg!(target_os = "macos");

/// A colour with what shows through it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tint {
    pub fill: egui::Color32,
    pub line: egui::Color32,
    pub text: egui::Color32,
}

/// The colours the skin draws with: the program gives its own (its
/// theme's, dark or light).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// Inside a window (`--bg-main`) and its bars, the title bar among
    /// them (`--bg-surface`).
    pub page: egui::Color32,
    pub bar: egui::Color32,
    /// The lines (`--border-color`): around a window, under its title
    /// bar, before its buttons.
    pub line: egui::Color32,
    /// `--text-main`, `--text-muted` (a window button's sign).
    pub text: egui::Color32,
    pub weak: egui::Color32,
    /// The close button under the pointer (red 500); a button that takes
    /// something away.
    pub danger: egui::Color32,
    /// The button that does what a dialog is for (`bg-blue-600`), and
    /// what is on it; the other buttons are the theme's.
    pub primary: egui::Color32,
    pub on_primary: egui::Color32,
    /// The icon's tile in the title bar.
    pub tile: Tint,
    /// Over what is behind a modal dialog.
    pub backdrop: egui::Color32,
}

/// What the window buttons are called (their hints, and what a screen
/// reader says): the program's language.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hints {
    pub minimize: String,
    pub maximize: String,
    pub restore: String,
    pub close: String,
}

impl Default for Hints {
    fn default() -> Hints {
        Hints {
            minimize: "Minimize".into(),
            maximize: "Maximize".into(),
            restore: "Restore".into(),
            close: "Close".into(),
        }
    }
}

/// The skin: its colours, its words, and where the platform puts a
/// dialog's main button.
#[derive(Clone, Debug, PartialEq)]
pub struct Skin {
    pub palette: Palette,
    pub hints: Hints,
    pub order: Order,
}

/// `color` as thin as `alpha` of 255 says.
#[must_use]
pub fn thin(color: egui::Color32, alpha: u8) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// A window to be made without the system's title bar: the skin's is
/// drawn instead. On macOS the window keeps the system's frame (its round
/// corners, its shadow, its edges to take it by), the title bar
/// see-through over the window's content and its buttons hidden for the
/// skin's; elsewhere a frame comes with a title bar or not at all, and
/// the window has none ([`edges`] draws its own).
#[must_use]
pub fn undecorated(viewport: egui::ViewportBuilder) -> egui::ViewportBuilder {
    if MAC {
        viewport
            .with_fullsize_content_view(true)
            .with_title_shown(false)
            .with_titlebar_shown(false)
            .with_titlebar_buttons_shown(false)
    } else {
        viewport.with_decorations(false)
    }
}

/// A whole window of its own (not the main one): the title bar, then
/// `content` in what is left, then the edges over everything. `resizable`
/// as the window was made.
pub fn window<R>(
    ui: &mut egui::Ui,
    skin: &Skin,
    bar: TitleBar<'_>,
    resizable: bool,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> (TitleShown, R) {
    let shown = bar.show_window(ui, skin);
    let page = egui::Frame::NONE.fill(skin.palette.page);
    let inner = egui::CentralPanel::default().frame(page).show_inside(ui, content).inner;
    edges(ui.ctx(), skin, resizable);
    (shown, inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undecorated_drops_the_title_bar() {
        let viewport = undecorated(egui::ViewportBuilder::default());
        if MAC {
            assert_eq!(viewport.titlebar_shown, Some(false));
            assert_eq!(viewport.fullsize_content_view, Some(true));
        } else {
            assert_eq!(viewport.decorations, Some(false));
        }
    }
}
