//! The main window's dialogs, each a window of its own (see
//! `part_window`): the settings, the first-run guide, and the dialog open
//! (`App::dialog`: a host, an import, a question). A dialog opened from
//! the settings or the guide is theirs; the rest are the main window's.

use crate::app::App;
use crate::part_window::{PartWindow, Parts};
use crate::window::Owner;

/// The parts of the `App` that are dialogs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The settings (the rail's gear).
    Settings,
    /// The first-run guide.
    Wizard,
    /// The dialog open.
    Dialog,
    /// A session's log, beside the server's sessions (modeless, made
    /// larger or smaller; it belongs to their dialog).
    ServerLog,
}

impl Part {
    /// Its window's key (see `window::open`).
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Part::Settings => "dialog-settings",
            Part::Wizard => "dialog-wizard",
            Part::Dialog => "dialog",
            Part::ServerLog => "dialog-server-log",
        }
    }
}

impl Parts for App {
    type Part = Part;

    fn show_part(&mut self, part: Part, ctx: &egui::Context) {
        App::show_part(self, part, ctx);
    }

    fn part_open(&self, part: Part) -> bool {
        App::part_open(self, part)
    }
}

/// How the `App`'s parts' windows are made now: modal but for a log; the dialog
/// belongs to the settings or the guide where one of them is open.
#[must_use]
pub fn windows(settings: bool, wizard: bool) -> [(Part, PartWindow); 4] {
    let modal = |part: Part, owner: Owner| PartWindow { key: part.key(), owner, modal: true, resizable: false };
    let dialog_owner = if settings {
        Owner::Window(Part::Settings.key().into())
    } else if wizard {
        Owner::Window(Part::Wizard.key().into())
    } else {
        Owner::Main
    };
    [
        (Part::Settings, modal(Part::Settings, Owner::Main)),
        (Part::Wizard, modal(Part::Wizard, Owner::Main)),
        (Part::Dialog, modal(Part::Dialog, dialog_owner)),
        (
            Part::ServerLog,
            PartWindow {
                key: Part::ServerLog.key(),
                owner: Owner::Window(Part::Dialog.key().into()),
                modal: false,
                resizable: true,
            },
        ),
    ]
}
