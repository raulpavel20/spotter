//! Opt-in debug log for bug reports: `SPOTTER_LOG=<file>` appends one
//! line per git call, per error and per notable event. Off by default and
//! free when off.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

static LOG: OnceLock<Option<Mutex<File>>> = OnceLock::new();

/// Opens the file named by `SPOTTER_LOG`, if set. Returns its path.
pub fn init_from_env() -> Option<String> {
    let path = std::env::var("SPOTTER_LOG").ok().filter(|p| !p.is_empty());
    let file = path
        .as_ref()
        .and_then(|p| OpenOptions::new().create(true).append(true).open(p).ok());
    let opened = file.is_some();
    let _ = LOG.set(file.map(Mutex::new));
    path.filter(|_| opened)
}

pub fn enabled() -> bool {
    matches!(LOG.get(), Some(Some(_)))
}

/// Appends a line; the closure only runs when logging is on.
pub fn line(f: impl FnOnce() -> String) {
    let Some(Some(file)) = LOG.get() else {
        return;
    };
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("?").trim_start_matches("spotter-");
    if let Ok(mut f2) = file.lock() {
        let _ = writeln!(
            f2,
            "{}.{:03} [{name}] {}",
            now.as_secs(),
            now.subsec_millis(),
            f()
        );
    }
}
