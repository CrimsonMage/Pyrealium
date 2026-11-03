//! Custom adapter service for GUN-like friends, ignore, presence and tells.
//!
//! The wire is `docs/gun-protocol.md`: TCP, a big-endian length and one XML element per frame.
//! Retail captures use UDP sequencing/acknowledgements and different XML fields
//! (`Cx`, `HR`, `User`, `FriendStatus`), plus Connect/ConInfo/ticket negotiation.
//! This endpoint is NOT compatible with stock GunDll.dll, even on port 2310.
//! A client needs a compatible transport adapter. The PR author's
//! external `gun-stub` and AC2_Rusted adapter are not included in this checkout.
//!
//! Identity is the ticket the client presents (`PresentTicketCmd`): in developer mode (loopback
//! only) the account name; with an account store the web backend's `account:token` ticket, spent
//! once. Friends and the ignore list persist per account in `gun_users`. Presence follows two sources: the GUN sessions themselves (`SetLoc` flags) and the
//! game server's characters in the world ([`publish_online`]).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

pub mod native;

#[cfg(test)]
mod regression_tests;

/// Largest frame payload either side sends.
pub const MAX_FRAME: usize = 65536;
/// `GUN_NAME_CCH` of the stub: names are at most 63 characters.
pub const MAX_NAME: usize = 63;
/// `GUN_MAX_LIST`: entries per list.
pub const MAX_LIST: usize = 256;
const MAX_TEXT: usize = 1024;
const MAX_SESSIONS: usize = 256;
const MAX_DEPTH: usize = 4;

pub const MODE_LOGGED_IN: u32 = 1;
pub const MODE_IN_GAME: u32 = 8;

pub const S_OK: u32 = 0;
pub const ZT_E_STR_TOO_LONG: u32 = 0xC112_0005;
pub const ZT_E_NOT_LOGGED_IN: u32 = 0xC112_0011;
pub const ZT_E_INVALID_USER: u32 = 0xC112_0012;
pub const ZT_E_INVALID_ARG: u32 = 0xC112_0013;
pub const ZT_E_INVALID_NAME_LENGTH: u32 = 0xC112_0019;
pub const ZT_E_TOO_MANY: u32 = 0xC112_001A;
pub const ZT_E_AUTH_INVALID_TICKET: u32 = 0xC112_0806;
pub const ZT_E_NOT_TICKETED: u32 = 0xC112_080B;
pub const ZT_E_ALREADY_TICKETED: u32 = 0xC112_080C;
pub const ZT_E_ALREADY_IN_LIST: u32 = 0xC112_1009;
pub const ZT_E_NOT_IN_LIST: u32 = 0xC112_1401;

// ------------------------------------------------------------------------------------- XML

/// One element of the wire: a name, attributes in order, and flat children.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Elem {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Elem>,
}

