//! What dialogs have in common, as SkinUI's `MsgBox`, `Confirm` and
//! `ShowLoading` had it: a large sign at the left saying what kind of
//! thing is said (information, a warning, an error, done, a question),
//! what the dialog has at its right (the program's own: text, choices, a
//! progress bar), and a row of buttons at the bottom, the one that does
//! what the dialog is for coloured, in the order the platform puts them;
//! at the row's left what goes with them ("Don't ask again"). Enter is
//! the coloured button; Escape gives the dialog up (the close button too).
//!
//! [`Message`] is all of it as a modal dialog; [`body`] and [`footer`]
//! are its parts, for a window of its own.

use crate::modal::Modal;
use crate::{thin, Skin};

/// What kind of thing a dialog says: its sign.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Notice {
    Info,
    Warning,
    Error,
    Success,
    Question,
}

/// The sign's size.
const SIGN: f32 = 48.0;

impl Notice {
    fn color(self) -> egui::Color32 {
        let rgb = |hex: u32| egui::Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
        match self {
            // (blue, amber, red, emerald 500)
            Notice::Info | Notice::Question => rgb(0x3b82f6),
            Notice::Warning => rgb(0xf59e0b),
            Notice::Error => rgb(0xef4444),
            Notice::Success => rgb(0x10b981),
        }
    }

    /// The sign: a round of its colour with a white mark in it, lines
    /// and dots (no font has to have them).
    pub fn paint(self, painter: &egui::Painter, rect: egui::Rect) {
        let c = rect.center();
        let r = rect.width().min(rect.height()) / 2.0;
        painter.circle_filled(c, r, self.color());
        painter.circle_stroke(c, r - 0.5, egui::Stroke::new(1.0_f32, thin(egui::Color32::BLACK, 30)));
        let white = egui::Color32::WHITE;
        let line = egui::Stroke::new(r * 0.16, white);
        let at = |x: f32, y: f32| c + egui::vec2(x, y) * r;
        match self {
            Notice::Error => {
                painter.line_segment([at(-0.35, -0.35), at(0.35, 0.35)], line);
                painter.line_segment([at(0.35, -0.35), at(-0.35, 0.35)], line);
            }
            Notice::Warning => {
                painter.line_segment([at(0.0, -0.5), at(0.0, 0.15)], line);
                painter.circle_filled(at(0.0, 0.45), r * 0.1, white);
            }
            Notice::Info => {
                painter.circle_filled(at(0.0, -0.45), r * 0.1, white);
                painter.line_segment([at(0.0, -0.18), at(0.0, 0.5)], line);
            }
            Notice::Success => {
                let mark = vec![at(-0.4, 0.02), at(-0.12, 0.3), at(0.42, -0.28)];
                painter.add(egui::Shape::line(mark, line));
            }
            Notice::Question => {
                let curve: Vec<egui::Pos2> = (0..=12)
                    .map(|i| {
                        let a = std::f32::consts::PI * (1.0 + 1.25 * i as f32 / 12.0);
                        at(0.25 * a.cos(), -0.28 + 0.25 * a.sin())
                    })
                    .chain([at(0.0, 0.05), at(0.0, 0.18)])
                    .collect();
                painter.add(egui::Shape::line(curve, line));
                painter.circle_filled(at(0.0, 0.45), r * 0.1, white);
            }
        }
    }
}

/// What a button in the row is for, which is how it looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// What the dialog is for (OK, Save, Connect): coloured; Enter.
    Primary,
    /// The others (Cancel, Skip).
    Plain,
    /// What takes something away, in place of the primary one.
    Danger,
}

/// Where the platform puts the button that does what the dialog is for:
/// first (Windows, KDE: OK Cancel) or last (macOS, GNOME: Cancel OK).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    PrimaryFirst,
    PrimaryLast,
}

impl Order {
    /// The platform's, where the program says nothing (Linux: GNOME's,
    /// the program tells KDE apart).
    #[must_use]
    pub fn platform() -> Order {
        if cfg!(windows) {
            Order::PrimaryFirst
        } else {
            Order::PrimaryLast
        }
    }
}

/// A button's least width (`min-w-[88px]`) and height (`h-9`).
const BUTTON_WIDTH: f32 = 88.0;
const BUTTON_HEIGHT: f32 = 32.0;

/// A dialog's button, as the row has them (for one at the row's left
/// too): egui's own, as large as the row's, the primary one and the one
/// that takes something away in their colours; the others as the theme
/// has buttons.
pub fn button(ui: &mut egui::Ui, skin: &Skin, text: &str, role: Role, enabled: bool) -> egui::Response {
    let p = &skin.palette;
    let colored = |fill: egui::Color32, on: egui::Color32| {
        egui::Button::new(egui::RichText::new(text).color(on)).fill(fill).stroke(egui::Stroke::NONE)
    };
    let button = match role {
        Role::Primary => colored(p.primary, p.on_primary),
        Role::Danger => colored(p.danger, egui::Color32::WHITE),
        Role::Plain => egui::Button::new(text),
    };
    ui.add_enabled(enabled, button.min_size(egui::vec2(BUTTON_WIDTH, BUTTON_HEIGHT)).corner_radius(8.0))
}

/// A button of the row: its text, what it is for, whether it can be
/// pressed now, whether Enter presses it, what it says under the pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub text: String,
    pub role: Role,
    pub enabled: bool,
    pub default: bool,
    pub hint: Option<String>,
}

impl Choice {
    #[must_use]
    pub fn new(text: impl Into<String>, role: Role) -> Choice {
        Choice { text: text.into(), role, enabled: true, default: false, hint: None }
    }

    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Choice {
        self.enabled = enabled;
        self
    }

