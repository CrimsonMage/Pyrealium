//! Test databases. `fresh()` returns a new, migrated, empty database for one test.
//!
//! With `PYREALIUM_TEST_DATABASE_URL` set (a role that may CREATE DATABASE), tests use that
//! server. Otherwise the first call starts a private throwaway cluster with the system's
//! `initdb`/`postgres` (fsync off, Unix socket only, trust auth) that a watchdog shell stops and
//! deletes when the test process exits. Each test database is cloned from a migrated template.
#![doc(hidden)]
use super::Database;
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

static ADMIN: OnceLock<String> = OnceLock::new();
static TEMPLATE: OnceLock<String> = OnceLock::new();
static CREATE: Mutex<()> = Mutex::new(());
static NEXT: AtomicU32 = AtomicU32::new(0);

fn bindir() -> Option<PathBuf> {
    let found = |dir: &Path| dir.join("initdb").is_file() && dir.join("postgres").is_file();
    if let Ok(out) = Command::new("pg_config").arg("--bindir").output()
        && out.status.success()
    {
        let dir = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
        if found(&dir) {
            return Some(dir);
        }
    }
    for dir in ["/usr/bin", "/usr/local/bin", "/opt/homebrew/bin"] {
        if found(Path::new(dir)) {
            return Some(dir.into());
        }
    }
    let mut versions: Vec<PathBuf> = std::fs::read_dir("/usr/lib/postgresql")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path().join("bin"))
        .filter(|d| found(d))
        .collect();
    versions.sort();
    versions.pop()
}

fn start_cluster() -> String {
    let bin = bindir().unwrap_or_else(|| {
        panic!(
            "tests need PostgreSQL: install it (initdb/postgres on PATH) or set \
             PYREALIUM_TEST_DATABASE_URL to a server where this role may CREATE DATABASE"
        )
    });
    // Unix socket paths are limited to ~107 bytes; keep the directory short.
    let base = std::env::temp_dir();
    let base = if base.as_os_str().len() < 60 { base } else { PathBuf::from("/tmp") };
    let dir = base.join(format!("pyrl-pg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let status = Command::new(bin.join("initdb"))
        .args(["-A", "trust", "-U", "postgres", "-E", "UTF8", "--no-sync", "-D"])
        .arg(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("run initdb");
    assert!(status.success(), "initdb failed for {}", dir.display());
    // The watchdog shell runs postgres and stops it (and removes the cluster) once this test
    // process has exited, so no cluster outlives `cargo test`.
    let script = r#"
        "$1" -D "$2" -k "$2" -c listen_addresses= -c fsync=off -c synchronous_commit=off \
            -c full_page_writes=off -c max_connections=200 >"$2/log" 2>&1 &
        pg=$!
        while kill -0 "$3" 2>/dev/null && kill -0 "$pg" 2>/dev/null; do sleep 1; done
        kill -INT "$pg" 2>/dev/null; wait "$pg"; rm -rf "$2"
    "#;
    // Deliberately not waited on: the watchdog outlives this process to clean up after it.
    #[allow(clippy::zombie_processes)]
    let _watchdog = Command::new("sh")
        .args(["-c", script, "sh"])
        .arg(bin.join("postgres"))
        .arg(&dir)
        .arg(std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start test postgres");
    let url = format!("host={} user=postgres dbname=postgres", dir.display());
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match url.parse::<postgres::Config>().unwrap().connect(postgres::NoTls) {
            Ok(_) => return url,
            Err(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => panic!("test postgres did not start ({e}); see {}/log", dir.display()),
        }
    }
}

fn admin() -> &'static str {
    ADMIN.get_or_init(|| std::env::var("PYREALIUM_TEST_DATABASE_URL").unwrap_or_else(|_| start_cluster()))
}

fn with_dbname(url: &str, name: &str) -> String {
    let mut config: postgres::Config = url.parse().expect("test database url");
    config.dbname(name);
    // Rebuild as key/value text; postgres::Config has no Display.
    let mut parts = Vec::new();
    for host in config.get_hosts() {
        match host {
            postgres::config::Host::Tcp(h) => parts.push(format!("host={h}")),
            #[cfg(unix)]
            postgres::config::Host::Unix(p) => parts.push(format!("host={}", p.display())),
        }
    }
    if let Some(port) = config.get_ports().first() {
        parts.push(format!("port={port}"));
    }
    if let Some(user) = config.get_user() {
        parts.push(format!("user={user}"));
    }
    if let Some(password) = config.get_password() {
        parts.push(format!("password='{}'", String::from_utf8_lossy(password).replace('\'', "\\'")));
    }
    parts.push(format!("dbname={name}"));
    parts.join(" ")
}

fn connect_admin() -> postgres::Client {
    admin().parse::<postgres::Config>().unwrap().connect(postgres::NoTls).expect("connect test postgres")
}

/// A new, migrated, empty database.
pub fn fresh() -> Database {
    let _guard = CREATE.lock().unwrap_or_else(|e| e.into_inner());
    let prefix = format!("pyrl_t{}", std::process::id());
    let template = TEMPLATE.get_or_init(|| {
        let name = format!("{prefix}_template");
        let mut server = connect_admin();
        server.batch_execute(&format!("DROP DATABASE IF EXISTS {name}")).unwrap();
        server.batch_execute(&format!("CREATE DATABASE {name}")).unwrap();
        drop(server);
        let database = Database::new(with_dbname(admin(), &name)).unwrap();
        database.migrate().expect("migrate test template");
        name
    });
    let name = format!("{prefix}_{}", NEXT.fetch_add(1, Ordering::Relaxed));
    let mut server = connect_admin();
    server.batch_execute(&format!("DROP DATABASE IF EXISTS {name}")).unwrap();
    server.batch_execute(&format!("CREATE DATABASE {name} TEMPLATE {template}")).unwrap();
    Database::new(with_dbname(admin(), &name)).unwrap()
}