impl Elem {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            ..Self::default()
        }
    }
    pub fn attr(mut self, key: &str, value: impl Into<String>) -> Self {
        self.attrs.push((key.to_owned(), value.into()));
        self
    }
    pub fn child(mut self, child: Elem) -> Self {
        self.children.push(child);
        self
    }
    pub fn get(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
    pub fn encode(&self) -> String {
        let mut out = String::new();
        self.write(&mut out);
        out
    }
    fn write(&self, out: &mut String) {
        out.push('<');
        out.push_str(&self.name);
        for (key, value) in &self.attrs {
            out.push(' ');
            out.push_str(key);
            out.push_str("=\"");
            for c in value.chars() {
                match c {
                    '&' => out.push_str("&amp;"),
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    '"' => out.push_str("&quot;"),
                    c if (c as u32) < 0x20 => out.push_str(&format!("&#{};", c as u32)),
                    c => out.push(c),
                }
            }
            out.push('"');
        }
        if self.children.is_empty() {
            out.push_str("/>");
        } else {
            out.push('>');
            for child in &self.children {
                child.write(out);
            }
            out.push_str("</");
            out.push_str(&self.name);
            out.push('>');
        }
    }
    /// Parses one element (no declaration, comments or text between elements).
    pub fn parse(text: &str) -> Result<Elem, &'static str> {
        let mut parser = Parser {
            bytes: text.as_bytes(),
            at: 0,
        };
        parser.space();
        let elem = parser.element(0)?;
        parser.space();
        if parser.at != parser.bytes.len() {
            return Err("trailing data");
        }
        Ok(elem)
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn space(&mut self) {
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }
    fn name(&mut self) -> Result<String, &'static str> {
        let start = self.at;
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        {
            self.at += 1;
        }
        if self.at == start || self.at - start > 64 {
            return Err("bad name");
        }
        Ok(String::from_utf8_lossy(&self.bytes[start..self.at]).into_owned())
    }
    fn expect(&mut self, byte: u8) -> Result<(), &'static str> {
        if self.bytes.get(self.at) == Some(&byte) {
            self.at += 1;
            Ok(())
        } else {
            Err("unexpected byte")
        }
    }
    fn element(&mut self, depth: usize) -> Result<Elem, &'static str> {
        if depth > MAX_DEPTH {
            return Err("too deep");
        }
        self.expect(b'<')?;
        let mut elem = Elem::new(&self.name()?);
        loop {
            self.space();
            match self.bytes.get(self.at) {
                Some(b'/') => {
                    self.at += 1;
                    self.expect(b'>')?;
                    return Ok(elem);
                }
                Some(b'>') => {
                    self.at += 1;
                    break;
                }
                Some(_) => {
                    if elem.attrs.len() >= 16 {
                        return Err("too many attributes");
                    }
                    let key = self.name()?;
                    self.space();
                    self.expect(b'=')?;
                    self.space();
                    let value = self.value()?;
                    elem.attrs.push((key, value));
                }
                None => return Err("truncated"),
            }
        }
        loop {
            self.space();
            if self.bytes.get(self.at) == Some(&b'<') && self.bytes.get(self.at + 1) == Some(&b'/')
            {
                self.at += 2;
                let close = self.name()?;
                self.space();
                self.expect(b'>')?;
                return if close == elem.name {
                    Ok(elem)
                } else {
                    Err("mismatched close")
                };
            }
            if elem.children.len() >= MAX_LIST {
                return Err("too many children");
            }
            elem.children.push(self.element(depth + 1)?);
        }
    }
    fn value(&mut self) -> Result<String, &'static str> {
        let quote = *self.bytes.get(self.at).ok_or("truncated")?;
        if quote != b'"' && quote != b'\'' {
            return Err("unquoted value");
        }
        self.at += 1;
        let mut out = Vec::new();
        loop {
            let byte = *self.bytes.get(self.at).ok_or("truncated")?;
            self.at += 1;
            if byte == quote {
                break;
            }
            if byte == b'<' {
                return Err("bad value");
            }
            if byte != b'&' {
                out.push(byte);
                continue;
            }
            let end = self.bytes[self.at..]
                .iter()
                .position(|&b| b == b';')
                .ok_or("bad entity")?;
            let entity = &self.bytes[self.at..self.at + end];
            self.at += end + 1;
            let ch = match entity {
                b"amp" => '&',
                b"lt" => '<',
                b"gt" => '>',
                b"quot" => '"',
                b"apos" => '\'',
                [b'#', b'x', rest @ ..] => char::from_u32(
                    u32::from_str_radix(std::str::from_utf8(rest).map_err(|_| "bad entity")?, 16)
                        .map_err(|_| "bad entity")?,
                )
                .ok_or("bad entity")?,
                [b'#', rest @ ..] => char::from_u32(
                    std::str::from_utf8(rest)
                        .map_err(|_| "bad entity")?
                        .parse::<u32>()
                        .map_err(|_| "bad entity")?,
                )
                .ok_or("bad entity")?,
                _ => return Err("bad entity"),
            };
            let mut buffer = [0u8; 4];
            out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
        }
        String::from_utf8(out).map_err(|_| "not UTF-8")
    }
}

/// One frame: a big-endian `u32` payload length, then the payload.
pub fn frame(elem: &Elem) -> Vec<u8> {
    let text = elem.encode();
    let mut out = Vec::with_capacity(4 + text.len());
    out.extend_from_slice(&(text.len() as u32).to_be_bytes());
    out.extend_from_slice(text.as_bytes());
    out
}

/// `0x%08X`, the form every `hr` attribute takes.
pub fn hr_text(hr: u32) -> String {
    format!("0x{hr:08X}")
}

// ------------------------------------------------------------------------------- the state

/// How a ticket becomes an account.
#[derive(Debug, Clone)]
pub enum GunAuth {
    /// The ticket is the account name. Loopback binds only.
    Developer,
    /// The ticket is `account:token`: a token (`auth::Accounts::issue`) from the account
    /// service, spent once and bound to the connecting IP. The game login spends its own token,
    /// so a launcher issues one for each.
    Tickets,
}

/// Retail UDP port; retained for API compatibility, not an adapter bind default.
pub const DEFAULT_PORT: u16 = 2310;

#[derive(Debug, Clone)]
pub struct GunConfig {
    pub bind: SocketAddr,
    pub database: crate::db::Database,
    pub auth: GunAuth,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct User {
    friends: Vec<String>,
    ignored: Vec<String>,
}

struct Session {
    account: Option<String>,
    out: mpsc::SyncSender<Vec<u8>>,
    socket: TcpStream,
    flags: u32,
    location: String,
    words: [u32; 2],
}

#[derive(Default)]
struct State {
    users: BTreeMap<String, User>,
    sessions: BTreeMap<u64, Session>,
    /// Characters in the world: lower-case name to its account.
    characters: BTreeMap<String, String>,
    /// The mode last announced for each lower-case base name, to send changes only.
    announced: BTreeMap<String, (u32, String)>,
    next_session: u64,
}

pub struct Hub {
    state: Mutex<State>,
    /// The service's database connection (lists and tickets); None only in unit tests, where
    /// every save fails. Always taken after `state`, never before.
    db: Mutex<Option<postgres::Client>>,
    auth: GunAuth,
}

impl Hub {
    fn save_user(&self, account: &str, user: &User) -> io::Result<()> {
        let mut db = self.db.lock().unwrap_or_else(|poison| poison.into_inner());
        let client = db.as_mut().ok_or_else(|| io::Error::other("no GUN database"))?;
        let lists = serde_json::to_value(user)?;
        client
            .execute(
                "INSERT INTO gun_users (account, lists) VALUES ($1, $2)
                 ON CONFLICT (account) DO UPDATE SET lists = excluded.lists",
                &[&account, &lists],
            )
            .map_err(crate::db::error)?;
        Ok(())
    }
}

// Do not retain a stopped service forever: a later service must receive presence.
static HUB: OnceLock<Mutex<Weak<Hub>>> = OnceLock::new();

/// The part of `Name@World` before the `@`, lower case: what presence is keyed by.
pub fn base(name: &str) -> String {
    name.split('@').next().unwrap_or("").to_ascii_lowercase()
}

fn lock(hub: &Hub) -> std::sync::MutexGuard<'_, State> {
    hub.state.lock().unwrap_or_else(|poison| poison.into_inner())
}

