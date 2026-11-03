//! `pyrealium.toml`: everything a server needs to start (`pyrealium init` writes a commented
//! default). Command-line options override single values.
use serde::Deserialize;
use std::{
    io,
    net::SocketAddr,
    path::{Path, PathBuf},
};

pub const FILE_NAME: &str = "pyrealium.toml";

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Settings {
    pub server: Server,
    pub database: DatabaseSettings,
    pub accounts: Accounts,
    pub world: World,
    pub save: Save,
    pub gun: Gun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Auth {
    /// Loopback only: the client's account name is trusted, no password.
    Developer,
    /// Every login spends a one-use ticket from the account service.
    Tickets,
    /// Loopback password accounts plus no-ticket login for `server.account`.
    LocalAccounts,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Server {
    pub bind: SocketAddr,
    pub auth: Auth,
    /// Developer-login account (developer and local-accounts modes).
    pub account: String,
    /// Address the account service gives clients for the game server.
    pub game_host: String,
    /// The 7230 DAT directory (portal.dat, cell_1.dat, ...). None: character selection only.
    pub dat_dir: Option<PathBuf>,
    /// Holds `stop.request` and the admin token.
    pub run_dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DatabaseSettings {
    /// The SQLite database file (created on first start).
    pub path: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Accounts {
    /// Run the account web service inside the game server (tickets / local-accounts modes).
    pub enabled: bool,
    pub web_bind: SocketAddr,
    /// Public origin of the account page (https://..., or http://127.0.0.1:PORT locally).
    pub public_origin: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct World {
    /// The content packs (`content/`) the world is prepared from.
    pub content_dir: Option<PathBuf>,
    /// Keep the training kit's authored placements (`training-world.tsv`); off mirrors the
    /// captured Shelter into the human start instance instead.
    pub training_world: bool,
    /// Without the training placements, add Rescue the Shelter Hold's placements.
    pub shelter_quest: bool,
    /// Instead of the prepared world: a placement TSV path, or `observed`, `training`, `none`.
    pub content: Option<String>,
    pub encounters: Option<String>,
    pub content_overlay: Option<PathBuf>,
    pub world_rules: Option<PathBuf>,
    pub admin_bind: Option<SocketAddr>,
    pub initialize_ui: bool,
    pub training_kit: bool,
    pub lifestones: bool,
    pub portals: bool,
    /// Loopback testing extension: `@maptravel`.
    pub test_map_travel: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Save {
    pub interval_seconds: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Gun {
    /// Custom TCP/XML social service (not stock GunDll.dll); None: off.
    pub bind: Option<SocketAddr>,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:9000".parse().unwrap(),
            auth: Auth::Tickets,
            account: "local".into(),
            game_host: "127.0.0.1".into(),
            dat_dir: None,
            run_dir: "run".into(),
        }
    }
}
impl Default for DatabaseSettings {
    fn default() -> Self {
        Self { path: "pyrealium.db".into() }
    }
}
impl Default for Accounts {
    fn default() -> Self {
        Self { enabled: true, web_bind: "127.0.0.1:19400".parse().unwrap(), public_origin: None }
    }
}
impl Default for World {
    fn default() -> Self {
        Self {
            content_dir: None,
            training_world: false,
            shelter_quest: true,
            content: None,
            encounters: None,
            content_overlay: None,
            world_rules: None,
            admin_bind: None,
            initialize_ui: true,
            training_kit: true,
            lifestones: true,
            portals: true,
            test_map_travel: false,
        }
    }
}
impl Default for Save {
    fn default() -> Self {
        Self { interval_seconds: crate::store::SAVE_INTERVAL.as_secs() }
    }
}

impl Settings {
    /// Reads `path`; relative paths inside it resolve against the file's directory.
    pub fn load(path: &Path) -> io::Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| io::Error::new(e.kind(), format!("cannot read {}: {e}", path.display())))?;
        let mut settings: Self = toml::from_str(&text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display())))?;
        let base = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let anchor = |p: &mut PathBuf| {
            if p.is_relative() {
                *p = base.join(&*p);
            }
        };
        if let Some(p) = settings.server.dat_dir.as_mut() {
            anchor(p);
        }
        anchor(&mut settings.server.run_dir);
        anchor(&mut settings.database.path);
        if let Some(p) = settings.world.content_dir.as_mut() {
            anchor(p);
        }
        for p in [settings.world.content_overlay.as_mut(), settings.world.world_rules.as_mut()].into_iter().flatten() {
            anchor(p);
        }
        for value in [settings.world.content.as_mut(), settings.world.encounters.as_mut()].into_iter().flatten() {
            if !matches!(value.as_str(), "observed" | "training" | "none") && Path::new(value.as_str()).is_relative() {
                *value = base.join(value.as_str()).to_string_lossy().into_owned();
            }
        }
        if settings.save.interval_seconds == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "save.interval_seconds must be at least 1"));
        }
        Ok(settings)
    }
    /// `--config PATH`, else `pyrealium.toml` in the working directory, else beside the binary.
    pub fn locate(explicit: Option<&Path>) -> Option<PathBuf> {
        if let Some(path) = explicit {
            return Some(path.to_path_buf());
        }
        let here = PathBuf::from(FILE_NAME);
        if here.is_file() {
            return Some(here);
        }
        let beside = std::env::current_exe().ok()?.parent()?.join(FILE_NAME);
        beside.is_file().then_some(beside)
    }
    /// `world.content_dir`, else the content packs of the source tree this server was built from.
    pub fn content_dir(&self) -> PathBuf {
        self.world.content_dir.clone().unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content"))
    }
    pub fn public_origin(&self) -> String {
        self.accounts
            .public_origin
            .clone()
            .unwrap_or_else(|| format!("http://127.0.0.1:{}", self.accounts.web_bind.port()))
    }
}

