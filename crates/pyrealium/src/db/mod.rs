//! SQLite persistence. One database file (`[database] path`, default `pyrealium.db`) holds
//! accounts and login tickets, characters with their items, ground items, world state, GUN lists,
//! and the prepared content (`content_files`, `content_meta`). DAT files never enter the
//! database; they stay on disk and are read on demand.
//!
//! The file runs in WAL mode with `synchronous=FULL`: a committed save survives a crash or power
//! loss, and readers (tools, the account service) never block the writer. The game loop never
//! waits on SQL during play: `Writer` runs one connection on its own thread and executes jobs in
//! submission order. It also holds the server lock (an OS lock on `<database>.lock`), so a
//! second server on the same file refuses to start, and a crashed server's lock is released
//! with its process (no stale lock files).
use std::{
    fs::{self, File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::Duration,
};

pub use rusqlite::Connection;

pub mod shard;
pub mod testdb;
pub mod writer;

pub use writer::Writer;

/// Embedded migrations, applied in order; the applied version is the file's `user_version`.
/// Never edit a published one; add the next number.
const MIGRATIONS: &[(i32, &str, &str)] = &[
    (1, "initial", include_str!("../../migrations/0001_initial.sql")),
];

/// How long a connection waits for another connection's write before failing.
const BUSY_TIMEOUT: Duration = Duration::from_secs(10);

pub fn error(e: rusqlite::Error) -> io::Error {
    io::Error::other(format!("database: {e}"))
}
pub fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// The database file.
#[derive(Clone, Debug)]
pub struct Database {
    path: PathBuf,
}

impl Database {
    pub fn new(path: impl Into<PathBuf>) -> io::Result<Self> {
        let path = path.into();
        if path.as_os_str().is_empty() {
            return Err(invalid("the database path is empty"));
        }
        Ok(Self { path })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// The path, for messages.
    pub fn name(&self) -> String {
        self.path.display().to_string()
    }
    fn lock_path(&self) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(".lock");
        name.into()
    }
    /// Opens the existing database file.
    pub fn connect(&self) -> io::Result<Connection> {
        self.open(rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
    }
    fn open(&self, flags: rusqlite::OpenFlags) -> io::Result<Connection> {
        let flags = flags | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX | rusqlite::OpenFlags::SQLITE_OPEN_URI;
        let connection = Connection::open_with_flags(&self.path, flags)
            .map_err(|e| io::Error::other(format!("cannot open database {}: {e}", self.name())))?;
        connection.busy_timeout(BUSY_TIMEOUT).map_err(error)?;
        connection
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL; PRAGMA foreign_keys = ON;")
            .map_err(error)?;
        Ok(connection)
    }
    /// Whether the database file exists.
    pub fn exists(&self) -> io::Result<bool> {
        self.path.try_exists()
    }
    /// Creates the database file (and its directory) when it does not exist.
    pub fn ensure_exists(&self) -> io::Result<()> {
        if self.exists()? {
            return Ok(());
        }
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        self.open(rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_CREATE)?;
        eprintln!("Created database {}.", self.name());
        Ok(())
    }
    /// Deletes the database file with its WAL and lock files (no server may be using it).
    pub fn drop_database(&self) -> io::Result<()> {
        for suffix in ["", "-wal", "-shm", ".lock"] {
            let mut name = self.path.clone().into_os_string();
            name.push(suffix);
            match fs::remove_file(PathBuf::from(name)) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        Ok(())
    }
    /// Applies every embedded migration the database lacks. Refuses a database migrated by a
    /// newer server.
    pub fn migrate(&self) -> io::Result<i32> {
        let mut connection = self.connect()?;
        migrate(&mut connection)
    }
    /// Takes the server lock: `None` when another server holds it. Held until the file is dropped.
    pub fn try_server_lock(&self) -> io::Result<Option<File>> {
        let file = OpenOptions::new().create(true).truncate(false).write(true).open(self.lock_path())?;
        // `File::try_lock` is Rust 1.89. The crate already needs 1.88+ (let chains), so the
        // declared `rust-version` is stale; raising it is a separate change.
        #[allow(clippy::incompatible_msrv)]
        let locked = file.try_lock();
        match locked {
            Ok(()) => Ok(Some(file)),
            Err(fs::TryLockError::WouldBlock) => Ok(None),
            Err(fs::TryLockError::Error(e)) => Err(e),
        }
    }
    /// Whether a game server holds this database's server lock.
    pub fn server_running(&self) -> io::Result<bool> {
        if !self.exists()? {
            return Ok(false);
        }
        Ok(self.try_server_lock()?.is_none())
    }
}

pub fn migrate(connection: &mut Connection) -> io::Result<i32> {
    // IMMEDIATE takes the write lock up front, serialising concurrent migrators (the game server
    // and an account service starting together).
    let tx = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(error)?;
    let current: i32 = tx.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(error)?;
    let latest = MIGRATIONS.last().map_or(0, |m| m.0);
    if current > latest {
        return Err(invalid(format!(
            "database schema version {current} is newer than this server ({latest}); run a newer Pyrealium"
        )));
    }
    for (version, name, sql) in MIGRATIONS.iter().filter(|m| m.0 > current) {
        tx.execute_batch(sql).map_err(|e| io::Error::other(format!("migration {version} {name}: {e}")))?;
        tx.execute_batch(&format!("PRAGMA user_version = {version}")).map_err(error)?;
        eprintln!("Applied database migration {version:04} {name}.");
    }
    tx.commit().map_err(error)?;
    Ok(latest)
}

/// Schema version currently applied (0 when never migrated).
pub fn schema_version(connection: &Connection) -> io::Result<i32> {
    connection.query_row("PRAGMA user_version", [], |r| r.get(0)).map_err(error)
}

/// JSON values are stored as their text.
pub fn json_text(value: &serde_json::Value) -> io::Result<String> {
    serde_json::to_string(value).map_err(io::Error::other)
}
pub fn json_value(text: &str, what: &str) -> io::Result<serde_json::Value> {
    serde_json::from_str(text).map_err(|e| invalid(format!("saved {what} is not JSON: {e}")))
}

/// Runs `job` on a short-lived thread with its own connection and waits for it: for callers that
/// are not the game loop (CLI tools, the account service's startup).
pub fn blocking<T: Send + 'static>(
    database: &Database,
    job: impl FnOnce(&mut Connection) -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    let database = database.clone();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(database.connect().and_then(|mut connection| job(&mut connection)));
    });
    rx.recv().map_err(|_| io::Error::other("database thread ended"))?
}

