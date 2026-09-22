//! The per-user channel between NativeTerm and its shims: a named pipe on
//! Windows, a Unix socket elsewhere, behind one small API.
//!
//! What every transport keeps to:
//! - one NativeTerm per user and sign-in: `pipe_name()` is theirs alone,
//!   and a second `bind` fails with `AddrInUse`;
//! - only that user's processes get through, and the server learns the
//!   client's process id (`client_pid`) to check what program it is;
//! - a connection is used by a reader and a writer thread at once, each
//!   message (one JSON line) arriving whole;
//! - `close` wakes a waiting `recv` (`ConnectionAborted`); a peer that
//!   went away reads as `UnexpectedEof`, but only after everything it
//!   wrote has been read — a client that writes and exits at once (the
//!   login helper) is still heard;
//! - a waiting reader costs no CPU.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::{connect, pipe_name, PipeConnection, PipeListener};
#[cfg(windows)]
pub use windows::{connect, pipe_name, PipeConnection, PipeListener};
