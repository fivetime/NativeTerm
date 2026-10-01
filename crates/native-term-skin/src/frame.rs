//! A window's edges where the system gives it none (Windows and the
//! Linux desktops: the title bar went, and the frame with it): a line
//! around it, and along it the bands it is taken by to be made larger or
//! smaller.

use crate::{Skin, MAC};

/// Where the window is taken by its edge to be made larger or smaller:
/// the band along each edge and the corners' reach along it, as Chrome
/// has them for a frame of its own without a shadow
/// (`kFrameBorderThickness`, `kResizeAreaCornerSize` in
/// `ui/views/window/default_frame_view.cc`).
pub const FRAME_BAND: f32 = 4.0;
pub const FRAME_CORNER: f32 = 16.0;

/// Which way the window is made larger or smaller when it is taken at
/// `point` (from its top left corner; it is `size` large), if it is
/// taken by its edge there: Chrome's `FrameView::GetHTComponentForFrame`
/// with `FRAME_BAND` along every edge and `FRAME_CORNER` for the corners.
#[must_use]
pub fn frame_hit(point: egui::Vec2, size: egui::Vec2) -> Option<egui::viewport::ResizeDirection> {
    use egui::viewport::ResizeDirection as To;
    let mut top = point.y < FRAME_BAND;
    let bottom = point.y >= size.y - FRAME_BAND;
    let mut left = point.x < FRAME_BAND;
    let mut right = point.x >= size.x - FRAME_BAND;
    if !(top || bottom || left || right) {
        return None;
    }
    // (in a band: the corners reach further along it)
    top |= point.y < FRAME_CORNER;
    left |= point.x < FRAME_CORNER;
    right |= point.x >= size.x - FRAME_CORNER;
    Some(match (top, bottom, left, right) {
        (true, _, true, _) => To::NorthWest,
        (true, _, _, true) => To::NorthEast,
        (true, _, _, _) => To::North,
        (_, true, true, _) => To::SouthWest,
        (_, true, _, true) => To::SouthEast,
        (_, true, _, _) => To::South,
        (_, _, true, _) => To::West,
        _ => To::East,
    })
}

/// The pointer's shape over an edge the window is taken by.
#[must_use]
pub fn frame_cursor(to: egui::viewport::ResizeDirection) -> egui::CursorIcon {
    use egui::viewport::ResizeDirection as To;
    match to {
        To::North | To::South => egui::CursorIcon::ResizeVertical,
        To::East | To::West => egui::CursorIcon::ResizeHorizontal,
        To::NorthWest | To::SouthEast => egui::CursorIcon::ResizeNwSe,
        To::NorthEast | To::SouthWest => egui::CursorIcon::ResizeNeSw,
    }
}

/// The window's edges: a line around it, and with `resizable` the bands
/// it is made larger or smaller by. Over everything else, so that what
/// is under a band is not pressed with it. Nothing on macOS (the system's
/// frame stays) or while the window fills the screen.
pub fn edges(ctx: &egui::Context, skin: &Skin, resizable: bool) {
    if MAC {
        return;
    }
    let filling = ctx.input(|i| {
        let window = i.viewport();
        window.maximized.unwrap_or(false) || window.fullscreen.unwrap_or(false)
    });
    if filling {
        return;
    }
    let whole = ctx.content_rect();
    let over = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("window-frame"));
    let line = egui::Stroke::new(1.0_f32, skin.palette.line);
    ctx.layer_painter(over).rect_stroke(whole, 0.0, line, egui::StrokeKind::Inside);
    if !resizable {
        return;
    }
    let band = FRAME_BAND;
    let bands = [
        ("n", egui::Rect::from_min_max(whole.min, egui::pos2(whole.right(), whole.top() + band))),
        ("s", egui::Rect::from_min_max(egui::pos2(whole.left(), whole.bottom() - band), whole.max)),
        ("w", egui::Rect::from_min_max(whole.min, egui::pos2(whole.left() + band, whole.bottom()))),
        ("e", egui::Rect::from_min_max(egui::pos2(whole.right() - band, whole.top()), whole.max)),
    ];
    for (name, rect) in bands {
        egui::Area::new(egui::Id::new(("window-frame", name)))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.min)
            .constrain(false)
            .show(ctx, |ui| {
                let (_, edge) = ui.allocate_exact_size(rect.size(), egui::Sense::drag());
                let to = edge.hover_pos().and_then(|at| frame_hit(at - whole.min, whole.size()));
                let Some(to) = to else { return };
                ui.ctx().set_cursor_icon(frame_cursor(to));
                if edge.drag_started_by(egui::PointerButton::Primary) {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::BeginResize(to));
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_is_taken_by_its_edges() {
        use egui::viewport::ResizeDirection as To;
        let size = egui::vec2(800.0, 600.0);
        let at = |x: f32, y: f32| frame_hit(egui::vec2(x, y), size);
        assert_eq!(at(400.0, 300.0), None, "in the window");
        assert_eq!(at(400.0, 4.0), None, "the header, under the band");
        assert_eq!(at(400.0, 3.0), Some(To::North));
        assert_eq!(at(400.0, 596.0), Some(To::South));
        assert_eq!(at(3.0, 300.0), Some(To::West));
        assert_eq!(at(796.0, 300.0), Some(To::East));
        // the corners reach 16 along a band
        assert_eq!(at(15.0, 3.0), Some(To::NorthWest));
        assert_eq!(at(3.0, 15.0), Some(To::NorthWest));
        assert_eq!(at(16.0, 3.0), Some(To::North));
        assert_eq!(at(790.0, 2.0), Some(To::NorthEast));
        assert_eq!(at(10.0, 598.0), Some(To::SouthWest));
        assert_eq!(at(799.0, 599.0), Some(To::SouthEast));
        // (as in Chrome: from the side a lower corner begins at the band below)
        assert_eq!(at(2.0, 590.0), Some(To::West));
        assert_eq!(frame_cursor(To::NorthWest), egui::CursorIcon::ResizeNwSe);
    }
}
