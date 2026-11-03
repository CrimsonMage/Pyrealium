//! PostgreSQL persistence. One database, every table in its default schema: accounts and login
//! tickets, characters with their items, ground items, world state, GUN lists, and the prepared
//! content (`content_files`, `content_meta`). DAT files never enter the database; they stay on
//! disk and are read on demand.
//!
//! The game loop never waits on SQL during play: `Writer` runs one connection on its own thread
//! and executes jobs in submission order. It also holds the server's advisory lock, so a second
//! server on the same database refuses to start, and a crashed server's lock is released with its
//! connection (no stale lock files).
use std::{io, sync::mpsc, thread};

pub mod shard;
pub mod testdb;
pub mod writer;

pub use writer::Writer;

/// `pg_try_advisory_lock` key held by the running game server ("PYRL" as a big-endian word).
pub const SERVER_LOCK: i64 = 0x5059_524c;

/// Embedded migrations, applied in order. Never edit a published one; add the next number.
const MIGRATIONS: &[(i32, &str, &str)] = &[
    (1, "initial", include_str!("../../migrations/0001_initial.sql")),
    (2, "one_schema", include_str!("../../migrations/0002_one_schema.sql")),
];

pub fn error(e: postgres::Error) -> io::Error {
    match e.as_db_error() {
        Some(db) => io::Error::other(format!("database: {} ({})", db.message(), db.code().code())),
        None => io::Error::other(format!("database: {e}")),
    }
}
pub fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Connection settings: a libpq-style URL or key/value string
/// (`postgres://user:pass@host/pyrealium`, `host=/run/postgresql dbname=pyrealium`).
#[derive(Clone, Debug)]
pub struct Database {
    url: String,
}

impl Database {
    pub fn new(url: impl Into<String>) -> io::Result<Self> {
        let url = url.into();
        url.parse::<postgres::Config>().map_err(|e| invalid(format!("invalid database url: {e}")))?;
        Ok(Self { url })
    }
    pub fn url(&self) -> &str {
        &self.url
    }
    fn config(&self) -> postgres::Config {
        self.url.parse().expect("validated in Database::new")
    }
    pub fn name(&self) -> String {
        self.config().get_dbname().map_or_else(|| self.config().get_user().unwrap_or("postgres").to_owned(), str::to_owned)
    }
    pub fn connect(&self) -> io::Result<postgres::Client> {
        self.config().connect(postgres::NoTls).map_err(|e| {
            let detail = error(e);
            io::Error::other(format!("cannot connect to PostgreSQL database {:?}: {detail}", self.name()))
        })
    }
    /// Whether the database exists (its server must be reachable).
    pub fn exists(&self) -> io::Result<bool> {
        match self.config().connect(postgres::NoTls) {
            Ok(_) => Ok(true),
            Err(e) if e.as_db_error().is_some_and(|d| d.code() == &postgres::error::SqlState::INVALID_CATALOG_NAME) => Ok(false),
            Err(e) => Err(io::Error::other(format!("cannot connect to PostgreSQL database {:?}: {}", self.name(), error(e)))),
        }
    }
    /// Creates the database when it does not exist and the role may; otherwise reports the commands an administrator must run.
    pub fn ensure_exists(&self) -> io::Result<()> {
        let mut config = self.config();
        let name = self.name();
        match config.connect(postgres::NoTls) {
            Ok(_) => return Ok(()),
            Err(e) if e.as_db_error().is_some_and(|d| d.code() == &postgres::error::SqlState::INVALID_CATALOG_NAME) => {}
            Err(e) => return Err(io::Error::other(format!("cannot connect to PostgreSQL database {name:?}: {e}"))),
        }
        config.dbname("postgres");
        let mut admin = config.connect(postgres::NoTls).map_err(|e| {
            io::Error::other(format!("database {name:?} does not exist and the maintenance database is unreachable: {e}"))
        })?;
        // Identifiers cannot be bound as parameters; quote it.
        let quoted = format!("\"{}\"", name.replace('"', "\"\""));
        admin.batch_execute(&format!("CREATE DATABASE {quoted}")).map_err(|e| {
            io::Error::other(format!(
                "database {name:?} does not exist and could not be created ({e}). As a PostgreSQL superuser run:\n  \
                 createuser --pwprompt pyrealium\n  createdb --owner pyrealium {name}"
            ))
        })?;
        eprintln!("Created PostgreSQL database {name:?}.");
        Ok(())
    }
    /// Drops the database (it must not be in use by a running server).
    pub fn drop_database(&self) -> io::Result<()> {
        let name = self.name();
        if matches!(name.as_str(), "postgres" | "template0" | "template1") {
            return Err(invalid(format!("refusing to drop the {name:?} database; name the server's own database")));
        }
        let mut config = self.config();
        config.dbname("postgres");
        let mut admin = config.connect(postgres::NoTls).map_err(error)?;
        admin
            .batch_execute(&format!("DROP DATABASE IF EXISTS \"{}\" WITH (FORCE)", name.replace('"', "\"\"")))
            .map_err(error)
    }
    /// Applies every embedded migration the database lacks. Refuses a database migrated by a
    /// newer server.
    pub fn migrate(&self) -> io::Result<i32> {
        let mut client = self.connect()?;
        migrate(&mut client)
    }
}

pub fn migrate(client: &mut postgres::Client) -> io::Result<i32> {
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS public.pyrealium_migrations (
                version integer PRIMARY KEY, name text NOT NULL,
                applied_at timestamptz NOT NULL DEFAULT now())",
        )
        .map_err(error)?;
    // Serialise concurrent migrators (a game server and a split account service starting together).
    let mut tx = client.transaction().map_err(error)?;
    tx.batch_execute("LOCK TABLE public.pyrealium_migrations IN EXCLUSIVE MODE").map_err(error)?;
    let current: i32 = tx
        .query_one("SELECT coalesce(max(version), 0) FROM public.pyrealium_migrations", &[])
        .map_err(error)?
        .get(0);
    let latest = MIGRATIONS.last().map_or(0, |m| m.0);
    if current > latest {
        return Err(invalid(format!(
            "database schema version {current} is newer than this server ({latest}); run a newer Pyrealium"
        )));
    }
    for (version, name, sql) in MIGRATIONS.iter().filter(|m| m.0 > current) {
        tx.batch_execute(sql).map_err(|e| io::Error::other(format!("migration {version} {name}: {e}")))?;
        tx.execute("INSERT INTO public.pyrealium_migrations (version, name) VALUES ($1, $2)", &[version, name])
            .map_err(error)?;
        eprintln!("Applied database migration {version:04} {name}.");
    }
    tx.commit().map_err(error)?;
    Ok(latest)
}

/// Schema version currently applied (0 when never migrated).
pub fn schema_version(client: &mut postgres::Client) -> io::Result<i32> {
    let exists: bool = client
        .query_one("SELECT to_regclass('public.pyrealium_migrations') IS NOT NULL", &[])
        .map_err(error)?
        .get(0);
    if !exists {
        return Ok(0);
    }
    Ok(client
        .query_one("SELECT coalesce(max(version), 0) FROM public.pyrealium_migrations", &[])
        .map_err(error)?
        .get(0))
}

/// Runs `job` on a short-lived thread with its own connection and waits for it: for callers that
/// are not the game loop (CLI tools, the account service's startup).
pub fn blocking<T: Send + 'static>(
    database: &Database,
    job: impl FnOnce(&mut postgres::Client) -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    let database = database.clone();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(database.connect().and_then(|mut client| job(&mut client)));
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
