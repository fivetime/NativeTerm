//! A dialog inside a window: the skin's title bar over what the dialog
//! has, its own frame and colours.
//!
//! Modal (the usual): in the middle of the window, over a backdrop that
//! takes the clicks meant for what is behind it (egui's `Modal`). Escape
//! and the close button close it; a click on the backdrop does not (what
//! was typed would be lost by a click beside it).
//!
//! Modeless ([`Modal::modeless`]): no backdrop, what is behind stays in
//! use, the dialog is moved by dragging it; its close button closes it.
//!
//! As wide as what it has (at least `min_width`), or a width it is
//! given; with [`Modal::resizable`] the person makes it larger or smaller
//! by its corner.
//!
//! In a window of its own ([`as_window`]: the program made the dialog a
//! window, owned by the one it belongs to, modal or not) the same dialog
//! is the whole window: the title bar moves the window, the window is as
//! large as the dialog (or, resizable, the dialog as large as the window),
//! the close button, Escape and the window system's close all give it up.

use crate::title::TitleBar;
use crate::Skin;

/// How round a dialog's corners are (`rounded-xl`, the design's cards).
const ROUND: u8 = 10;

fn window_id() -> egui::Id {
    egui::Id::new("skin-dialog-window")
}

fn close_id() -> egui::Id {
    egui::Id::new("skin-dialog-close-asked")
}

/// The dialogs shown with `ctx` are each a window of its own (the
/// program's dialog windows say so for theirs).
pub fn as_window(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(window_id(), true));
}

fn room_id() -> egui::Id {
    egui::Id::new("skin-dialog-room")
}

/// A dialog in a window of its own is sized by what it has: what it may
/// take is its owner's room, given here (the owner's size).
pub fn set_room(ctx: &egui::Context, room: egui::Rect) {
    ctx.data_mut(|d| d.insert_temp(room_id(), room));
}

/// The room a dialog shown with `ctx` may take: its owner's (see
/// `set_room`), else the window's.
#[must_use]
pub fn room(ctx: &egui::Context) -> egui::Rect {
    ctx.data(|d| d.get_temp::<egui::Rect>(room_id())).unwrap_or_else(|| ctx.content_rect())
}

/// The window system asked to close a dialog's window (Alt+F4, the
/// taskbar): the dialog shown in it next is given up, as by its close
/// button.
pub fn close_asked(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(close_id(), true));
    ctx.request_repaint();
}

/// What a dialog in a window is.
pub struct Modal<'a> {
    id: egui::Id,
    bar: TitleBar<'a>,
    width: Option<f32>,
    min_width: f32,
    resize: Option<egui::Vec2>,
    padding: i8,
    modeless: bool,
}

/// What happened in a dialog this frame.
pub struct ModalShown<R> {
    pub inner: R,
    /// Its close button was clicked, or (modal) Escape pressed while it is
    /// the one in front and no popup of its own is open: it is given up.
    pub closed: bool,
}

impl<'a> Modal<'a> {
    /// A modal dialog `title` with the close button, as wide as what it
    /// has; `id_salt` tells it from other dialogs.
    #[must_use]
    pub fn new(id_salt: impl std::hash::Hash + std::fmt::Debug, title: &'a str) -> Modal<'a> {
        Modal {
            id: egui::Id::new(("skin-modal", id_salt)),
            bar: TitleBar::new(title),
            width: None,
            min_width: 320.0,
            resize: None,
            padding: 16,
            modeless: false,
        }
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

    /// This wide, title bar and all.
    #[must_use]
    pub fn width(mut self, width: f32) -> Modal<'a> {
        self.width = Some(width);
        self
    }

    /// At least this wide, title bar and all.
    #[must_use]
    pub fn min_width(mut self, width: f32) -> Modal<'a> {
        self.min_width = width;
        self
    }