impl State {
    /// Mode and location of a base name now: 0 when nobody is it.
    fn presence(&self, who: &str) -> (u32, String) {
        let owner = self.characters.get(who);
        let mut mode = 0;
        let mut location = String::new();
        for session in self.sessions.values() {
            let Some(account) = &session.account else {
                continue;
            };
            if account == who || owner == Some(account) {
                mode |= session.flags | MODE_LOGGED_IN;
                if location.is_empty() {
                    location = session.location.clone();
                }
            }
        }
        if owner.is_some() {
            mode |= MODE_LOGGED_IN | MODE_IN_GAME;
        }
        (mode, location)
    }

    fn deliver(&self, session: u64, elem: &Elem) {
        if let Some(target) = self.sessions.get(&session) {
            let bytes = frame(elem);
            // A slow reader must never hold the shared state (or game loop)
            // while writing to its socket. Disconnect on bounded-queue overflow.
            if bytes.len() > MAX_FRAME + 4 || target.out.try_send(bytes).is_err() {
                let _ = target.socket.shutdown(Shutdown::Both);
            }
        }
    }

    /// Tells every session that lists one of `names` as a friend about its new mode, when the
    /// mode differs from the last announcement.
    fn announce(&mut self, names: &BTreeSet<String>) {
        for who in names {
            let (mode, location) = self.presence(who);
            if self.announced.get(who) == Some(&(mode, location.clone())) {
                continue;
            }
            self.announced.insert(who.clone(), (mode, location.clone()));
            let mut sends = Vec::new();
            for (&id, session) in &self.sessions {
                let Some(user) = session.account.as_ref().and_then(|a| self.users.get(a)) else {
                    continue;
                };
                if let Some(friend) = user.friends.iter().find(|f| base(f) == *who) {
                    sends.push((
                        id,
                        Elem::new("FriendStat")
                            .attr("Name", friend.clone())
                            .attr("Mode", mode.to_string())
                            .attr("Location", location.clone()),
                    ));
                }
            }
            for (id, elem) in sends {
                self.deliver(id, &elem);
            }
        }
    }

