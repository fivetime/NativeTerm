//! Short messages in the corner of the main window: what an action just
//! did, when nothing else on screen says it — "closed 5 disconnected
//! tabs", "sent to 3 sessions". They fade in, wait a few seconds and
//! fade out; a click takes one away at once.
//!
//! Problems are not shown this way. They stay in the notice line at the
//! top of the window until the person clears them, because a message
//! that disappears by itself is the wrong place for something that went
//! wrong.
//!
//! Anything, on any thread, can ask for one (`toast::done`), and the
//! window is woken to show it.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What the message is about (it picks the icon).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Something was done.
    Done,
    /// Something is under way, or worth knowing.
    Info,
}

/// How long one stays before it fades.
const STAY: Duration = Duration::from_secs(4);
/// The fade at each end (skipped when Windows' animations are off).
const FADE: Duration = Duration::from_millis(180);
/// At most this many at once; older ones go first.
const AT_ONCE: usize = 4;
/// Segoe Fluent Icons (Segoe MDL2 Assets on Windows 10), the fallback
/// font the window already loads: a tick, and an "i" in a circle.
const DONE_GLYPH: char = '\u{E73E}';
const INFO_GLYPH: char = '\u{E946}';

static QUEUE: Mutex<Vec<(Kind, String)>> = Mutex::new(Vec::new());
type Wake = Box<dyn Fn() + Send + Sync>;
static WAKE: Mutex<Option<Wake>> = Mutex::new(None);

fn add(kind: Kind, text: String) {
    if text.trim().is_empty() {
        return;
    }
    QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push((kind, text));
    if let Some(wake) = WAKE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        wake();
    }
}

/// Say that something was done.
pub fn done(text: impl Into<String>) {
    add(Kind::Done, text.into());
}

/// Say what is happening.
pub fn info(text: impl Into<String>) {
    add(Kind::Info, text.into());
}

/// How the window is woken when a message arrives from another thread.
pub fn wake_with(wake: impl Fn() + Send + Sync + 'static) {
    *WAKE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(wake));
}

/// Waiting messages (the window takes them on its next frame).
fn take() -> Vec<(Kind, String)> {
    std::mem::take(&mut *QUEUE.lock().unwrap_or_else(|e| e.into_inner()))
}

struct Toast {
    kind: Kind,
    text: String,
    /// When it goes away (moved earlier by a click).
    until: Instant,
}

/// What the window shows in its corner.
#[derive(Default)]
pub struct Toasts {
    shown: Vec<Toast>,
}

impl Toasts {
    /// Take what is waiting and draw what is on screen. `animate`: the
    /// person's Windows animation setting.
    pub fn show(&mut self, ctx: &egui::Context, animate: bool) {
        let now = Instant::now();
        let fade = if animate { FADE } else { Duration::ZERO };
        for (kind, text) in take() {
            self.shown.push(Toast { kind, text, until: now + STAY + fade });
        }
        // the newest are the ones worth seeing
        while self.shown.len() > AT_ONCE {
            self.shown.remove(0);
        }
        self.shown.retain(|t| t.until > now);
        if self.shown.is_empty() {
            return;
        }
        let next = self.shown.iter().map(|t| t.until).min().unwrap_or(now);
        ctx.request_repaint_after(next.saturating_duration_since(now).min(Duration::from_millis(100)));

        let mut dismissed = None;
        let visuals = ctx.global_style().visuals.clone();
        egui::Area::new(egui::Id::new("nativeterm-toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -16.0))
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Max), |ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    for (at, toast) in self.shown.iter().enumerate().rev() {
                        let left = toast.until.saturating_duration_since(now);
                        let opacity = fading(left, fade);
                        let face = visuals.window_fill.gamma_multiply(opacity);
                        let response = egui::Frame::new()
                            .fill(face)
                            .stroke(visuals.window_stroke)
                            .corner_radius(egui::CornerRadius::same(8))
                            .inner_margin(egui::Margin::symmetric(12, 8))
                            .shadow(visuals.window_shadow)
                            .show(ui, |ui| {
                                // the icon first, whatever way the corner lays out
                                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                    let glyph = match toast.kind {
                                        Kind::Done => DONE_GLYPH,
                                        Kind::Info => INFO_GLYPH,
                                    };
                                    ui.colored_label(visuals.text_color().gamma_multiply(opacity), glyph.to_string());
                                    ui.colored_label(visuals.text_color().gamma_multiply(opacity), &toast.text);
                                });
                            })
                            .response;
                        if response.interact(egui::Sense::click()).clicked() {
                            dismissed = Some(at);
                        }
                    }
                });
            });
        if let Some(at) = dismissed {
            self.shown.remove(at);
        }
    }

    /// How many are on screen (tests).
    #[must_use]
    pub fn count(&self) -> usize {
        self.shown.len()
    }
}

/// How solid a message is with `left` to go: it fades in over its first
/// `fade` and out over its last.
fn fading(left: Duration, fade: Duration) -> f32 {
    if fade.is_zero() {
        return 1.0;
    }
    let out = left.as_secs_f32() / fade.as_secs_f32();
    let in_ = (STAY.as_secs_f32() + fade.as_secs_f32() - left.as_secs_f32()) / fade.as_secs_f32();
    out.min(in_).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_fades_in_and_out() {
        let fade = Duration::from_millis(200);
        let full = STAY + fade;
        assert_eq!(fading(full, fade), 0.0, "just born");
        assert!((fading(full - fade / 2, fade) - 0.5).abs() < 0.01, "half way in");
        assert!(fading(STAY, fade) > 0.999, "up");
        assert!(fading(fade, fade) > 0.999, "still up");
        assert!((fading(fade / 2, fade) - 0.5).abs() < 0.01, "half way out");
        assert_eq!(fading(Duration::ZERO, fade), 0.0);
        // with animations off it is simply there
        assert_eq!(fading(full, Duration::ZERO), 1.0);
        assert_eq!(fading(Duration::ZERO, Duration::ZERO), 1.0);
    }

    #[test]
    fn empty_messages_are_not_kept() {
        take();
        done("   ");
        info(String::new());
        assert!(take().is_empty());
        done("closed 5 tabs");
        assert_eq!(take().len(), 1);
    }
}
