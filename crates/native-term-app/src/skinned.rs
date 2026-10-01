//! What NativeTerm's windows of their own have in common on the skin
//! (`native-term-skin`): the theme the main window chose, the skin's
//! title bar and edges, the content on the skin's page, and the row of
//! buttons at the bottom.

use native_term_skin::{Buttons, Choice, Skin, TitleBar};

/// The space around a window's content and its buttons.
pub const PADDING: i8 = 14;

/// The viewport of a window of its own without the system's title bar,
/// `content` high below the skin's (which is in the window).
pub fn viewport(title: &str, width: f32, content: f32) -> egui::ViewportBuilder {
    native_term_skin::undecorated(
        egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size([width, content + native_term_skin::TITLE_BAR]),
    )
}

/// The top of a dialog's frame: the theme the main window chose (light
/// or dark), the title bar with `title`, `icon` and the close button, the
/// edges. The skin, for the rest.
pub fn chrome(ui: &mut egui::Ui, title: &str, icon: char) -> Skin {
    frame(ui, title, icon, Buttons::CLOSE, false)
}

/// The same for a window that is made larger and smaller: all three
/// buttons, edges to take it by.
pub fn chrome_resizable(ui: &mut egui::Ui, title: &str, icon: char) -> Skin {
    frame(ui, title, icon, Buttons::ALL, true)
}

fn frame(ui: &mut egui::Ui, title: &str, icon: char, buttons: Buttons, resizable: bool) -> Skin {
    if let Some(theme) = *crate::app::THEME.lock().unwrap_or_else(|e| e.into_inner()) {
        if ui.ctx().options(|o| o.theme_preference) != theme {
            ui.ctx().set_theme(theme);
        }
    }
    let skin = crate::looks::skin(ui.visuals());
    TitleBar::new(title).icon(icon).buttons(buttons).show_window(ui, &skin);
    native_term_skin::edges(ui.ctx(), &skin, resizable);
    skin
}

/// The skin's page, with the space around the content.
pub fn page(skin: &Skin) -> egui::Frame {
    egui::Frame::NONE.fill(skin.palette.page).inner_margin(egui::Margin { bottom: 0, ..egui::Margin::same(PADDING) })
}

/// The row of buttons at the window's bottom (put before the content:
/// it keeps its room whatever the content takes), `left` at its left.
/// Which was pressed.
pub fn buttons(
    ui: &mut egui::Ui,
    skin: &Skin,
    id: &'static str,
    left: impl FnOnce(&mut egui::Ui),
    choices: &[Choice],
) -> Option<usize> {
    let frame = egui::Frame::NONE.fill(skin.palette.page).inner_margin(PADDING);
    egui::Panel::bottom(id)
        .frame(frame)
        .show_separator_line(false)
        .show_inside(ui, |ui| native_term_skin::footer(ui, skin, left, choices))
        .inner
}
