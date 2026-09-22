//! Global keyboard shortcuts, on a thread of their own. The owner gives
//! the whole set (`set`); the thread keeps whether each worked
//! (`results`), and a press calls `on_press` with its id there.

#[cfg(windows)]
pub use native_term_win::hotkey::{Hotkey, Hotkeys, Results};

#[cfg(unix)]
mod unix {
    use std::collections::HashMap;
    use std::io;

    /// One shortcut: an id of the owner's, the modifiers and the key, in
    /// the numbering the desktop uses.
    pub type Hotkey = (i32, u32, u32);

    /// What registering each id gave.
    pub type Results = HashMap<i32, Result<(), String>>;

    /// Global shortcuts aren't taken here yet: `start` says so.
    pub struct Hotkeys {
        _private: (),
    }

    impl Hotkeys {
        pub fn start(_on_press: impl Fn(i32) + Send + 'static) -> io::Result<Hotkeys> {
            Err(crate::unsupported("global shortcuts"))
        }

        pub fn set(&self, _keys: Vec<Hotkey>) {}

        pub fn results(&self) -> Results {
            Results::new()
        }
    }
}

#[cfg(unix)]
pub use unix::{Hotkey, Hotkeys, Results};
