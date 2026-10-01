//! A modal dialog inside a window: the skin's title bar over what the
//! dialog has, in the middle of the window, over a backdrop that takes
//! the clicks meant for what is behind it (egui's `Modal`). Escape and
//! the close button close it; a click on the backdrop does not (what was
//! typed would be lost by a click beside it).

use crate::title::TitleBar;
use crate::Skin;

/// How round a dialog's corners are (`rounded-xl`, the design's cards).
const ROUND: u8 = 10;

/// What a modal dialog is.
pub struct Modal<'a> {
    id: egui::Id,
    bar: TitleBar<'a>,
    width: f32,
    padding: i8,
}

/// What happened in a modal dialog this frame.
pub struct ModalShown<R> {
    pub inner: R,
    /// Its close button was clicked, or Escape pressed while it is the
    /// one in front and no popup of its own is open: it is given up.
    pub closed: bool,
}

impl<'a> Modal<'a> {
    /// A dialog `title` with the close button, 420 wide; `id_salt` tells
    /// it from other dialogs.
    #[must_use]
    pub fn new(id_salt: impl std::hash::Hash, title: &'a str) -> Modal<'a> {
        Modal { id: egui::Id::new(("skin-modal", id_salt)), bar: TitleBar::new(title), width: 420.0, padding: 16 }
    }

    #[must_use]
    pub fn icon(mut self, icon: char) -> Modal<'a> {
        self.bar = self.bar.icon(icon);
        self
    }

    /// Another title bar height than the main window's.
    #[must_use]
    pub fn title_height(mut self, height: f32) -> Modal<'a> {
        self.bar = self.bar.height(height);
        self
    }

    /// How wide it is, title bar and all.
    #[must_use]
    pub fn width(mut self, width: f32) -> Modal<'a> {
        self.width = width;
        self
    }

    /// The space around what the dialog has.
    #[must_use]
    pub fn padding(mut self, padding: i8) -> Modal<'a> {
        self.padding = padding;
        self
    }

    pub fn show<R>(self, ctx: &egui::Context, skin: &Skin, content: impl FnOnce(&mut egui::Ui) -> R) -> ModalShown<R> {
        let Modal { id, bar, width, padding } = self;
        let p = &skin.palette;
        let frame = egui::Frame::NONE
            .fill(p.page)
            .stroke(egui::Stroke::new(1.0_f32, p.line))
            .corner_radius(ROUND)
            .shadow(ctx.global_style().visuals.popup_shadow);
        let shown = egui::Modal::new(id).backdrop_color(p.backdrop).frame(frame).show(ctx, |ui| {
            ui.set_width(width);
            // (the bar's top corners are the frame's)
            let title = bar.show_inside(ui, skin, width, ROUND);
            let inner = egui::Frame::NONE.inner_margin(padding).show(ui, |ui| {
                ui.set_width(width - 2.0 * f32::from(padding));
                content(ui)
            });
            (title, inner.inner)
        });
        let (title, inner) = shown.inner;
        let escape = shown.is_top_modal
            && !shown.any_popup_open
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        ModalShown { inner, closed: title.closed() || escape }
    }
}
