//! The window buttons: minimize, maximize (restore while maximized) and
//! close, as Windows and the Linux desktops have them, or as macOS's
//! three dots.

use crate::{thin, Skin};

/// One of the window's buttons (`p-1.5` around an icon of 14), and what
/// is between two of them (`space-x-1`).
const CAPTION: f32 = 26.0;
const CAPTION_GAP: f32 = 4.0;
/// The window's buttons as macOS has them (`w-3 h-3 rounded-full`,
/// `gap-2`).
const DOT: f32 = 12.0;
const DOT_GAP: f32 = 8.0;

/// One of the window's buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Caption {
    Minimize,
    Maximize,
    /// What "maximize" is while the window is maximized.
    Restore,
    Close,
}

/// Which buttons a window has besides close, which every window has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Buttons {
    pub minimize: bool,
    pub maximize: bool,
}

impl Buttons {
    /// All three: a main window's.
    pub const ALL: Buttons = Buttons { minimize: true, maximize: true };
    /// Close only: a dialog's.
    pub const CLOSE: Buttons = Buttons { minimize: false, maximize: false };
}

impl Caption {
    fn hint(self, skin: &Skin) -> &str {
        let hints = &skin.hints;
        match self {
            Caption::Minimize => &hints.minimize,
            Caption::Maximize => &hints.maximize,
            Caption::Restore => &hints.restore,
            Caption::Close => &hints.close,
        }
    }

    /// Its sign, drawn in a square `side` wide around `middle`: the
    /// design's icons (a line, a square, two squares, a cross), which
    /// are lines a twelfth of their size thick.
    fn paint(self, painter: &egui::Painter, middle: egui::Pos2, side: f32, color: egui::Color32) {
        let line = egui::Stroke::new((side / 12.0).max(1.0), color);
        // (the icons are drawn on a square of 24)
        let at = |x: f32, y: f32| middle + egui::vec2(x - 12.0, y - 12.0) * (side / 24.0);
        match self {
            Caption::Minimize => {
                painter.line_segment([at(5.0, 12.0), at(19.0, 12.0)], line);
            }
            Caption::Maximize => {
                let square = egui::Rect::from_min_max(at(3.0, 3.0), at(21.0, 21.0));
                painter.rect_stroke(square, side / 12.0, line, egui::StrokeKind::Middle);
            }
            Caption::Restore => {
                let front = egui::Rect::from_min_max(at(8.0, 8.0), at(22.0, 22.0));
                painter.rect_stroke(front, side / 12.0, line, egui::StrokeKind::Middle);
                let behind = [at(4.0, 16.0), at(2.0, 14.0), at(2.0, 4.0), at(4.0, 2.0), at(14.0, 2.0), at(16.0, 4.0)];
                painter.add(egui::Shape::line(behind.to_vec(), line));
            }
            Caption::Close => {
                painter.line_segment([at(6.0, 6.0), at(18.0, 18.0)], line);
                painter.line_segment([at(18.0, 6.0), at(6.0, 18.0)], line);
            }
        }
    }
}

/// One of the window's buttons as Windows and the Linux desktops have
/// them (`p-1.5 rounded-lg hover:bg-gray-500/10`; the one that closes
/// `hover:bg-red-500 hover:text-white`).
fn caption_button(ui: &mut egui::Ui, skin: &Skin, caption: Caption) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(CAPTION, CAPTION), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, caption.hint(skin)));
    if ui.is_rect_visible(rect) {
        let p = &skin.palette;
        let (fill, color) = match (response.hovered(), caption) {
            (false, _) => (egui::Color32::TRANSPARENT, p.weak),
            (true, Caption::Close) => (p.danger, egui::Color32::WHITE),
            (true, _) => (thin(egui::Color32::from_rgb(0x6b, 0x72, 0x80), 0x1a), p.weak),
        };
        ui.painter().rect_filled(rect, 8.0, fill);
        // (the square is `w-3` where the others are `w-3.5`)
        let side = if matches!(caption, Caption::Maximize | Caption::Restore) { 12.0 } else { 14.0 };
        caption.paint(ui.painter(), rect.center(), side, color);
    }
    response.on_hover_text(caption.hint(skin))
}

/// The window's buttons at the title bar's end, from the right (the
/// layout they are put into goes that way), `buttons` of them; with
/// `line`, a line before them (`pl-2 border-l`), where other buttons
/// share the title bar. The one that was clicked.
pub fn caption_buttons(
    ui: &mut egui::Ui,
    skin: &Skin,
    buttons: Buttons,
    maximized: bool,
    line: bool,
) -> Option<Caption> {
    let mut clicked = None;
    let middle = if maximized { Caption::Restore } else { Caption::Maximize };
    let shown = [(Caption::Close, true), (middle, buttons.maximize), (Caption::Minimize, buttons.minimize)];
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = CAPTION_GAP;
        for (caption, _) in shown.into_iter().filter(|(_, on)| *on) {
            if caption_button(ui, skin, caption).clicked() {
                clicked = Some(caption);
            }
        }
        if line {
            ui.add_space(8.0 - CAPTION_GAP);
            let (rule, _) = ui.allocate_exact_size(egui::vec2(1.0, CAPTION), egui::Sense::hover());
            ui.painter().rect_filled(rule, 0.0, skin.palette.line);
        }
    });
    clicked
}

/// The window's buttons as macOS has them, at the title bar's start:
/// three dots (`bg-red-500`, `bg-amber-500`, `bg-emerald-500`; a shade
/// darker under the pointer), their signs in them while the pointer is
/// on any of them; without their colours while the window is not the one
/// in front, as the system's are. A button the window doesn't have is a
/// dot without colour that does nothing, as macOS shows it. The one that
/// was clicked.
pub fn caption_dots(ui: &mut egui::Ui, skin: &Skin, buttons: Buttons, focused: bool) -> Option<Caption> {
    let dots = [
        (Caption::Close, true, 0xef4444, 0xdc2626, 0x450a0a),
        (Caption::Minimize, buttons.minimize, 0xf59e0b, 0xd97706, 0x451a03),
        (Caption::Maximize, buttons.maximize, 0x10b981, 0x059669, 0x022c22),
    ];
    let rgb = |hex: u32| egui::Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
    let width = 3.0 * DOT + 2.0 * DOT_GAP;
    let (all, _) = ui.allocate_exact_size(egui::vec2(width, DOT), egui::Sense::hover());
    let over_any = ui.rect_contains_pointer(all.expand(2.0));
    let mut clicked = None;
    for (i, (caption, on, color, near, sign)) in dots.into_iter().enumerate() {
        let left = all.left() + i as f32 * (DOT + DOT_GAP);
        let rect = egui::Rect::from_min_size(egui::pos2(left, all.top()), egui::vec2(DOT, DOT));
        if !on {
            ui.painter().circle_filled(rect.center(), DOT / 2.0, skin.palette.line);
            continue;
        }
        let response = ui.interact(rect.expand(2.0), ui.id().with(("dot", i)), egui::Sense::click());
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, caption.hint(skin)));
        let fill = match (focused || over_any, response.hovered()) {
            (false, _) => skin.palette.line,
            (true, false) => rgb(color),
            (true, true) => rgb(near),
        };
        ui.painter().circle_filled(rect.center(), DOT / 2.0, fill);
        if over_any {
            // (`w-2` with lines of three: thicker than the others')
            caption.paint(ui.painter(), rect.center(), 7.0, rgb(sign));
        }
        if response.on_hover_text(caption.hint(skin)).clicked() {
            clicked = Some(caption);
        }
    }
    clicked
}
