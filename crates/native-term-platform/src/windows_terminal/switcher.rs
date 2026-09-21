//! The grid that appears while Ctrl+Tab is held: every tab of the
//! Terminal window in front, as a tile with its last picture, the one
//! that would be switched to highlighted. Further Tab presses (or the
//! arrow keys) move the choice, releasing Ctrl switches, Esc leaves
//! everything as it was.
//!
//! Windows Terminal's own Ctrl+Tab list shows titles and icons; with
//! twenty tabs of ssh sessions and AI tools, the pictures are what tells
//! them apart. Terminal has no extension point for this (see "Taking
//! over Ctrl+Tab" in `docs/ARCHITECTURE.md`), so NativeTerm takes the
//! key with the low-level keyboard hook it already has and draws the
//! grid in the same non-activating Direct2D popup as the tab menu and
//! the hover card. Terminal keeps the focus throughout; NativeTerm only
//! selects a tab at the end, through UI Automation.
//!
//! Off unless the person turns it on: taking a key away from another
//! program is not something to do by default.

use std::cell::RefCell;

use windows::Win32::Foundation::{COLORREF, HWND, RECT, SIZE};

use super::menu_draw::{Canvas, Painter};
use super::theme::Look;

/// A tile's picture, in effective pixels.
pub(super) const TILE_W: f32 = 200.0;
pub(super) const TILE_H: f32 = 112.0;
/// The title under it.
pub(super) const LABEL_H: f32 = 18.0;
pub(super) const GAP: f32 = 8.0;
pub(super) const PAD: f32 = 10.0;
/// At most this many tiles across.
pub(super) const MAX_COLUMNS: usize = 5;
/// A window with more tabs than this shows the first ones (the grid is
/// meant for a glance, not for a hundred tabs).
pub const MAX_TILES: usize = 20;

/// One tab in the grid.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SwitcherTab {
    pub window: isize,
    pub index: usize,
    /// What the tile says: the session's name, or the tab's title.
    pub title: String,
    /// What Terminal calls the tab, for finding it again when the strip
    /// has moved under the grid.
    pub name: String,
    /// The tab's last picture: width, height, RGBA.
    pub image: Option<(u32, u32, Vec<u8>)>,
    /// What was on its screen, when there is no picture.
    pub lines: Vec<String>,
    /// How wide that screen is, for sizing the text.
    pub columns: u16,
    /// Whether it is the window's selected tab.
    pub selected: bool,
}

pub(super) struct Grid {
    pub(super) popup: HWND,
    pub(super) window: isize,
    pub(super) tabs: Vec<SwitcherTab>,
    /// Which tile is highlighted.
    pub(super) pick: usize,
    pub(super) look: Look,
    pub(super) scale: f32,
    pub(super) size: SIZE,
    pub(super) columns: usize,
}

thread_local! {
    pub(super) static GRID: RefCell<Option<Grid>> = const { RefCell::new(None) };
}

/// How the tiles are laid out for `count` tabs: (size of the window,
/// tiles across).
pub(super) fn layout(count: usize, scale: f32) -> (SIZE, usize) {
    let count = count.clamp(1, MAX_TILES);
    // as square as it can be, never wider than `MAX_COLUMNS`
    let columns = (count as f32).sqrt().ceil() as usize;
    let columns = columns.clamp(1, MAX_COLUMNS).min(count);
    let rows = count.div_ceil(columns);
    let px = |v: f32| (v * scale).round() as i32;
    let tile_w = TILE_W + GAP;
    let tile_h = TILE_H + LABEL_H + GAP;
    let size =
        SIZE { cx: px(PAD * 2.0 - GAP + tile_w * columns as f32), cy: px(PAD * 2.0 - GAP + tile_h * rows as f32) };
    (size, columns)
}

/// Whether `hwnd` is the grid's window.
pub(super) fn is_grid(hwnd: HWND) -> bool {
    GRID.with(|g| g.borrow().as_ref().is_some_and(|grid| grid.popup == hwnd))
}

/// The tab the grid would switch to.
pub(super) fn picked() -> Option<(isize, usize, String)> {
    GRID.with(|g| {
        let grid = g.borrow();
        let grid = grid.as_ref()?;
        let tab = grid.tabs.get(grid.pick)?;
        Some((grid.window, tab.index, tab.name.clone()))
    })
}

/// Move the choice by `steps` tiles, wrapping around.
pub(super) fn step(steps: isize) -> bool {
    GRID.with(|g| {
        let mut grid = g.borrow_mut();
        let Some(grid) = grid.as_mut() else { return false };
        let count = grid.tabs.len() as isize;
        if count == 0 {
            return false;
        }
        grid.pick = (grid.pick as isize + steps).rem_euclid(count) as usize;
        true
    })
}

/// Pick a tile outright (the mouse). Whether the choice moved.
pub(super) fn pick(at: usize) -> bool {
    GRID.with(|g| {
        let mut grid = g.borrow_mut();
        let Some(grid) = grid.as_mut() else { return false };
        if at >= grid.tabs.len() || at == grid.pick {
            return false;
        }
        grid.pick = at;
        true
    })
}

