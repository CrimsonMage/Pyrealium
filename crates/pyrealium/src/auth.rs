//! Password accounts and short-lived native GLS tickets (`accounts`, `login_tickets`). No password enters UDP.
//! The account service issues tickets; the game server consumes them, each through its own
//! database connection.
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use crate::db::{self, error};
use rusqlite::OptionalExtension;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io,
    net::IpAddr,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub const TICKET_SECONDS: u64 = 60;
const MAX_ACCOUNTS: usize = 10_000;
const MAX_LIVE_TICKETS: i64 = 1024;
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
pub fn now() -> io::Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|t| t.as_secs())
        .map_err(|_| invalid("system clock precedes Unix epoch"))
}
pub fn valid_account(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}
pub fn random<const N: usize>() -> io::Result<[u8; N]> {
    let mut value = [0; N];
    getrandom::getrandom(&mut value)
        .map_err(|_| invalid("operating-system randomness unavailable"))?;
    Ok(value)
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
pub fn login_proof(login: &crate::wire::Login) -> String {
    let mut digest = Sha256::new();
    for bytes in [
        login.version.as_bytes(),
        login.account.as_bytes(),
        &login.crypto_data,
        &login.extra_data,
    ] {
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    digest.update(login.auth_type.to_le_bytes());
    digest.update(login.auth_flags.to_le_bytes());
    hex(&digest.finalize())
}
pub fn private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn password_hash(password: &str) -> io::Result<String> {
    let salt =
        SaltString::encode_b64(&random::<16>()?).map_err(|_| invalid("password salt encoding"))?;
    let params =
        argon2::Params::new(19456, 2, 1, Some(32)).map_err(|_| invalid("password parameters"))?;
    Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| invalid("password hashing failed"))
}
fn ticket_hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

pub struct Accounts {
    client: db::Connection,
    dummy_hash: String,
}
impl Accounts {
    /// Connects (creating and migrating the database when needed).
    pub fn open(database: &db::Database) -> io::Result<Self> {
        database.ensure_exists()?;
        let mut client = database.connect()?;
        db::migrate(&mut client)?;
        Ok(Self { client, dummy_hash: password_hash(&hex(&random::<32>()?))? })
    }
    pub fn create(&mut self, name: &str, password: &str) -> io::Result<()> {
        if !valid_account(name)
            || !(12..=128).contains(&password.chars().count())
            || password.len() > 512
        {
            return Err(invalid(
                "Use a lowercase account name of 1-32 letters, digits or underscores and a password of 12-128 characters.",
            ));
        }
        let count: i64 = self.client.query_row("SELECT count(*) FROM accounts", [], |r| r.get(0)).map_err(error)?;
        if count >= MAX_ACCOUNTS as i64 {
            return Err(invalid("Account registration is unavailable."));
        }
        let hash = password_hash(password)?;
        let inserted = self
            .client
            .execute(
                "INSERT INTO accounts (name, password_hash) VALUES (?1, ?2) ON CONFLICT (name) DO NOTHING",
                [name, hash.as_str()],
            )
            .map_err(error)?;
        if inserted == 0 {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "That account name is unavailable."));
        }
        Ok(())
    }
    pub fn contains(&mut self, name: &str) -> io::Result<bool> {
        Ok(self.stored_hash(name)?.is_some())
    }
    fn stored_hash(&mut self, name: &str) -> io::Result<Option<String>> {
        self.client
            .query_row("SELECT password_hash FROM accounts WHERE name = ?1", [name], |r| r.get(0))
            .optional()
            .map_err(error)
    }
    /// Whether characters are already saved under `account` (see `account_web`'s reservation).
    pub fn owns_characters(&mut self, account: &str) -> io::Result<bool> {
        db::shard::account_owns_characters(&self.client, account)
    }
    pub fn verify(&mut self, name: &str, password: &str) -> io::Result<bool> {
        if !valid_account(name) || password.len() > 512 {
            return Ok(false);
        }
        let stored = self.stored_hash(name)?;
        // Hash against a dummy for unknown names so timing does not reveal which exist.
        let parsed = PasswordHash::new(stored.as_deref().unwrap_or(&self.dummy_hash));
        let matched = parsed.is_ok_and(|hash| {
            Argon2::default()
                .verify_password(password.as_bytes(), &hash)
                .is_ok()
        });
        if stored.is_some() && matched {
            let _ = self.client.execute("UPDATE accounts SET last_login_at = unixepoch() WHERE name = ?1", [name]);
            return Ok(true);
        }
        Ok(false)
    }
    pub fn issue(&mut self, account: &str, ip: IpAddr) -> io::Result<String> {
        if !self.contains(account)? || !ip.is_ipv4() {
            return Err(invalid(
                "A registered account and IPv4 connection are required.",
            ));
        }
        self.client
            .execute("DELETE FROM login_tickets WHERE expires_at <= unixepoch() OR used_at IS NOT NULL", [])
            .map_err(error)?;
        let live: i64 = self.client.query_row("SELECT count(*) FROM login_tickets", [], |r| r.get(0)).map_err(error)?;
        if live >= MAX_LIVE_TICKETS {
            return Err(invalid("Login ticket capacity."));
        }
        let token = hex(&random::<32>()?);
        self.client
            .execute(
                "INSERT INTO login_tickets (token_hash, account, ip, expires_at)
                 VALUES (?1, ?2, ?3, unixepoch() + ?4)",
                rusqlite::params![ticket_hash(&token), account, ip.to_string(), TICKET_SECONDS as i64],
            )
            .map_err(error)?;
        Ok(token)
    }
}

/// Atomically consume a SHA256-indexed, single-use ticket. Never log its value.
pub fn consume(client: &mut db::Connection, account: &str, ip: IpAddr, token: &str) -> io::Result<()> {
    if !valid_account(account)
        || token.len() != 64
        || !token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("Public login rejected."));
    }
    let claimed = client
        .execute(
            "UPDATE login_tickets SET used_at = unixepoch()
             WHERE token_hash = ?1 AND used_at IS NULL AND account = ?2 AND ip = ?3
               AND expires_at > unixepoch() AND expires_at <= unixepoch() + ?4",
            rusqlite::params![ticket_hash(token), account, ip.to_string(), TICKET_SECONDS as i64],
        )
        .map_err(|_| invalid("Public login rejected."))?;
    if claimed != 1 {
        return Err(invalid("Public login rejected."));
    }
    Ok(())
}

/// Bounded rolling windows for unauthenticated login work.
pub struct RateLimit {
    entries: BTreeMap<IpAddr, (u64, u32)>,
    limit: u32,
}
impl RateLimit {
    pub fn new(limit: u32) -> Self {
        Self {
            entries: BTreeMap::new(),
            limit,
        }
    }
    pub fn allow(&mut self, ip: IpAddr, seconds: u64) -> bool {
        self.entries
            .retain(|_, (start, _)| seconds.saturating_sub(*start) < 60);
        if !self.entries.contains_key(&ip) && self.entries.len() >= 2048 {
            return false;
        }
        let (_, count) = self.entries.entry(ip).or_insert((seconds, 0));
        if *count >= self.limit {
            return false;
        }
        *count += 1;
        true
    }
}