    /// Enter presses this one rather than the primary one (Cancel where
    /// what the dialog is for must not be done by a key press).
    #[must_use]
    pub fn default(mut self) -> Choice {
        self.default = true;
        self
    }

    #[must_use]
    pub fn hint(mut self, hint: impl Into<String>) -> Choice {
        self.hint = Some(hint.into());
        self
    }
}

/// The row of buttons at a dialog's bottom: `left` at its left (a check
/// box, a button that is not one of the answers), `choices` at its right
/// in the platform's order (`choices` as written: the primary one first).
/// Which one was pressed, by its place in `choices`; Enter is the one
/// marked default, else the primary one (never the one that takes
/// something away, unless marked), where it can be pressed and no field
/// has the keyboard.
pub fn footer(ui: &mut egui::Ui, skin: &Skin, left: impl FnOnce(&mut egui::Ui), choices: &[Choice]) -> Option<usize> {
    const GAP: f32 = 10.0;
    let mut pressed = None;
    // left to right, in the platform's order
    let mut order: Vec<usize> = (0..choices.len()).collect();
    if skin.order == Order::PrimaryLast {
        order.reverse();
    }
    let font = egui::TextStyle::Button.resolve(ui.style());
    let padding = ui.spacing().button_padding.x;
    let buttons: f32 = choices
        .iter()
        .map(|c| {
            let text = ui.painter().layout_no_wrap(c.text.clone(), font.clone(), egui::Color32::WHITE).size().x;
            (text + 2.0 * padding).max(BUTTON_WIDTH)
        })
        .sum::<f32>()
        + GAP * choices.len().saturating_sub(1) as f32;
    // as wide as what is above it (a dialog grows to what it has, not to
    // its buttons' row), or the room there is where nothing is (a
    // window's bottom bar)
    let above = ui.min_rect().width();
    let room = if above > 1.0 { above } else { ui.available_width() };
    ui.horizontal(|ui| {
        ui.set_min_height(BUTTON_HEIGHT);
        let start = ui.cursor().min.x;
        left(ui);
        let used = ui.cursor().min.x - start;
        ui.add_space((room - used - buttons).max(0.0));
        ui.spacing_mut().item_spacing.x = GAP;
        for i in order {
            let c = &choices[i];
            let mut response = button(ui, skin, &c.text, c.role, c.enabled);
            if let Some(hint) = &c.hint {
                response = response.on_hover_text(hint);
            }
            if response.clicked() {
                pressed = Some(i);
            }
        }
    });
    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter)) && ui.memory(|m| m.focused().is_none());
    if pressed.is_none() && enter {
        let default = choices.iter().position(|c| c.default);
        pressed =
            default.or_else(|| choices.iter().position(|c| c.role == Role::Primary)).filter(|i| choices[*i].enabled);
    }
    pressed
}

/// A dialog's body: `notice`'s sign at the left, `content` at its right.
pub fn body<R>(ui: &mut egui::Ui, notice: Option<Notice>, content: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.horizontal_top(|ui| {
        if let Some(notice) = notice {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(SIGN, SIGN), egui::Sense::hover());
            notice.paint(ui.painter(), rect);
            ui.add_space(12.0);
        }
        ui.vertical(content).inner
    })
    .inner
}

/// A modal dialog with a sign, what the program puts in it, and a row of
/// buttons: SkinUI's `MsgBox` and `Confirm`, and with a progress bar its
/// `ShowLoading`.
pub struct Message<'a> {
    modal: Modal<'a>,
    notice: Option<Notice>,
    choices: Vec<Choice>,
}

/// What happened in a [`Message`] this frame.
pub struct MessageShown<R> {
    /// What the program's part returned.
    pub inner: R,
    /// The button pressed, by its place.
    pub pressed: Option<usize>,
    /// Given up: the close button, or Escape.
    pub closed: bool,
}

impl<'a> Message<'a> {
    /// A dialog `title`; `id_salt` tells it from others.
    #[must_use]
    pub fn new(id_salt: impl std::hash::Hash + std::fmt::Debug, title: &'a str) -> Message<'a> {
        Message { modal: Modal::new(id_salt, title), notice: None, choices: Vec::new() }
    }

    #[must_use]
    pub fn notice(mut self, notice: Notice) -> Message<'a> {
        self.notice = Some(notice);
        self
    }

    /// A button, after those given before (the primary one first).
    #[must_use]
    pub fn choice(mut self, choice: Choice) -> Message<'a> {
        self.choices.push(choice);
        self
    }

    /// The title bar's icon.
    #[must_use]
    pub fn icon(mut self, icon: char) -> Message<'a> {
        self.modal = self.modal.icon(icon);
        self
    }

    #[must_use]
    pub fn title_height(mut self, height: f32) -> Message<'a> {
        self.modal = self.modal.title_height(height);
        self
    }

    #[must_use]
    pub fn width(mut self, width: f32) -> Message<'a> {
        self.modal = self.modal.width(width);
        self
    }

    /// Shows it: `content` is the program's part, `left` what goes at the
    /// buttons' left.
    pub fn show<R>(
        self,
        ctx: &egui::Context,
        skin: &Skin,
        content: impl FnOnce(&mut egui::Ui) -> R,
        left: impl FnOnce(&mut egui::Ui),
    ) -> MessageShown<R> {
        let Message { modal, notice, choices } = self;
        let shown = modal.show(ctx, skin, |ui| {
            let inner = body(ui, notice, content);
            ui.add_space(16.0);
            let pressed = footer(ui, skin, left, &choices);
            (inner, pressed)
        });
        let (inner, pressed) = shown.inner;
        MessageShown { inner, pressed, closed: shown.closed }
    }
}