    /// Made larger or smaller by its corner, `size` (what it has) at first.
    #[must_use]
    pub fn resizable(mut self, size: egui::Vec2) -> Modal<'a> {
        self.resize = Some(size);
        self
    }

    /// The space around what the dialog has.
    #[must_use]
    pub fn padding(mut self, padding: i8) -> Modal<'a> {
        self.padding = padding;
        self
    }

    /// Without a backdrop: what is behind stays in use.
    #[must_use]
    pub fn modeless(mut self) -> Modal<'a> {
        self.modeless = true;
        self
    }

    pub fn show<R>(self, ctx: &egui::Context, skin: &Skin, content: impl FnOnce(&mut egui::Ui) -> R) -> ModalShown<R> {
        let Modal { id, bar, width, min_width, resize, padding, modeless } = self;
        let p = &skin.palette;
        let frame = egui::Frame::NONE
            .fill(p.page)
            .stroke(egui::Stroke::new(1.0_f32, p.line))
            .corner_radius(ROUND)
            .shadow(ctx.global_style().visuals.popup_shadow);
        let pad = 2.0 * f32::from(padding);
        // the first dialog shown in a frame is the window; one it shows
        // over itself (a session's log over the sessions) is in it
        let frame_nr = ctx.cumulative_frame_nr();
        let taken = ctx.data(|d| d.get_temp::<u64>(window_id().with("taken"))) == Some(frame_nr);
        let own_window = !taken && ctx.data(|d| d.get_temp::<bool>(window_id())).unwrap_or(false);
        if own_window {
            ctx.data_mut(|d| d.insert_temp(window_id().with("taken"), frame_nr));
        }
        // a window of its own, resizable: the dialog fills it
        let fill = (own_window && resize.is_some()).then(|| ctx.content_rect().size());
        let round = if own_window { 0 } else { ROUND };
        let title_height = bar.bar_height();
        let inside = |ui: &mut egui::Ui| {
            // the title bar's place first; it is drawn once it is known
            // how wide what is under it is
            let top = ui.cursor().min;
            ui.add_space(bar.bar_height());
            let inner = egui::Frame::NONE
                .inner_margin(padding)
                .show(ui, |ui| {
                    match width {
                        Some(width) => ui.set_width(width - pad),
                        None => ui.set_min_width(min_width - pad),
                    }
                    if let Some(fill) = fill {
                        ui.set_min_size(fill - egui::vec2(pad, pad + bar.bar_height()));
                        return content(ui);
                    }
                    match resize {
                        Some(size) => egui::Resize::default()
                            .id_salt(id.with("size"))
                            .default_size(size)
                            .min_size(egui::vec2(min_width - pad, 120.0))
                            .show(ui, content),
                        None => content(ui),
                    }
                })
                .inner;
            let wide = ui.min_rect().width();
            let rect = egui::Rect::from_min_size(top, egui::vec2(wide, bar.bar_height()));
            let title = bar.show_at(ui, skin, rect, round, own_window);
            (title, inner)
        };
        if own_window {
            let page = egui::Frame::NONE.fill(p.page);
            let shown = egui::Area::new(id)
                .order(egui::Order::Background)
                .fixed_pos(egui::Pos2::ZERO)
                .constrain(false)
                .show(ctx, |ui| page.show(ui, inside).inner);
            let (title, inner) = shown.inner;
            // the window as large as the dialog; one the person makes
            // larger or smaller: as large as it is at first
            let size = shown.response.rect.size();
            let sized = ctx.data(|d| d.get_temp::<bool>(id.with("sized"))).unwrap_or(false);
            let now = ctx.content_rect().size();
            if (resize.is_none() || !sized) && (size - now).length() > 0.5 {
                let want = match resize {
                    Some(first) => first + egui::vec2(pad, pad + title_height),
                    None => size,
                };
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(want));
            }
            ctx.data_mut(|d| d.insert_temp(id.with("sized"), true));
            crate::edges(ctx, skin, resize.is_some());
            let asked = ctx.data_mut(|d| d.remove_temp::<bool>(close_id())).unwrap_or(false);
            let escape = !egui::Popup::is_any_open(ctx)
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            return ModalShown { inner, closed: title.closed() || escape || asked };
        }
        if modeless {
            let shown = egui::Area::new(id)
                .order(egui::Order::Middle)
                .movable(true)
                .default_pos(ctx.content_rect().center() - egui::vec2(min_width / 2.0, 160.0))
                .show(ctx, |ui| frame.show(ui, inside).inner);
            let (title, inner) = shown.inner;
            return ModalShown { inner, closed: title.closed() };
        }
        let shown = egui::Modal::new(id).backdrop_color(p.backdrop).frame(frame).show(ctx, inside);
        let (title, inner) = shown.inner;
        let escape = shown.is_top_modal
            && !shown.any_popup_open
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        ModalShown { inner, closed: title.closed() || escape }
    }
}
