//! The host's color scheme in its tab (`NativeTermColorScheme`, or its
//! folder's), set with escape sequences before every connect: `wt
//! new-tab --colorScheme` had no effect in Terminal 1.26. Changed to none,
//! the Terminal's own colors come back at the next connect.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use native_term_config::appearance;
use native_term_config::tree::SessionTree;

use crate::plink;

/// This tab got colors from us, so "none" has something to undo.
static APPLIED: AtomicBool = AtomicBool::new(false);

pub fn apply(alias: &str) {
    let tree = SessionTree::load(&plink::ssh_dir());
    let Some((folder, host)) = tree.find(alias) else { return };
    let scheme = appearance::for_host(folder, host).color_scheme.and_then(|name| appearance::scheme(&name));
    if scheme.is_none() && !APPLIED.load(Ordering::Relaxed) {
        return;
    }
    APPLIED.store(scheme.is_some(), Ordering::Relaxed);
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(appearance::osc(scheme).as_bytes());
    let _ = out.flush();
}