/// u64 identities and DIDs are stored as their two's-complement i64 bit pattern; item identities
/// carry high namespace bits, so they read back negative in SQL. They are opaque keys.
pub fn to_sql(v: u64) -> i64 {
    v as i64
}
pub fn from_sql(v: i64) -> u64 {
    v as u64
}

/// Serde for maps whose keys are not strings or integers (tuples): stored as a JSON list of
/// `[key, value]` pairs. Use with `#[serde(with = "crate::db::pairs")]`.
pub mod pairs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::BTreeMap;
    pub fn serialize<K: Serialize, V: Serialize, S: Serializer>(map: &BTreeMap<K, V>, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(map.iter())
    }
    pub fn deserialize<'de, K, V, D>(d: D) -> Result<BTreeMap<K, V>, D::Error>
    where
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        let pairs = Vec::<(K, V)>::deserialize(d)?;
        let len = pairs.len();
        let map: BTreeMap<K, V> = pairs.into_iter().collect();
        if map.len() != len {
            return Err(serde::de::Error::custom("duplicate map key"));
        }
        Ok(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_server_holds_the_lock_until_it_stops() {
        let database = testdb::fresh();
        assert!(!database.server_running().unwrap());
        let writer = Writer::start(&database).unwrap();
        assert!(database.server_running().unwrap());
        let second = Writer::start(&database).err().expect("a second server must refuse to start");
        assert_eq!(second.kind(), io::ErrorKind::AddrInUse);
        drop(writer);
        assert!(!database.server_running().unwrap());
        Writer::start(&database).expect("the lock is free once the first server stops");
    }

    #[test]
    fn connections_use_wal_full_sync_and_foreign_keys() {
        let database = testdb::fresh();
        let c = database.connect().unwrap();
        let text = |pragma: &str| c.query_row(&format!("PRAGMA {pragma}"), [], |r| r.get::<_, String>(0)).unwrap();
        let int = |pragma: &str| c.query_row(&format!("PRAGMA {pragma}"), [], |r| r.get::<_, i64>(0)).unwrap();
        assert_eq!(text("journal_mode"), "wal");
        assert_eq!(int("synchronous"), 2, "FULL");
        assert_eq!(int("foreign_keys"), 1);
        assert_eq!(schema_version(&c).unwrap(), MIGRATIONS.last().unwrap().0);
    }

    #[test]
    fn migrate_is_idempotent_and_refuses_a_newer_database() {
        let database = testdb::fresh();
        assert_eq!(database.migrate().unwrap(), 1);
        assert_eq!(database.migrate().unwrap(), 1);
        database.connect().unwrap().execute_batch("PRAGMA user_version = 99").unwrap();
        assert!(database.migrate().unwrap_err().to_string().contains("newer than this server"));
    }

    #[test]
    fn missing_database_is_not_created_by_connect_and_drop_removes_its_files() {
        let database = testdb::fresh();
        let other = Database::new(database.path().with_file_name("absent.db")).unwrap();
        assert!(other.connect().is_err());
        assert!(!other.exists().unwrap());
        other.ensure_exists().unwrap();
        let _lock = other.try_server_lock().unwrap().unwrap();
        drop(_lock);
        other.drop_database().unwrap();
        for suffix in ["", "-wal", "-shm", ".lock"] {
            assert!(!PathBuf::from(format!("{}{suffix}", other.name())).exists(), "{suffix}");
        }
    }

    #[test]
    fn account_names_and_tickets_are_checked() {
        let database = testdb::fresh();
        let mut accounts = crate::auth::Accounts::open(&database).unwrap();
        accounts.create("hero_1", "correct horse battery").unwrap();
        assert!(accounts.create("hero_1", "correct horse battery").is_err(), "taken");
        assert!(accounts.verify("hero_1", "correct horse battery").unwrap());
        assert!(!accounts.verify("hero_1", "wrong horse battery!").unwrap());
        // The table itself refuses a name outside [a-z0-9_]{1,32}.
        let c = database.connect().unwrap();
        for bad in ["Hero", "a b", "", &"x".repeat(33)] {
            assert!(c.execute("INSERT INTO accounts (name, password_hash) VALUES (?1, 'h')", [bad]).is_err(), "{bad:?}");
        }
        let stored: String = c.query_row("SELECT password_hash FROM accounts WHERE name = 'hero_1'", [], |r| r.get(0)).unwrap();
        assert!(stored.starts_with("$argon2id$") && !stored.contains("correct horse"));
        let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();
        let token = accounts.issue("hero_1", ip).unwrap();
        let mut c = database.connect().unwrap();
        assert!(crate::auth::consume(&mut c, "hero_1", "127.0.0.2".parse().unwrap(), &token).is_err(), "other ip");
        assert!(crate::auth::consume(&mut c, "other", ip, &token).is_err(), "other account");
        crate::auth::consume(&mut c, "hero_1", ip, &token).unwrap();
        assert!(crate::auth::consume(&mut c, "hero_1", ip, &token).is_err(), "single use");
        let expired = accounts.issue("hero_1", ip).unwrap();
        c.execute("UPDATE login_tickets SET expires_at = unixepoch() - 1 WHERE used_at IS NULL", []).unwrap();
        assert!(crate::auth::consume(&mut c, "hero_1", ip, &expired).is_err(), "expired");
    }
}
