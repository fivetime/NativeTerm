//! QQ-style docking (see `docs/ARCHITECTURE.md`, "Sidebar auto-hide /
//! pin"): a window dragged to the top, left or right edge of the screen
//! sticks there; when the pointer leaves it slides away until only a thin
//! strip is visible, and comes back when the pointer touches the strip.
//! Pinned, it stays out. While docked it is always on top.
//!
//! This file is the geometry and the timing; `window.rs` applies it.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

pub use native_term_os::dock::Bounds;

/// Visible strip of a hidden window, in physical pixels.
pub const STRIP: i32 = 4;
/// A window this close to an edge docks there.
pub const SNAP: i32 = 12;
/// The pointer must stay away this long before the window hides.
pub const LEAVE_DELAY: Duration = Duration::from_millis(450);
/// How long the window takes to slide.
pub const SLIDE: Duration = Duration::from_millis(160);
/// Between checks after a move, until the mouse button is released.
pub const DRAG_POLL: Duration = Duration::from_millis(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Left,
    Right,
}

impl Edge {
    pub fn name(self) -> &'static str {
        match self {
            Edge::Top => "top",
            Edge::Left => "left",
            Edge::Right => "right",
        }
    }

    pub fn from_name(name: &str) -> Option<Edge> {
        match name {
            "top" => Some(Edge::Top),
            "left" => Some(Edge::Left),
            "right" => Some(Edge::Right),
            _ => None,
        }
    }
}

/// Shared with the UI: where the window is docked and whether it is pinned.
static EDGE: AtomicU8 = AtomicU8::new(0);
static PINNED: AtomicBool = AtomicBool::new(false);

pub fn docked_edge() -> Option<Edge> {
    match EDGE.load(Ordering::Relaxed) {
        1 => Some(Edge::Top),
        2 => Some(Edge::Left),
        3 => Some(Edge::Right),
        _ => None,
    }
}

pub(crate) fn publish_edge(edge: Option<Edge>) {
    let n = match edge {
        None => 0,
        Some(Edge::Top) => 1,
        Some(Edge::Left) => 2,
        Some(Edge::Right) => 3,
    };
    EDGE.store(n, Ordering::Relaxed);
}

pub fn pinned() -> bool {
    PINNED.load(Ordering::Relaxed)
}

pub fn set_pinned(on: bool) {
    PINNED.store(on, Ordering::Relaxed);
}

/// The edge `frame` (what the user sees of the window) is dragged to, if
/// any. `neighbour` tells whether a monitor continues past a point; an
/// edge with another monitor behind it isn't used (the hidden window
/// would show there).
pub fn snap_edge(frame: Bounds, work: Bounds, neighbour: impl Fn(i32, i32) -> bool) -> Option<Edge> {
    let mid_x = (frame.left + frame.right) / 2;
    let mid_y = (frame.top + frame.bottom) / 2;
    let candidates = [
        (Edge::Top, (frame.top - work.top).abs() <= SNAP, (mid_x, work.top - 1)),
        (Edge::Left, (frame.left - work.left).abs() <= SNAP, (work.left - 1, mid_y)),
        (Edge::Right, (frame.right - work.right).abs() <= SNAP, (work.right, mid_y)),
    ];
    candidates.into_iter().find(|(_, near, (x, y))| *near && !neighbour(*x, *y)).map(|(edge, _, _)| edge)
}

/// Where the window rectangle (`window`, borders included) goes, docked at
/// `edge`, shown or hidden. `frame` is the visible part of the same window.
pub fn docked_position(edge: Edge, window: Bounds, frame: Bounds, work: Bounds, hidden: bool) -> (i32, i32) {
    let inset_left = frame.left - window.left;
    let inset_top = frame.top - window.top;
    let clamp_x =
        |x: i32| x.clamp(work.left - inset_left, (work.right - frame.width() - inset_left).max(work.left - inset_left));
    let clamp_y =
        |y: i32| y.clamp(work.top - inset_top, (work.bottom - frame.height() - inset_top).max(work.top - inset_top));
    let away = |size: i32| if hidden { size - STRIP } else { 0 };
    match edge {
        Edge::Top => (clamp_x(window.left), work.top - inset_top - away(frame.height())),
        Edge::Left => (work.left - inset_left - away(frame.width()), clamp_y(window.top)),
        Edge::Right => (work.right - frame.width() - inset_left + away(frame.width()), clamp_y(window.top)),
    }
}