/// Which tile a point of the grid's window is on (physical pixels).
pub(super) fn tile_at(x: i32, y: i32) -> Option<usize> {
    GRID.with(|g| {
        let grid = g.borrow();
        let grid = grid.as_ref()?;
        let px = |v: f32| (v * grid.scale).round() as i32;
        let (step_x, step_y) = (px(TILE_W + GAP), px(TILE_H + LABEL_H + GAP));
        let (dx, dy) = (x - px(PAD), y - px(PAD));
        if dx < 0 || dy < 0 {
            return None;
        }
        let (column, row) = (dx / step_x, dy / step_y);
        // the gap between tiles belongs to neither
        if dx % step_x > px(TILE_W) || dy % step_y > px(TILE_H + LABEL_H) || column as usize >= grid.columns {
            return None;
        }
        let at = row as usize * grid.columns + column as usize;
        (at < grid.tabs.len()).then_some(at)
    })
}

/// Move by a row (the down and up keys).
pub(super) fn step_row(rows: isize) -> bool {
    let columns = GRID.with(|g| g.borrow().as_ref().map_or(1, |grid| grid.columns as isize));
    step(rows * columns)
}

/// Draws the grid into `canvas` (physical pixels, its own corner at 0, 0).
pub(super) fn draw(canvas: &Canvas, painter: &Painter) {
    GRID.with(|g| {
        let grid = g.borrow();
        let Some(grid) = grid.as_ref() else { return };
        let px = |v: f32| (v * grid.scale).round() as i32;
        let colors = &grid.look.palette;
        let radius = 8.0 * grid.scale;
        canvas.fill_rounded(0, 0, grid.size.cx, grid.size.cy, radius, colors.border);
        canvas.fill_rounded(1, 1, grid.size.cx - 1, grid.size.cy - 1, radius, colors.background);

        let ts = grid.look.text_scale;
        let Ok(format) = painter.format("Segoe UI", px(12.0 * ts) as f32) else { return };
        for (at, tab) in grid.tabs.iter().enumerate().take(MAX_TILES) {
            let (row, column) = (at / grid.columns, at % grid.columns);
            let left = px(PAD + (TILE_W + GAP) * column as f32);
            let top = px(PAD + (TILE_H + LABEL_H + GAP) * row as f32);
            let right = left + px(TILE_W);
            let bottom = top + px(TILE_H);
            let chosen = at == grid.pick;
            // the picked tile is framed; the window's own tab is marked
            // by its title's color alone
            if chosen {
                let edge = px(3.0).max(2);
                canvas.fill_rounded(
                    left - edge,
                    top - edge,
                    right + edge,
                    bottom + px(LABEL_H) + edge,
                    radius,
                    colors.accent,
                );
            }
            canvas.fill(left, top, right, bottom, colors.hover);
            match &tab.image {
                Some((w, h, rgba)) => canvas.picture(left, top, right, bottom, rgba, *w, *h),
                None => draw_lines(canvas, painter, grid, tab, RECT { left, top, right, bottom }),
            }
            // on the accent frame the name needs the accent's own
            // contrast; the tab the window is already showing is dimmed
            let color = if chosen {
                on(colors.accent)
            } else if tab.selected {
                colors.dim
            } else {
                colors.text
            };
            canvas.text(&format, &tab.title, left, bottom, right, bottom + px(LABEL_H), color);
        }
    });
}

/// Black or white, whichever can be read on `background`.
fn on(background: COLORREF) -> COLORREF {
    let (b, g, r) = ((background.0 >> 16) & 0xff, (background.0 >> 8) & 0xff, background.0 & 0xff);
    // the usual weighted brightness
    let light = (r * 299 + g * 587 + b * 114) / 1000 > 140;
    if light {
        COLORREF(0x0000_0000)
    } else {
        COLORREF(0x00ff_ffff)
    }
}

