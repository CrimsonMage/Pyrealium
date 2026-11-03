//! Test databases. `fresh()` returns a new, migrated, empty database file for one test, deleted
//! (with its WAL and lock files) when the returned guard drops.
#![doc(hidden)]
use super::Database;
use std::{
    ops::Deref,
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
};

static NEXT: AtomicU32 = AtomicU32::new(0);

/// A test database; derefs to `Database`.
pub struct TestDatabase {
    database: Database,
    dir: PathBuf,
}

impl Deref for TestDatabase {
    type Target = Database;
    fn deref(&self) -> &Database {
        &self.database
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A new, migrated, empty database.
pub fn fresh() -> TestDatabase {
    let dir = std::env::temp_dir().join(format!(
        "pyrl-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create test database directory");
    let database = Database::new(dir.join("pyrealium.db")).unwrap();
    database.ensure_exists().expect("create test database");
    database.migrate().expect("migrate test database");
    TestDatabase { database, dir }
}
