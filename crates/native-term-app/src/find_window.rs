//! The "Find" dialog, as SecureCRT's: a small window of its own, above the
//! others, that stays while the person goes from match to match. The
//! terminal that asked is another program's window (see `find`).

use std::cell::RefCell;
use std::time::{Duration, Instant};

use native_term_app::find::{self, Find, Outcome, Question};
use native_term_app::{t, Core};

use crate::window::Place;

/// After "Find Next" the terminal finds and asks again; one that has not
/// by then is gone, and the dialog goes too.
const ANSWER: Duration = Duration::from_secs(15);
/// What the field takes from the pane's selection: a line of this many
/// characters at most.
const INITIAL: usize = 200;

thread_local! {
    /// What the terminal asked since the dialog last looked.
    static ASKED: RefCell<Vec<Question>> = const { RefCell::new(Vec::new()) };
    /// The open dialog's context (to wake it); `None`: no dialog.
    static WINDOW: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
}

/// The terminal asks what to find (see `find::answer`).
pub fn asked(question: Question, core: Core) {
    let open = WINDOW.with(|w| w.borrow().clone());
    match open {
        Some(ctx) => {
            ASKED.with(|a| a.borrow_mut().push(question));
            ctx.request_repaint();
        }
        // what to find next, of a dialog that was closed meanwhile
        None if question.result.is_some() => find::answer(question.ticket, None),
        None => {
            ASKED.with(|a| a.borrow_mut().push(question));
            let viewport = egui::ViewportBuilder::default()
                .with_title(t!("find-title"))
                .with_inner_size([520.0, 190.0])
                .with_resizable(false)
                .with_minimize_button(false)
                .with_maximize_button(false)
                .with_always_on_top();
            // where the person left it (it stays up while they find, and
            // where the pointer is it is over what is found, as often as
            // not); the first time where the pointer is: in the terminal's
            // window, on the menu's item
            let left = core.setting(find::PLACE_SETTING).and_then(|s| find::place_from_setting(&s));
            let place = left.map(|(x, y)| Place::At(x, y)).or_else(crate::window::pointer);
            let settings = core.clone();
            let moved = move |x: i32, y: i32| settings.set_setting(find::PLACE_SETTING, &format!("{x},{y}"));
            crate::window::open_at("find", viewport, place, moved, move |ctx| Box::new(FindWindow::new(ctx, core)));
        }
    }
}

struct FindWindow {
    core: Core,
    find: Find,
    /// The terminal's question "Find Next" answers; `None` while the
    /// terminal finds.
    ticket: Option<u64>,
    /// Since when the terminal has been finding.
    finding: Option<Instant>,
    outcome: Option<(Outcome, String)>,
    /// The field gets the keyboard when the window opens.
    focused: bool,
    done: bool,
}

impl FindWindow {
    fn new(ctx: &egui::Context, core: Core) -> FindWindow {
        WINDOW.with(|w| *w.borrow_mut() = Some(ctx.clone()));
        let find = Find::saved(&core, String::new());
        FindWindow { core, find, ticket: None, finding: None, outcome: None, focused: false, done: false }
    }

    fn take_asked(&mut self) {
        for question in ASKED.with(|a| std::mem::take(&mut *a.borrow_mut())) {
            // one question at a time: an earlier one's terminal is told
            // that nothing more is to be found for it
            if let Some(earlier) = self.ticket.replace(question.ticket) {
                find::answer(earlier, None);
            }
            self.finding = None;
            if let Some(initial) = question.initial.filter(|i| one_line(i)) {
                self.find.text = initial;
                self.focused = false;
            }
            self.outcome = question.result.map(|result| (Outcome::of(result), self.find.text.clone()));
        }
    }

    fn find_next(&mut self) {
        let Some(ticket) = self.ticket.take() else { return };
        self.find.save(&self.core);
        self.finding = Some(Instant::now());
        find::answer(ticket, Some(self.find.clone()));
    }

