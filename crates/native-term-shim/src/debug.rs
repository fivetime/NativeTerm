//! Diagnostics: if `%TEMP%\nativeterm-shim-debug` exists, each shim appends
//! timestamped events to `%TEMP%\nativeterm-shim-<pid>.log`.

use std::io::Write;
use std::sync::OnceLock;
use std::time::Instant;

static STATE: OnceLock<Option<(Instant, std::path::PathBuf)>> = OnceLock::new();

pub fn log(event: impl AsRef<str>) {
    let state = STATE.get_or_init(|| {
        let temp = std::env::temp_dir();
        temp.join("nativeterm-shim-debug")
            .exists()
            .then(|| (Instant::now(), temp.join(format!("nativeterm-shim-{}.log", std::process::id()))))
    });
    if let Some((start, path)) = state {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{:>8.3} {}", start.elapsed().as_secs_f64(), event.as_ref());
        }
    }
}
