//! Shared native wire clock. Calendar length/offset belong to the client's DAT.
//! A world's durable origin keeps days advancing across restarts and downtime;
//! Instant keeps wall-clock adjustments from disturbing a running session.
use std::{fs::OpenOptions, io::{self, Write}, path::Path,
    time::{Instant, SystemTime, UNIX_EPOCH}};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Origin { version: u32, unix_seconds: f64 }

pub struct WorldClock { base: f64, started: Instant }

impl WorldClock {
    /// Called only after acquiring the world's Store lock.
    pub fn open(directory: &Path) -> io::Result<Self> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?.as_secs_f64();
        Self::open_at(directory, now, Instant::now())
    }

    fn open_at(directory: &Path, now: f64, started: Instant) -> io::Result<Self> {
        let path = directory.join("clock-origin.json");
        let origin = match std::fs::read(&path) {
            Ok(bytes) => {
                let value: Origin = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                if value.version != 1 || !value.unix_seconds.is_finite() || value.unix_seconds < 0.0 {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid world clock origin"));
                }
                value
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let value = Origin { version: 1, unix_seconds: now };
                let mut file = OpenOptions::new().create_new(true).write(true).open(&path)?;
                file.write_all(&serde_json::to_vec(&value).map_err(io::Error::other)?)?;
                file.sync_all()?;
                value
            }
            Err(error) => return Err(error),
        };
        Ok(Self { base: (now - origin.unix_seconds).max(0.0), started })
    }

    pub fn seconds(&self) -> f64 { self.at(Instant::now()) }
    fn at(&self, now: Instant) -> f64 {
        self.base + now.saturating_duration_since(self.started).as_secs_f64()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idle_restart_and_downtime_share_one_continuous_clock() {
        let dir = std::env::temp_dir().join(format!("ac2-clock-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = Instant::now();
        let first = WorldClock::open_at(&dir, 10_000.0, t).unwrap();
        assert_eq!(first.at(t + std::time::Duration::from_secs(7347)), 7347.0);
        let restarted = WorldClock::open_at(&dir, 18_000.0, t).unwrap();
        assert_eq!(restarted.at(t), 8000.0);
        assert_eq!(restarted.at(t + std::time::Duration::from_millis(250)), 8000.25);
        std::fs::remove_file(dir.join("clock-origin.json")).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
