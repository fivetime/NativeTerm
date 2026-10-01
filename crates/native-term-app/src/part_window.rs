//! A window's dialogs as windows of their own (see `window::open_dialog`).
//!
//! A window that has dialogs (the main window's `App`, the files window)
//! is shared (`Shared`: an `Rc<RefCell<…>>`) with its dialogs' windows.
//! Each dialog's window (`PartHost`) draws one part of it with its own
//! context, so what a dialog shows and what its answer does stay that
//! window's code; all windows are drawn on the event loop's thread, one
//! after another, so it is never borrowed twice. `sync` opens a part's
//! window when the part is there and closes it when it is gone.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::window::{Owner, Ui};

/// A window whose dialogs are windows of their own.
pub trait Parts: Ui + 'static {
    type Part: Copy + PartialEq + 'static;

    /// Draw `part` with its window's context.
    fn show_part(&mut self, part: Self::Part, ctx: &egui::Context);

    /// Whether `part` is there to be shown.
    fn part_open(&self, part: Self::Part) -> bool;
}

/// How a part's window is made.
pub struct PartWindow {
    /// Its key (see `window::open`): one window per key.
    pub key: &'static str,
    /// The window it belongs to.
    pub owner: Owner,
    /// Its owner takes no input while it is open.
    pub modal: bool,
    /// The person makes it larger or smaller (else it is as large as what
    /// it has).
    pub resizable: bool,
}

/// A window shared with its dialogs' windows.
pub struct Shared<T: Parts>(pub Rc<RefCell<T>>);

impl<T: Parts> Ui for Shared<T> {
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

/// A dialog's window: `part` of its owner, drawn as a window of its own
/// (the skin's dialogs take the whole window there).
pub struct PartHost<T: Parts> {
    owner: Weak<RefCell<T>>,
    part: T::Part,
    /// The window system asked to close it: the dialog is given up.
    close: bool,
}

impl<T: Parts> Ui for PartHost<T> {
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
        let Some(owner) = self.owner.upgrade() else { return };
        let Ok(mut owner) = owner.try_borrow_mut() else {
            ctx.request_repaint();
            return;
        };
        owner.show_part(self.part, &ctx);
    }

    fn close_requested(&mut self) -> bool {
        // (given up through the dialog: its window closes when its owner
        // is done with it)
        self.close = true;
        false
    }

    fn wants_close(&self) -> bool {
        let Some(owner) = self.owner.upgrade() else { return true };
        owner.try_borrow().is_ok_and(|owner| !owner.part_open(self.part))
    }
}

/// Open the window of each part that is there and has none yet (asked
/// for: in `asked` until it is open), close the window of each that is
/// gone. `me` is the window itself, shared; `parts` its parts and how
/// their windows are made.
pub fn sync<T: Parts>(me: &Weak<RefCell<T>>, this: &T, asked: &mut Vec<T::Part>, parts: &[(T::Part, PartWindow)]) {
    for (part, made) in parts {
        let (part, there) = (*part, this.part_open(*part));
        let open = crate::window::is_open(made.key);
        if open || !there {
            asked.retain(|p| *p != part);
        }
        if open && !there {
            crate::window::close(made.key);
        }
        if open || !there || asked.contains(&part) {
            continue;
        }
        let viewport = native_term_skin::undecorated(
            egui::ViewportBuilder::default()
                .with_title("NativeTerm")
                .with_inner_size([480.0, 360.0])
                .with_resizable(made.resizable),
        );
        let owner = me.clone();
        crate::window::open_dialog(made.key, viewport, made.owner.clone(), made.modal, move |_| {
            Box::new(PartHost { owner, part, close: false })
        });
        asked.push(part);
    }
}
