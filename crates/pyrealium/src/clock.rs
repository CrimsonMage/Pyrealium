//! Shared native wire clock. Calendar length/offset belong to the client's DAT.
//! A world's durable origin keeps days advancing across restarts and downtime;
//! Instant keeps wall-clock adjustments from disturbing a running session.
use serde::{Deserialize, Serialize};
use std::{
    io,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

/// `world_state` key of the world's clock origin.
const KEY: &str = "clock_origin";

#[derive(Serialize, Deserialize)]
struct Origin {
    version: u32,
    unix_seconds: f64,
}

pub struct WorldClock {
    base: f64,
    started: Instant,
}

impl WorldClock {
    /// Reads the world's origin, establishing it on a new world.
    pub fn open(writer: &crate::db::Writer) -> io::Result<Self> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(io::Error::other)?.as_secs_f64();
        Self::open_at(writer, now, Instant::now())
    }

    fn open_at(writer: &crate::db::Writer, now: f64, started: Instant) -> io::Result<Self> {
        let fresh = serde_json::to_value(Origin { version: 1, unix_seconds: now }).map_err(io::Error::other)?;
        let value = writer.call(move |client| crate::db::shard::world_value_init(client, KEY, &fresh))?;
        let origin: Origin = serde_json::from_value(value).map_err(io::Error::other)?;
        if origin.version != 1 || !origin.unix_seconds.is_finite() || origin.unix_seconds < 0.0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "invalid world clock origin"));
        }
        Ok(Self { base: (now - origin.unix_seconds).max(0.0), started })
    }

    pub fn seconds(&self) -> f64 {
        self.at(Instant::now())
    }
    fn at(&self, now: Instant) -> f64 {
        self.base + now.saturating_duration_since(self.started).as_secs_f64()
    }
}

/// Now, in Unix seconds: the wall clock quest and profession times go by (durable records).
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idle_restart_and_downtime_share_one_continuous_clock() {
        let database = crate::db::testdb::fresh();
        let t = Instant::now();
        {
            let writer = crate::db::Writer::start(&database).unwrap();
            let first = WorldClock::open_at(&writer, 10_000.0, t).unwrap();
            assert_eq!(first.at(t + std::time::Duration::from_secs(7347)), 7347.0);
        }
        let writer = crate::db::Writer::start(&database).unwrap();
        let restarted = WorldClock::open_at(&writer, 18_000.0, t).unwrap();
        assert_eq!(restarted.at(t), 8000.0);
        assert_eq!(restarted.at(t + std::time::Duration::from_millis(250)), 8000.25);
    }
}
