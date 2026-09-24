//! The operating system, as NativeTerm's program needs it: local time,
//! process identity, the shell and its folders, the desktop, saved
//! passwords, cloud-synced files, folder watching, global shortcuts,
//! services, font files, the desktop's look. One name per need; on Windows each is the
//! `native-term-win` helper it always was, elsewhere the same name over
//! libc and the desktop's own tools, or an honest "not here"
//! (`io::ErrorKind::Unsupported`, an empty list, `None`) that the program
//! shows as a missing feature rather than a broken one.
//!
//! What only Windows has (the registry, layered windows) is here under
//! `cfg(windows)` only; the program keeps those behind the same `cfg`.

pub mod appearance;
pub mod cloud;
pub mod credentials;
pub mod desktop;
pub mod dock;
pub mod fonts;
pub mod home;
pub mod host;
pub mod hotkey;
pub mod icons;
#[cfg(all(unix, not(target_os = "macos")))]
pub mod kwin;
pub mod layered;
pub mod picker;
pub mod process;
pub mod serial;
pub mod service;
pub mod shell;
pub mod ssh;
pub mod time;
pub mod watch;

#[cfg(windows)]
pub use native_term_win::registry;

/// The error a feature gives where the OS has nothing for it.
#[cfg(unix)]
pub(crate) fn unsupported(what: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Unsupported, format!("{what}: not available on this system"))
}