/// A slide from one position to another.
#[derive(Clone, Copy, Debug)]
pub struct Slide {
    pub from: (i32, i32),
    pub to: (i32, i32),
    pub started: Instant,
}

impl Slide {
    /// The position at `now` (eased), and whether the slide is over.
    pub fn at(&self, now: Instant) -> ((i32, i32), bool) {
        let t = (now.saturating_duration_since(self.started).as_secs_f32() / SLIDE.as_secs_f32()).min(1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        let lerp = |a: i32, b: i32| a + ((b - a) as f32 * eased).round() as i32;
        ((lerp(self.from.0, self.to.0), lerp(self.from.1, self.to.1)), t >= 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Bounds = Bounds { left: 0, top: 0, right: 1920, bottom: 1040 };

    /// A window with 7 px invisible borders left, right and bottom.
    fn window_at(left: i32, top: i32) -> (Bounds, Bounds) {
        let frame = Bounds { left, top, right: left + 400, bottom: top + 700 };
        let window = Bounds { left: left - 7, top, right: left + 407, bottom: top + 707 };
        (window, frame)
    }

    #[test]
    fn edges() {
        let alone = |_: i32, _: i32| false;
        assert_eq!(snap_edge(window_at(300, 8).1, WORK, alone), Some(Edge::Top));
        assert_eq!(snap_edge(window_at(-5, 200).1, WORK, alone), Some(Edge::Left));
        assert_eq!(snap_edge(window_at(1515, 200).1, WORK, alone), Some(Edge::Right));
        assert_eq!(snap_edge(window_at(300, 200).1, WORK, alone), None);
        // top wins in the corner
        assert_eq!(snap_edge(window_at(0, 0).1, WORK, alone), Some(Edge::Top));
        // a second monitor to the right: no docking there
        let right_monitor = |x: i32, _: i32| x >= 1920;
        assert_eq!(snap_edge(window_at(1515, 200).1, WORK, right_monitor), None);
        assert_eq!(snap_edge(window_at(-5, 200).1, WORK, right_monitor), Some(Edge::Left));
    }

    #[test]
    fn positions() {
        let (window, frame) = window_at(300, 8);
        // shown: the frame touches the edge
        assert_eq!(docked_position(Edge::Top, window, frame, WORK, false), (293, 0));
        // hidden: only the strip is on screen
        let (x, y) = docked_position(Edge::Top, window, frame, WORK, true);
        assert_eq!((x, y + 700), (293, STRIP));

        let (window, frame) = window_at(-5, 200);
        assert_eq!(docked_position(Edge::Left, window, frame, WORK, false), (-7, 200));
        let (x, _) = docked_position(Edge::Left, window, frame, WORK, true);
        assert_eq!(x + 7 + 400, STRIP, "frame's right edge at the strip");

        let (window, frame) = window_at(1515, 200);
        assert_eq!(docked_position(Edge::Right, window, frame, WORK, false), (1513, 200));
        let (x, _) = docked_position(Edge::Right, window, frame, WORK, true);
        assert_eq!(x + 7, 1920 - STRIP, "frame's left edge at the strip");

        // kept inside the work area along the edge
        let (window, frame) = window_at(1800, 8);
        assert_eq!(docked_position(Edge::Top, window, frame, WORK, false).0, 1920 - 400 - 7);
    }

    #[test]
    fn slides_end_where_they_should() {
        let started = Instant::now();
        let slide = Slide { from: (0, -696), to: (0, 0), started };
        assert_eq!(slide.at(started), ((0, -696), false));
        let (mid, done) = slide.at(started + SLIDE / 2);
        assert!(!done && mid.1 > -348, "eased: {mid:?}");
        assert_eq!(slide.at(started + SLIDE), ((0, 0), true));
    }

    #[test]
    fn names() {
        for e in [Edge::Top, Edge::Left, Edge::Right] {
            assert_eq!(Edge::from_name(e.name()), Some(e));
        }
    }
}