    fn friends_elems(&self, account: &str) -> Vec<Elem> {
        self.users
            .get(account)
            .map(|user| {
                user.friends
                    .iter()
                    .map(|name| {
                        let (mode, location) = self.presence(&base(name));
                        Elem::new("Friend")
                            .attr("Name", name.clone())
                            .attr("Mode", mode.to_string())
                            .attr("Location", location)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

}

/// The game server's characters in the world, as `(character name, account)`: friends who are
/// in the world show as logged in and in game. Cheap when GUN is off.
pub fn publish_online(online: impl IntoIterator<Item = (String, String)>) {
    let Some(registry) = HUB.get() else { return };
    let Ok(active) = registry.try_lock() else { return };
    let Some(hub) = active.upgrade() else { return };
    drop(active);
    let now: BTreeMap<String, String> = online
        .into_iter()
        .map(|(name, account)| (name.to_ascii_lowercase(), account))
        .collect();
    // Called every 500 ms: retry next time if chat/persistence is busy.
    let Ok(mut state) = hub.state.try_lock() else { return };
    if state.characters == now {
        return;
    }
    let mut changed: BTreeSet<String> = state.characters.keys().cloned().collect();
    changed.extend(now.keys().cloned());
    state.characters = now;
    state.announce(&changed);
}

// --------------------------------------------------------------------------- the requests

fn reply(name: &str, seq: &str, hr: u32) -> Elem {
    Elem::new(name).attr("seq", seq).attr("hr", hr_text(hr))
}

fn check_name(name: Option<&str>) -> Result<&str, u32> {
    let name = name.ok_or(ZT_E_INVALID_ARG)?;
    if name.is_empty() || name.chars().any(|c| c.is_control()) {
        return Err(ZT_E_INVALID_ARG);
    }
    if name.chars().count() > MAX_NAME {
        return Err(ZT_E_INVALID_NAME_LENGTH);
    }
    Ok(name)
}

fn account_for_ticket(hub: &Hub, ticket: &str, ip: IpAddr) -> Result<String, u32> {
    match &hub.auth {
        GunAuth::Developer => {
            let account = ticket.to_ascii_lowercase();
            if crate::auth::valid_account(&account) {
                Ok(account)
            } else {
                Err(ZT_E_AUTH_INVALID_TICKET)
            }
        }
        GunAuth::Tickets => {
            let (account, token) = ticket.split_once(':').ok_or(ZT_E_AUTH_INVALID_TICKET)?;
            let account = account.to_ascii_lowercase();
            if !crate::auth::valid_account(&account) {
                return Err(ZT_E_AUTH_INVALID_TICKET);
            }
            let mut db = hub.db.lock().unwrap_or_else(|poison| poison.into_inner());
            let client = db.as_mut().ok_or(ZT_E_AUTH_INVALID_TICKET)?;
            crate::auth::consume(client, &account, ip, token)
                .map(|()| account)
                .map_err(|_| ZT_E_AUTH_INVALID_TICKET)
        }
    }
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || text.len() > 4096 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Handles one request from `session`; the answer (and any pushed events) go out on the
/// sockets. Returns false when the connection should close.
fn handle(hub: &Hub, session: u64, ip: IpAddr, message: &Elem) -> bool {
    let seq = message.get("seq").unwrap_or("0").to_owned();
    let mut state = lock(hub);
    let account = state
        .sessions
        .get(&session)
        .and_then(|s| s.account.clone());
    let answer = |state: &State, elem: Elem| state.deliver(session, &elem);
    match message.name.as_str() {
        "Connect" => {
            let random = crate::auth::random::<16>().map(|r| crate::auth::hex(&r));
            let elem = Elem::new("ConnectRes")
                .attr("hr", hr_text(S_OK))
                .attr("Random", random.unwrap_or_default());
            answer(&state, elem);
        }
        "PresentTicketCmd" => {
            if account.is_some() {
                answer(&state, reply("PresentTicketRes", &seq, ZT_E_ALREADY_TICKETED));
                return true;
            }
            let ticket = message
                .get("Ticket")
                .and_then(unhex)
                .and_then(|bytes| String::from_utf8(bytes).ok());
            let verdict = match ticket {
                Some(ticket) => {
                    drop(state);
                    let verdict = account_for_ticket(hub, &ticket, ip);
                    state = lock(hub);
                    verdict
                }
                None => Err(ZT_E_AUTH_INVALID_TICKET),
            };
            match verdict {
                Ok(name) => {
                    if let Some(entry) = state.sessions.get_mut(&session) {
                        entry.account = Some(name.clone());
                        entry.flags = 0;
                    }
                    state.users.entry(name.clone()).or_default();
                    let elem = reply("PresentTicketRes", &seq, S_OK)
                        .attr("Status", "0")
                        .attr("Account", name.clone());
                    answer(&state, elem);
                    let friends = state.friends_elems(&name);
                    if !friends.is_empty() {
                        let mut start = Elem::new("FStart");
                        start.children = friends;
                        answer(&state, start);
                    }
                    let changed = BTreeSet::from([name]);
                    state.announce(&changed);
                }
                Err(hr) => {
                    answer(&state, reply("PresentTicketRes", &seq, hr).attr("Status", "0"));
                }
            }
        }
        "ReleaseTicketCmd" => {
            if let Some(entry) = state.sessions.get_mut(&session) {
                entry.account = None;
                entry.flags = 0;
                entry.location.clear();
            }
            answer(&state, reply("ReleaseTicketRes", &seq, S_OK));
            if let Some(account) = account {
                let mut names = BTreeSet::from([account.clone()]);
                names.extend(state.characters.iter().filter(|(_, owner)| **owner == account)
                    .map(|(name, _)| name.clone()));
                state.announce(&names);
            }
        }
        _ => {
            let Some(account) = account else {
                let name = format!("{}Res", message.name);
                answer(&state, reply(&name, &seq, ZT_E_NOT_TICKETED));
                return true;
            };
            request(hub, &mut state, session, &account, &seq, message);
        }
    }
    true
}

fn request(hub: &Hub, state: &mut State, session: u64, account: &str, seq: &str, message: &Elem) {
    let name = message.name.as_str();
    let res = format!("{name}Res");
    match name {
        "GetFriendsList" => {
            let mut elem = reply(&res, seq, S_OK);
            elem.children = state.friends_elems(account);
            state.deliver(session, &elem);
        }
        "GetIgnoredList" => {
            let mut elem = reply(&res, seq, S_OK);
            if let Some(user) = state.users.get(account) {
                elem.children = user
                    .ignored
                    .iter()
                    .map(|n| Elem::new("Ignored").attr("Name", n.clone()))
                    .collect();
            }
            state.deliver(session, &elem);
        }
        "FriendAdd" | "FriendDelete" | "IgnoredAdd" | "IgnoredDelete" => {
            let hr = match check_name(message.get("Name")) {
                Err(hr) => hr,
                Ok(who) => {
                    let who = who.to_owned();
                    let user = state.users.entry(account.to_owned()).or_default();
                    let previous = user.clone();
                    let list = if name.starts_with("Friend") {
                        &mut user.friends
                    } else {
                        &mut user.ignored
                    };
                    let at = list.iter().position(|n| n.eq_ignore_ascii_case(&who));
                    let hr = if name.ends_with("Add") {
                        if at.is_some() {
                            ZT_E_ALREADY_IN_LIST
                        } else if list.len() >= MAX_LIST {
                            ZT_E_TOO_MANY
                        } else {
                            list.push(who.clone());
                            S_OK
                        }
                    } else if let Some(at) = at {
                        list.remove(at);
                        S_OK
                    } else {
                        ZT_E_NOT_IN_LIST
                    };
                    if hr == S_OK && hub.save_user(account, &state.users[account]).is_err() {
                        state.users.insert(account.to_owned(), previous);
                        // E_FAIL: do not acknowledge a list edit that was not saved.
                        0x80004005
                    } else { hr }
                }
            };
            state.deliver(session, &reply(&res, seq, hr));
            if hr == S_OK && name == "FriendAdd" {
                // The new friend's current presence, as a status event.
                if let Some(who) = message.get("Name") {
                    let key = base(who);
                    let (mode, location) = state.presence(&key);
                    let status = Elem::new("FriendStat")
                        .attr("Name", who)
                        .attr("Mode", mode.to_string())
                        .attr("Location", location);
                    state.deliver(session, &status);
                }
            }
        }
        "Flags" => {
            let op: u32 = message.get("Op").and_then(|v| v.parse().ok()).unwrap_or(9);
            let which: usize = message
                .get("Which")
                .and_then(|v| v.parse().ok())
                .unwrap_or(9);
            let bits: u32 = message.get("Flags").and_then(|v| v.parse().ok()).unwrap_or(0);
            let (hr, which_word, value) = match (state.sessions.get_mut(&session), op, which) {
                (Some(entry), 0..=2, 0..=1) => {
                    let word = &mut entry.words[which];
                    match op {
                        1 => *word |= bits,
                        2 => *word &= !bits,
                        _ => {}
                    }
                    (S_OK, which as u32, *word)
                }
                _ => (ZT_E_INVALID_ARG, 0, 0),
            };
            let elem = reply(&res, seq, hr)
                .attr("WhichFlag", which_word.to_string())
                .attr("Flags", value.to_string());
            state.deliver(session, &elem);
        }
        "SetLoc" => {
            let flags: u32 = message.get("Flags").and_then(|v| v.parse().ok()).unwrap_or(0);
            let location = message.get("Location").unwrap_or("");
            let hr = if location.chars().count() > MAX_NAME {
                ZT_E_INVALID_NAME_LENGTH
            } else {
                if let Some(entry) = state.sessions.get_mut(&session) {
                    entry.flags = flags;
                    entry.location = location.to_owned();
                }
                S_OK
            };
            state.deliver(session, &reply(&res, seq, hr));
            let mut names = BTreeSet::from([account.to_owned()]);
            names.extend(
                state
                    .characters
                    .iter()
                    .filter(|(_, owner)| owner.as_str() == account)
                    .map(|(character, _)| character.clone()),
            );
            state.announce(&names);
        }
        "SendToUserEx" => {
            let text = message.get("Text").unwrap_or("");
            let hr = match check_name(message.get("Name")) {
                Err(hr) => hr,
                Ok(_) if text.is_empty() || text.chars().count() > MAX_TEXT => ZT_E_STR_TOO_LONG,
                Ok(target) => {
                    let key = base(target);
                    let owner = state.characters.get(&key).cloned();
                    let recipients: Vec<u64> = state
                        .sessions
                        .iter()
                        .filter(|(_, s)| {
                            s.account
                                .as_deref()
                                .is_some_and(|a| a == key || owner.as_deref() == Some(a))
                        })
                        .map(|(&id, _)| id)
                        .collect();
                    if recipients.is_empty() {
                        ZT_E_INVALID_USER
                    } else {
                        let ignored = state.users.get(owner.as_deref().unwrap_or(&key))
                            .is_some_and(|u| u.ignored.iter().any(|n| {
                                let ignored_name = base(n);
                                ignored_name == account || state.characters.get(&ignored_name)
                                    .is_some_and(|owner| owner == account)
                            }));
                        // The sender is named by its character in the world, else its account.
                        let sender = state
                            .characters
                            .iter()
                            .find(|(_, a)| a.as_str() == account)
                            .map_or(account.to_owned(), |(c, _)| c.clone());
                        let world = target.split_once('@').map(|(_, w)| format!("@{w}"));
                        let from = format!("{sender}{}", world.unwrap_or_default());
                        if !ignored {
                            let event = Elem::new("UTellEv")
                                .attr("From", from)
                                .attr("Text", text)
                                .attr("Flags", message.get("Flags").unwrap_or("0"));
                            for id in recipients {
                                state.deliver(id, &event);
                            }
                        }
                        S_OK
                    }
                }
            };
            state.deliver(session, &reply(&res, seq, hr));
        }
        _ => {
            state.deliver(session, &reply(&res, seq, ZT_E_INVALID_ARG));
        }
    }
}

// ------------------------------------------------------------------------- the connection

fn read_frame(stream: &mut TcpStream, buffer: &mut Vec<u8>, stop: &AtomicBool) -> io::Result<Option<Vec<u8>>> {
    read_frame_with_deadline(stream, buffer, stop, Duration::from_secs(10))
}

fn read_frame_with_deadline(stream: &mut TcpStream, buffer: &mut Vec<u8>, stop: &AtomicBool,
                           deadline: Duration) -> io::Result<Option<Vec<u8>>> {
    let started = Instant::now();
    loop {
        if !buffer.is_empty() && started.elapsed() > deadline {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "incomplete frame deadline"));
        }
        if buffer.len() >= 4 {
            let length = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
            if length == 0 || length > MAX_FRAME {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "bad frame length"));
            }
            if buffer.len() >= 4 + length {
                let payload = buffer[4..4 + length].to_vec();
                buffer.drain(..4 + length);
                return Ok(Some(payload));
            }
        }
        if stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(None),
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                // An idle connection that never presented a ticket is dropped by the caller;
                // here only a half-sent frame is bounded.
                if !buffer.is_empty() && started.elapsed() > Duration::from_secs(10) {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "half a frame"));
                }
                if buffer.is_empty() {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        }
    }
}

