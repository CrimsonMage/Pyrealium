//! Which game data release the server runs on. Tables imported from the Legions DVD data have
//! counterparts imported from the installed 7230 data under `content/7230/`, used when the
//! server runs on the installed 7230 data (its fingerprint, see bootstrap.rs).
use std::sync::OnceLock;

static UNCONVERTED_7230: OnceLock<bool> = OnceLock::new();

/// Set once, from the bootstrap's data fingerprint, before any table is read.
pub fn set_unconverted_7230(active: bool) {
    let _ = UNCONVERTED_7230.set(active);
}

pub fn unconverted_7230() -> bool {
    UNCONVERTED_7230.get().copied().unwrap_or(false)
}

/// The table for the data release in use.
pub fn table(legions: &'static str, release_7230: &'static str) -> &'static str {
    if unconverted_7230() { release_7230 } else { legions }
}
