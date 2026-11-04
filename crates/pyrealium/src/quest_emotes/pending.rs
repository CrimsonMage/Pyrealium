//! Sets waiting out an action's delay (`Outcome::resume`), by character, until the server's tick
//! carries them on (`Tables::resume`). Kept in memory only: a restart or a logout drops them, as
//! ACE drops its emote chains.
use std::sync::Mutex;

use super::Resume;

static PENDING: Mutex<Vec<(u64, u64, Resume)>> = Mutex::new(Vec::new());

fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Waits out `resume` for the character (from now).
pub fn defer(character: u64, resume: Resume) {
    let due = unix_ms().saturating_add(u64::from(resume.after_ms));
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).push((due, character, resume));
}
/// The outcome's resume, if it paused.
pub fn defer_outcome(character: u64, outcome: &super::Outcome) {
    if let Some(resume) = &outcome.resume { defer(character, resume.clone()); }
}
/// Every wait that has run out, oldest first.
pub fn take_due() -> Vec<(u64, Resume)> {
    let now = unix_ms();
    let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let mut due: Vec<(u64, u64, Resume)> = Vec::new();
    pending.retain(|entry| if entry.0 <= now { due.push(entry.clone()); false } else { true });
    due.sort_by_key(|entry| entry.0);
    due.into_iter().map(|(_, character, resume)| (character, resume)).collect()
}
/// Drops a character's waits (it left the world).
pub fn forget(character: u64) {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).retain(|entry| entry.1 != character);
}