    fn close(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            find::answer(ticket, None);
        }
        self.done = true;
    }
}

/// Whether the text can go into the field as it is.
fn one_line(text: &str) -> bool {
    !text.is_empty() && text.chars().count() <= INITIAL && !text.contains(['\n', '\r'])
}

impl Drop for FindWindow {
    fn drop(&mut self) {
        self.close();
        WINDOW.with(|w| *w.borrow_mut() = None);
        // asked between the last look and now: nobody is left to answer
        for question in ASKED.with(|a| std::mem::take(&mut *a.borrow_mut())) {
            find::answer(question.ticket, None);
        }
    }
}

impl crate::window::Ui for FindWindow {
    fn ui(&mut self, ui: &mut egui::Ui) {
        if let Some(theme) = *crate::app::THEME.lock().unwrap_or_else(|e| e.into_inner()) {
            if ui.ctx().options(|o| o.theme_preference) != theme {
                ui.ctx().set_theme(theme);
            }
        }
        self.take_asked();
        if let Some(since) = self.finding {
            if since.elapsed() > ANSWER {
                // the terminal is gone
                self.done = true;
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(500));
            }
        }
        let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        let can_find = self.ticket.is_some() && !self.find.text.is_empty();
        let frame = egui::Frame::NONE.inner_margin(14.0_f32).fill(ui.visuals().panel_fill);
        // the buttons in a column of their own at the right, as SecureCRT's
        egui::Panel::right("find-buttons").frame(frame).show_separator_line(false).resizable(false).show_inside(
            ui,
            |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                let size = egui::vec2(110.0, 26.0);
                if ui.add_enabled(can_find, egui::Button::new(t!("find-next")).min_size(size)).clicked()
                    || (enter && can_find)
                {
                    self.find_next();
                }
                if ui.add(egui::Button::new(t!("button-cancel")).min_size(size)).clicked() || escape {
                    self.close();
                }
            },
        );
        let frame = frame.inner_margin(egui::Margin { right: 0, ..egui::Margin::same(14) });
        egui::CentralPanel::default().frame(frame).show_inside(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.horizontal(|ui| {
                ui.label(t!("find-what"));
                let edit = ui.add(egui::TextEdit::singleline(&mut self.find.text).desired_width(f32::INFINITY));
                if !self.focused {
                    self.focused = true;
                    edit.request_focus();
                }
            });
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.checkbox(&mut self.find.whole_word, t!("find-whole-word"));
                    ui.checkbox(&mut self.find.match_case, t!("find-match-case"));
                    ui.checkbox(&mut self.find.wrap, t!("find-wrap"));
                });
                ui.add_space(12.0);
                ui.group(|ui| {
                    ui.vertical(|ui| {
                        ui.label(t!("find-direction"));
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.find.up, true, t!("find-up"));
                            ui.radio_value(&mut self.find.up, false, t!("find-down"));
                        });
                    });
                });
            });
            match &self.outcome {
                Some((Outcome::Shown { position, count }, _)) => {
                    ui.label(t!("find-shown", position = i64::from(*position), count = i64::from(*count)));
                }
                Some((Outcome::NoMore, _)) => {
                    ui.colored_label(ui.visuals().warn_fg_color, t!("find-no-more"));
                }
                Some((Outcome::NotFound, text)) => {
                    ui.colored_label(ui.visuals().warn_fg_color, t!("find-not-found", text = text.as_str()));
                }
                None => {}
            }
        });
    }

    fn wants_close(&self) -> bool {
        self.done
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn what_the_field_takes_from_the_selection() {
        assert!(super::one_line("eth0: link up"));
        assert!(!super::one_line(""));
        assert!(!super::one_line("two\nlines"));
        assert!(!super::one_line(&"x".repeat(201)));
        assert!(super::one_line(&"中".repeat(200)));
    }
}
