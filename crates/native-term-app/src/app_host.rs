//! The main window's `App`, shared with its dialogs' windows.
//!
//! A dialog is a window of its own (see `window::open_dialog`): owned by
//! the window it belongs to, modal, as large as what it has, moved
//! anywhere. What it shows and what its answer does are still the
//! `App`'s (`show_dialog`, `settings_window`, `show_wizard`, which change
//! the tree, the configuration, the sessions): the dialog's window draws
//! that part of the `App` with its own context. All windows are drawn on
//! the event loop's thread, one after another, so the `App` is never
//! borrowed twice.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::app::App;

/// The parts of the `App` that are dialogs: each a window of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The settings (the rail's gear).
    Settings,
    /// The first-run guide.
    Wizard,
    /// The dialog open (`App::dialog`: a host, an import, a question).
    Dialog,
}

impl Part {
    pub const ALL: [Part; 3] = [Part::Settings, Part::Wizard, Part::Dialog];

    /// Its window's key (see `window::open`).
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Part::Settings => "dialog-settings",
            Part::Wizard => "dialog-wizard",
            Part::Dialog => "dialog",
        }
    }
}

/// The main window: the `App`.
pub struct AppHost(pub Rc<RefCell<App>>);

impl crate::window::Ui for AppHost {
    fn ui(&mut self, ui: &mut egui::Ui) {
        self.0.borrow_mut().ui(ui);
    }

    fn on_exit(&mut self) {
        self.0.borrow_mut().on_exit();
    }

    fn close_requested(&mut self) -> bool {
        self.0.borrow_mut().close_requested()
    }

    fn wants_close(&self) -> bool {
        self.0.borrow().wants_close()
    }
}

/// A dialog's window: `part` of the `App`, drawn as a window of its own
/// (the skin's dialogs take the whole window there).
pub struct DialogHost {
    pub app: Weak<RefCell<App>>,
    pub part: Part,
    /// The window system asked to close it: the dialog is given up.
    close: bool,
}

impl DialogHost {
    #[must_use]
    pub fn new(app: Weak<RefCell<App>>, part: Part) -> DialogHost {
        DialogHost { app, part, close: false }
    }
}

impl crate::window::Ui for DialogHost {
    fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if let Some(theme) = *crate::app::THEME.lock().unwrap_or_else(|e| e.into_inner()) {
            if ctx.options(|o| o.theme_preference) != theme {
                ctx.set_theme(theme);
            }
        }
        native_term_skin::as_window(&ctx);
        if std::mem::take(&mut self.close) {
            native_term_skin::close_asked(&ctx);
        }
        let Some(app) = self.app.upgrade() else { return };
        let Ok(mut app) = app.try_borrow_mut() else {
            ctx.request_repaint();
            return;
        };
        app.show_part(self.part, &ctx);
    }

    fn close_requested(&mut self) -> bool {
        // (given up through the dialog: its window closes when the `App`
        // is done with it)
        self.close = true;
        false
    }

    fn wants_close(&self) -> bool {
        let Some(app) = self.app.upgrade() else { return true };
        app.try_borrow().is_ok_and(|app| !app.part_open(self.part))
    }
}