fn serve(hub: Arc<Hub>, mut stream: TcpStream, peer: SocketAddr, stop: Arc<AtomicBool>) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let Ok(mut writer) = stream.try_clone() else { return };
    let Ok(socket) = stream.try_clone() else { return };
    let (out, outgoing) = mpsc::sync_channel::<Vec<u8>>(64);
    let id = {
        let mut state = lock(&hub);
        if state.sessions.len() >= MAX_SESSIONS {
            return;
        }
        state.next_session += 1;
        let id = state.next_session;
        state.sessions.insert(
            id,
            Session {
                account: None,
                out,
                socket,
                flags: 0,
                location: String::new(),
                words: [0; 2],
            },
        );
        id
    };
    let sender_stop = stop.clone();
    let sender = std::thread::spawn(move || {
        while !sender_stop.load(Ordering::Relaxed) {
            match outgoing.recv_timeout(Duration::from_millis(250)) {
                Ok(bytes) => if writer.write_all(&bytes).is_err() { break; },
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            }
        }
        let _ = writer.shutdown(Shutdown::Both);
    });
    let opened = Instant::now();
    let mut buffer = Vec::new();
    loop {
        if opened.elapsed() > Duration::from_secs(30)
            && !lock(&hub).sessions.get(&id).is_some_and(|s| s.account.is_some()) { break; }
        match read_frame(&mut stream, &mut buffer, &stop) {
            Ok(Some(payload)) => {
                let Ok(text) = std::str::from_utf8(&payload) else {
                    break;
                };
                let Ok(message) = Elem::parse(text) else {
                    break;
                };
                if !handle(&hub, id, peer.ip(), &message) {
                    break;
                }
            }
            Ok(None) => break,
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                let authed = lock(&hub).sessions.get(&id).is_some_and(|s| s.account.is_some());
                if !authed && opened.elapsed() > Duration::from_secs(30) {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let mut state = lock(&hub);
    let gone = state.sessions.remove(&id);
    if let Some(account) = gone.and_then(|s| s.account) {
        let mut names = BTreeSet::from([account.clone()]);
        names.extend(
            state
                .characters
                .iter()
                .filter(|(_, owner)| **owner == account)
                .map(|(character, _)| character.clone()),
        );
        state.announce(&names);
    }
    drop(state);
    let _ = stream.shutdown(Shutdown::Both);
    let _ = sender.join();
}

/// A running service; dropping it (or [`GunHandle::stop`]) closes the listener and the sessions.
pub struct GunHandle {
    pub addr: SocketAddr,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl GunHandle {
    pub fn stop(mut self) {
        self.shutdown();
    }
    fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for GunHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Starts the service on its own threads. Developer mode requires a loopback bind.
pub fn start(config: GunConfig) -> io::Result<GunHandle> {
    if matches!(config.auth, GunAuth::Developer) && !config.bind.ip().is_loopback() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "GUN developer authentication requires a loopback bind",
        ));
    }
    config.database.ensure_exists()?;
    let mut client = config.database.connect()?;
    crate::db::migrate(&mut client)?;
    let mut users = BTreeMap::new();
    for row in client.query("SELECT account, lists FROM gun_users", &[]).map_err(crate::db::error)? {
        let user: User = serde_json::from_value(row.get(1)).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        users.insert(row.get::<_, String>(0), user);
    }
    let listener = TcpListener::bind(config.bind)?;
    listener.set_nonblocking(true)?;
    let addr = listener.local_addr()?;
    let hub = Arc::new(Hub {
        state: Mutex::new(State {
            users,
            ..State::default()
        }),
        db: Mutex::new(Some(client)),
        auth: config.auth,
    });
    // Preserve the active service, but allow a stopped one to be replaced.
    let registry = HUB.get_or_init(|| Mutex::new(Weak::new()));
    let mut active = registry.lock().unwrap_or_else(|poison| poison.into_inner());
    if active.upgrade().is_none() {
        *active = Arc::downgrade(&hub);
    }
    drop(active);
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    let join = std::thread::spawn(move || {
        let mut workers = Vec::new();
        while !flag.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((stream, peer)) => {
                    workers.retain(|w: &std::thread::JoinHandle<()>| !w.is_finished());
                    if workers.len() >= MAX_SESSIONS { continue; }
                    let _ = stream.set_nonblocking(false);
                    let hub = Arc::clone(&hub);
                    let flag = Arc::clone(&flag);
                    workers.push(std::thread::spawn(move || serve(hub, stream, peer, flag)));
                    workers.retain(|w: &std::thread::JoinHandle<()>| !w.is_finished());
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(100)),
            }
        }
        for worker in workers {
            let _ = worker.join();
        }
    });
    Ok(GunHandle {
        addr,
        stop,
        join: Some(join),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_exact() {
        let elem = Elem::new("FriendAdd").attr("seq", "3").attr("Name", "Bob & \"Al\"@Pyrealium");
        let bytes = frame(&elem);
        let text = r#"<FriendAdd seq="3" Name="Bob &amp; &quot;Al&quot;@Pyrealium"/>"#;
        assert_eq!(&bytes[..4], &(text.len() as u32).to_be_bytes());
        assert_eq!(&bytes[4..], text.as_bytes());
        assert_eq!(Elem::parse(text).unwrap(), elem);
    }

    #[test]
    fn children_and_entities_round_trip() {
        let mut list = Elem::new("GetFriendsListRes").attr("seq", "1").attr("hr", "0x00000000");
        list.children.push(Elem::new("Friend").attr("Name", "a\nb<c>").attr("Mode", "9"));
        let text = list.encode();
        assert!(text.contains("a&#10;b&lt;c&gt;"));
        assert_eq!(Elem::parse(&text).unwrap(), list);
    }

    #[test]
    fn malformed_input_is_refused() {
        for bad in ["", "<A", "<A b=1/>", "<A></B>", "<A/><B/>", "<A b=\"&bogus;\"/>", "text"] {
            assert!(Elem::parse(bad).is_err(), "{bad}");
        }
        let deep = "<A><B><C><D><E><F/></E></D></C></B></A>";
        assert!(Elem::parse(deep).is_err());
    }

    struct Client {
        stream: TcpStream,
        buffer: Vec<u8>,
        stop: AtomicBool,
        seq: u32,
    }

    impl Client {
        fn connect(addr: SocketAddr) -> Self {
            let stream = TcpStream::connect(addr).unwrap();
            stream.set_read_timeout(Some(Duration::from_millis(250))).unwrap();
            Self { stream, buffer: Vec::new(), stop: AtomicBool::new(false), seq: 0 }
        }
        fn send(&mut self, elem: Elem) {
            self.stream.write_all(&frame(&elem)).unwrap();
        }
        fn next(&mut self) -> Elem {
            let started = Instant::now();
            loop {
                match read_frame(&mut self.stream, &mut self.buffer, &self.stop) {
                    Ok(Some(payload)) => return Elem::parse(std::str::from_utf8(&payload).unwrap()).unwrap(),
                    Ok(None) => panic!("closed"),
                    Err(_) if started.elapsed() < Duration::from_secs(5) => {}
                    Err(e) => panic!("{e}"),
                }
            }
        }
        fn ask(&mut self, elem: Elem) -> Elem {
            self.seq += 1;
            self.send(elem.attr("seq", self.seq.to_string()));
            self.next()
        }
        fn login(&mut self, ticket: &str) -> Elem {
            self.send(Elem::new("Connect").attr("ProtoVer", "1"));
            let connect = self.next();
            assert_eq!(connect.name, "ConnectRes");
            assert_eq!(connect.get("hr"), Some("0x00000000"));
            assert_eq!(connect.get("Random").map(str::len), Some(32));
            self.ask(Elem::new("PresentTicketCmd").attr("Ticket", crate::auth::hex(ticket.as_bytes())))
        }
    }

    fn service(database: &crate::db::Database) -> GunHandle {
        start(GunConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            database: database.clone(),
            auth: GunAuth::Developer,
        })
        .unwrap()
    }

    #[test]
    fn ticket_friends_ignore_flags_presence_and_tells() {
        let dir = crate::db::testdb::fresh();
        let service = service(&dir);

        let mut alice = Client::connect(service.addr);
        let res = alice.login("Alice");
        assert_eq!((res.name.as_str(), res.get("hr")), ("PresentTicketRes", Some("0x00000000")));
        assert_eq!(res.get("Account"), Some("alice"));

        // Requests before the ticket and a bad ticket are refused.
        let mut stranger = Client::connect(service.addr);
        let denied = stranger.ask(Elem::new("GetFriendsList"));
        assert_eq!(denied.get("hr"), Some("0xC112080B"));
        let bad = stranger.login("Not A Name!");
        assert_eq!(bad.get("hr"), Some("0xC1120806"));

        // A friend who is not online, then added twice and deleted.
        let add = alice.ask(Elem::new("FriendAdd").attr("Name", "Bob@Pyrealium"));
        assert_eq!(add.get("hr"), Some("0x00000000"));
        let status = alice.next();
        assert_eq!((status.name.as_str(), status.get("Mode")), ("FriendStat", Some("0")));
        assert_eq!(alice.ask(Elem::new("FriendAdd").attr("Name", "bob@pyrealium")).get("hr"), Some("0xC1121009"));
        let list = alice.ask(Elem::new("GetFriendsList"));
        assert_eq!(list.children.len(), 1);
        assert_eq!(list.children[0].get("Name"), Some("Bob@Pyrealium"));
        assert_eq!(list.children[0].get("Mode"), Some("0"));

        // Bob logs in: Alice is told, and Bob's own FStart is empty (not sent).
        let mut bob = Client::connect(service.addr);
        assert_eq!(bob.login("bob").get("hr"), Some("0x00000000"));
        let online = alice.next();
        assert_eq!(
            (online.name.as_str(), online.get("Name"), online.get("Mode")),
            ("FriendStat", Some("Bob@Pyrealium"), Some("1"))
        );

        // Bob goes away: flags 2.
        let set = bob.ask(Elem::new("SetLoc").attr("Flags", "2").attr("Location", "Holtburg"));
        assert_eq!(set.get("hr"), Some("0x00000000"));
        let away = alice.next();
        assert_eq!((away.get("Mode"), away.get("Location")), (Some("3"), Some("Holtburg")));

        // Flags words: set, clear, get.
        let flags = bob.ask(Elem::new("Flags").attr("Op", "1").attr("Which", "1").attr("Flags", "6"));
        assert_eq!((flags.get("WhichFlag"), flags.get("Flags")), (Some("1"), Some("6")));
        let flags = bob.ask(Elem::new("Flags").attr("Op", "2").attr("Which", "1").attr("Flags", "2"));
        assert_eq!(flags.get("Flags"), Some("4"));
        assert_eq!(bob.ask(Elem::new("Flags").attr("Op", "7").attr("Which", "1")).get("hr"), Some("0xC1120013"));

        // A tell reaches the recipient; an ignored sender is dropped silently.
        let sent = alice.ask(Elem::new("SendToUserEx").attr("Name", "Bob@Pyrealium").attr("Text", "hi\nthere").attr("Flags", "0"));
        assert_eq!(sent.get("hr"), Some("0x00000000"));
        let tell = bob.next();
        assert_eq!(
            (tell.name.as_str(), tell.get("From"), tell.get("Text")),
            ("UTellEv", Some("alice@Pyrealium"), Some("hi\nthere"))
        );
        assert_eq!(bob.ask(Elem::new("IgnoredAdd").attr("Name", "Alice@Pyrealium")).get("hr"), Some("0x00000000"));
        assert_eq!(alice.ask(Elem::new("SendToUserEx").attr("Name", "Bob@Pyrealium").attr("Text", "x")).get("hr"), Some("0x00000000"));
        let ignored = bob.ask(Elem::new("GetIgnoredList"));
        assert_eq!(ignored.children.len(), 1);
        assert_eq!(
            alice.ask(Elem::new("SendToUserEx").attr("Name", "Nobody@Pyrealium").attr("Text", "x")).get("hr"),
            Some("0xC1120012")
        );

        // A character in the world shows its owner's friends a logged-in, in-game mode.
        publish_online([("Zed".to_owned(), "zed_account".to_owned())]);
        assert_eq!(alice.ask(Elem::new("FriendAdd").attr("Name", "Zed@Pyrealium")).get("hr"), Some("0x00000000"));
        assert_eq!(alice.next().get("Mode"), Some("9"));
        publish_online(Vec::new());
        assert_eq!(alice.next().get("Mode"), Some("0"));

        // Bob leaves: Alice is told he is gone.
        drop(bob);
        assert_eq!(alice.next().get("Mode"), Some("0"));

        // The lists persist across a restart.
        drop(alice);
        service.stop();
        let again = start(GunConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            database: dir.clone(),
            auth: GunAuth::Developer,
        })
        .unwrap();
        let mut alice = Client::connect(again.addr);
        alice.send(Elem::new("Connect").attr("ProtoVer", "1"));
        alice.next();
        alice.send(Elem::new("PresentTicketCmd").attr("seq", "1").attr("Ticket", crate::auth::hex(b"alice")));
        assert_eq!(alice.next().name, "PresentTicketRes");
        let start = alice.next();
        assert_eq!(start.name, "FStart");
        assert_eq!(start.children.len(), 2);
        // Presence must reach the restarted service, not the old hub.
        publish_online([("Zed".to_owned(), "zed_account".to_owned())]);
        let online = alice.next();
        assert_eq!((online.name.as_str(), online.get("Name"), online.get("Mode")),
            ("FriendStat", Some("Zed@Pyrealium"), Some("9")));
        again.stop();
    }

    #[test]
    fn developer_mode_refuses_a_public_bind() {
        let result = start(GunConfig {
            bind: "0.0.0.0:0".parse().unwrap(),
            database: crate::db::Database::new("host=/nonexistent dbname=unused").unwrap(),
            auth: GunAuth::Developer,
        });
        assert!(result.is_err());
    }
}