/// A tab with no picture: the text that was last on it.
fn draw_lines(canvas: &Canvas, painter: &Painter, grid: &Grid, tab: &SwitcherTab, into: RECT) {
    if tab.lines.is_empty() {
        return;
    }
    let px = |v: f32| (v * grid.scale).round() as i32;
    let columns = f32::from(tab.columns.max(20));
    let size = (((into.right - into.left) as f32 - px(4.0) as f32) / (columns * 0.5)).clamp(3.0, 8.0 * grid.scale);
    let Ok(format) = painter.format("Consolas", size) else { return };
    let line_height = (size * 1.2).ceil() as i32;
    let mut y = into.top + px(2.0);
    for line in &tab.lines {
        if y + line_height > into.bottom {
            break;
        }
        if !line.is_empty() {
            canvas.text(&format, line, into.left + px(3.0), y, into.right, y + line_height, grid.look.palette.text);
        }
        y += line_height;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_is_as_square_as_it_can_be() {
        let (size, columns) = layout(1, 1.0);
        assert_eq!(columns, 1);
        assert_eq!(size.cx, (PAD * 2.0 + TILE_W) as i32);
        assert_eq!(layout(4, 1.0).1, 2);
        assert_eq!(layout(5, 1.0).1, 3, "three across, two rows");
        assert_eq!(layout(9, 1.0).1, 3);
        assert_eq!(layout(30, 1.0).1, MAX_COLUMNS, "never wider than that");
        // a taller grid for more tabs, and the scale carries through
        let (small, _) = layout(4, 1.0);
        let (big, _) = layout(16, 1.0);
        assert!(big.cy > small.cy && big.cx > small.cx);
        assert_eq!(layout(4, 2.0).0.cx, small.cx * 2);
    }

    /// A grid of `count` tabs, the first one the window's own, with no
    /// window behind it: everything but the painting can be tried this way.
    fn grid(count: usize) -> Grid {
        let (size, columns) = layout(count, 1.0);
        let tabs = (0..count)
            .map(|i| SwitcherTab {
                window: 7,
                index: i,
                title: format!("tab {i}"),
                name: format!("shell {i}"),
                selected: i == 0,
                ..Default::default()
            })
            .collect();
        Grid {
            popup: HWND(std::ptr::null_mut()),
            window: 7,
            tabs,
            pick: 0,
            look: crate::windows_terminal::theme::look(std::path::Path::new("no-such-settings.json")),
            scale: 1.0,
            size,
            columns,
        }
    }

    fn with_grid(count: usize, body: impl FnOnce()) {
        GRID.with(|g| *g.borrow_mut() = Some(grid(count)));
        body();
        GRID.with(|g| *g.borrow_mut() = None);
    }

    fn at() -> Option<usize> {
        picked().map(|(_, index, _)| index)
    }

    #[test]
    fn the_choice_wraps_around() {
        // without a grid nothing moves
        assert!(!step(1));
        assert_eq!(picked(), None);
        with_grid(5, || {
            assert!(step(1));
            assert_eq!(at(), Some(1));
            assert!(step(-1));
            assert_eq!(at(), Some(0));
            assert!(step(-1));
            assert_eq!(at(), Some(4), "back from the first goes to the last");
            assert!(step(1));
            assert_eq!(at(), Some(0));
            // five tabs are three across, so a row down is three along
            assert!(step_row(1));
            assert_eq!(at(), Some(3));
            assert!(step_row(1));
            assert_eq!(at(), Some(1), "and it wraps like the tabs do");
        });
    }

    #[test]
    fn the_tab_is_switched_to_by_the_name_terminal_knows() {
        with_grid(3, || {
            step(2);
            // the tile says "tab 2"; the strip is searched for "shell 2"
            assert_eq!(picked(), Some((7, 2, "shell 2".to_string())));
        });
    }

    #[test]
    fn the_mouse_finds_the_tile_it_is_over() {
        with_grid(4, || {
            let inside = |v: f32| (v + 4.0) as i32;
            assert_eq!(tile_at(inside(PAD), inside(PAD)), Some(0));
            assert_eq!(tile_at(inside(PAD + TILE_W + GAP), inside(PAD)), Some(1));
            assert_eq!(tile_at(inside(PAD), inside(PAD + TILE_H + LABEL_H + GAP)), Some(2), "the second row");
            // the gaps and the edges belong to no tile
            assert_eq!(tile_at((PAD + TILE_W + 2.0) as i32, inside(PAD)), None);
            assert_eq!(tile_at(inside(PAD), (PAD + TILE_H + LABEL_H + 2.0) as i32), None);
            assert_eq!(tile_at(0, 0), None);
            assert_eq!(tile_at(9_999, 9_999), None);
            // a tile the mouse picks becomes the choice, once
            assert!(pick(3));
            assert_eq!(at(), Some(3));
            assert!(!pick(3), "already there");
            assert!(!pick(9), "not a tile");
        });
    }

    #[test]
    fn the_picked_name_can_be_read_on_any_accent() {
        assert_eq!(on(COLORREF(0x00ff_ffff)), COLORREF(0), "black on white");
        assert_eq!(on(COLORREF(0)), COLORREF(0x00ff_ffff), "white on black");
        // 0x00BBGGRR: a light blue accent takes black, a dark one white
        assert_eq!(on(COLORREF(0x00ff_c24c)), COLORREF(0));
        assert_eq!(on(COLORREF(0x00b8_5f00)), COLORREF(0x00ff_ffff));
    }

    #[test]
    fn a_grid_with_fewer_tiles_than_tabs_is_not_confused() {
        // more tabs than tiles: the layout stays within the maximum
        let (size, columns) = layout(60, 1.5);
        assert_eq!(columns, MAX_COLUMNS);
        let rows = MAX_TILES.div_ceil(MAX_COLUMNS);
        assert_eq!(size.cy, ((PAD * 2.0 - GAP + (TILE_H + LABEL_H + GAP) * rows as f32) * 1.5).round() as i32);
    }
}