/// The file `pyrealium init` writes.
pub const TEMPLATE: &str = r#"# Pyrealium server settings. Start the server with `pyrealium` (or `pyrealium run`).
# Paths are relative to this file.

[server]
# Game UDP address; the server also uses the next port.
bind = "127.0.0.1:9000"
# tickets: logins come from the account service (below). developer: loopback only, no
# passwords. local-accounts: loopback passwords plus no-ticket login for `account`.
auth = "tickets"
account = "local"
# Address the account service tells clients to connect to.
game_host = "127.0.0.1"
# The Asheron's Call 2 build 7230 DAT directory (portal.dat, cell_1.dat, ...).
dat_dir = "dataset-7230"
# Holds the stop.request signal file and the admin token.
run_dir = "run"

[database]
# The SQLite database file: accounts, characters and the prepared world. Created on first
# start. Back it up while the server runs with: sqlite3 pyrealium.db ".backup backup.db"
path = "pyrealium.db"

[accounts]
# Account registration and login web service, run inside the game server.
enabled = true
web_bind = "127.0.0.1:19400"
# Public address of the account page; https:// behind a TLS reverse proxy when hosting.
# public_origin = "https://accounts.example.com"

[world]
# The content packs the world is prepared from (prepared again when they or the DATs change,
# and stored in the database). Default: the source tree's content/.
# content_dir = "content"
# Keep the training kit's authored start-room placements (off: the captured retail Shelter).
training_world = false
# Rescue the Shelter Hold's placements (when training_world is off).
shelter_quest = true
# Serve a placement file instead of the prepared world: a TSV path, or observed / training / none.
# content = "world.tsv"
initialize_ui = true
training_kit = true
lifestones = true
portals = true
# Loopback testing extension: @maptravel LAND_X LAND_Y.
test_map_travel = false

[save]
# Seconds between saves of changed characters (also saved at logoff and shutdown).
interval_seconds = 300

[gun]
# Custom TCP/XML social service for a compatible adapter (not stock GunDll.dll).
# bind = "127.0.0.1:2310"
"#;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_template_parses_to_the_defaults() {
        let parsed: Settings = toml::from_str(TEMPLATE).unwrap();
        let defaults = Settings::default();
        assert_eq!(parsed.server.bind, defaults.server.bind);
        assert_eq!(parsed.server.auth, Auth::Tickets);
        assert_eq!(parsed.database.path, defaults.database.path);
        assert_eq!(parsed.save.interval_seconds, 300);
        assert!(parsed.accounts.enabled);
        assert!(toml::from_str::<Settings>("[server]\nbogus = 1\n").is_err());
    }
}
