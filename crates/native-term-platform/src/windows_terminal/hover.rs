//! The card that appears when the mouse rests on one of NativeTerm's tabs,
//! as a browser shows a tab preview: the tab's picture (or, for a tab
//! Terminal has never rendered, the text its console holds), with the
//! session's name and state under it.
//!
//! It rides on what the tab menu already has: the low-level mouse hook
//! that knows where NativeTerm's tabs are, and the same non-activating
//! popup drawn with Direct2D (`menu_draw.rs`). Terminal is not touched:
//! the card is a window of ours, over its tab strip.
//!
//! Nothing runs while the mouse is elsewhere: the hook only tests the
//! point against the tab rectangles it already has, and the timer that
//! shows the card exists only while the mouse rests on a tab.

use std::cell::RefCell;

use windows::Win32::Foundation::{HWND, RECT, SIZE};

use super::menu_draw::{Canvas, Painter};
use super::theme::Look;

/// The card's picture area, in effective pixels.
pub(super) const CARD_W: f32 = 280.0;
pub(super) const CARD_H: f32 = 158.0;
pub(super) const PAD: f32 = 8.0;
/// The name and the note under the picture.
pub(super) const TITLE_H: f32 = 20.0;
pub(super) const NOTE_H: f32 = 16.0;

pub use crate::overlay::HoverCard;
use crate::WindowId;

pub(super) struct Card {
    pub(super) popup: HWND,
    /// The tab it is for (window handle, tab index).
    pub(super) tab: (WindowId, usize),
    pub(super) card: HoverCard,
    pub(super) look: Look,
    pub(super) scale: f32,
    pub(super) size: SIZE,
}

thread_local! {
    pub(super) static CARD: RefCell<Option<Card>> = const { RefCell::new(None) };
}

/// The size a card of this scale takes.
pub(super) fn size_for(scale: f32) -> SIZE {
    let px = |v: f32| (v * scale).round() as i32;
    SIZE { cx: px(CARD_W + PAD * 2.0), cy: px(CARD_H + TITLE_H + NOTE_H + PAD * 3.0) }
}

/// Which tab the card on show is for.
pub(super) fn showing() -> Option<(WindowId, usize)> {
    CARD.with(|c| c.borrow().as_ref().map(|card| card.tab))
}

/// Whether `hwnd` is the card's window.
pub(super) fn is_card(hwnd: HWND) -> bool {
    CARD.with(|c| c.borrow().as_ref().is_some_and(|card| card.popup == hwnd))
}

/// Draws the card into `canvas` (physical pixels, its own corner at 0, 0).
pub(super) fn draw(canvas: &Canvas, painter: &Painter) {
    CARD.with(|c| {
        let card = c.borrow();
        let Some(card) = card.as_ref() else { return };
        let px = |v: f32| (v * card.scale).round() as i32;
        let (w, h) = (card.size.cx, card.size.cy);
        let colors = &card.look.palette;
        let radius = 8.0 * card.scale;
        // the border, then the face inside it
        canvas.fill_rounded(0, 0, w, h, radius, colors.border);
        canvas.fill_rounded(1, 1, w - 1, h - 1, radius, colors.background);

        let picture = RECT { left: px(PAD), top: px(PAD), right: w - px(PAD), bottom: px(PAD) + px(CARD_H) };
        canvas.fill(picture.left, picture.top, picture.right, picture.bottom, colors.hover);
        match &card.card.image {
            Some((iw, ih, rgba)) => {
                canvas.picture(picture.left, picture.top, picture.right, picture.bottom, rgba, *iw, *ih);
            }
            None => draw_lines(canvas, painter, card, picture),
        }

        let ts = card.look.text_scale;
        let (Ok(title_format), Ok(note_format)) =
            (painter.format("Segoe UI", px(13.0 * ts) as f32), painter.format("Segoe UI", px(11.0 * ts) as f32))
        else {
            return;
        };
        let title_top = picture.bottom + px(PAD / 2.0);
        let title_bottom = title_top + px(TITLE_H);
        canvas.text(&title_format, &card.card.title, px(PAD), title_top, w - px(PAD), title_bottom, colors.text);
        canvas.text(
            &note_format,
            &card.card.note,
            px(PAD),
            title_bottom,
            w - px(PAD),
            title_bottom + px(NOTE_H),
            colors.dim,
        );
    });
}

/// A tab Terminal has never rendered: the text its console holds, as small
/// as the widest line needs it to be.
fn draw_lines(canvas: &Canvas, painter: &Painter, card: &Card, into: RECT) {
    if card.card.lines.is_empty() {
        return;
    }
    let px = |v: f32| (v * card.scale).round() as i32;
    let columns = f32::from(card.card.columns.max(20));
    // a monospace character is about half its height wide
    let size = (((into.right - into.left) as f32 - px(6.0) as f32) / (columns * 0.5)).clamp(3.0, 9.0 * card.scale);
    let Ok(format) = painter.format("Consolas", size) else { return };
    let line_height = (size * 1.2).ceil() as i32;
    let mut y = into.top + px(2.0);
    for line in &card.card.lines {
        if y + line_height > into.bottom {
            break;
        }
        if !line.is_empty() {
            canvas.text(&format, line, into.left + px(3.0), y, into.right, y + line_height, card.look.palette.text);
        }
        y += line_height;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_with_neither_picture_nor_text_shows_nothing() {
        assert!(HoverCard::default().is_empty());
        let text = HoverCard { lines: vec![String::new(), String::new()], ..Default::default() };
        assert!(text.is_empty(), "blank lines are nothing to show");
        let some = HoverCard { lines: vec!["root@web01:~#".into()], ..Default::default() };
        assert!(!some.is_empty());
        let pictured = HoverCard { image: Some((1, 1, vec![0, 0, 0, 255])), ..Default::default() };
        assert!(!pictured.is_empty());
    }

    #[test]
    fn the_card_grows_with_the_screen() {
        let small = size_for(1.0);
        let large = size_for(2.0);
        assert!(large.cx == small.cx * 2 && large.cy == small.cy * 2, "{small:?} {large:?}");
    }
}
