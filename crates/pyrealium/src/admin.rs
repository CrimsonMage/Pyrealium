//! Local live-edit endpoint for the world builder (loopback only).
//!
//! `GET /admin/status`, `GET /admin/quests` (`{revision, quests}`: the quest database dump
//! the server runs, for the editor to start from) and `POST /admin/sync` (JSON
//! `{base, world, rules, quests?}`: the overlay world and rules texts, optionally a whole
//! quest database dump, and the revision they were edited from). The
//! request thread parses, checks and diffs (`live_edit::Planner`); the game loop
//! applies the result between packets and replies; then the texts are saved to the
//! overlay paths so a restart gives the same world. Every request carries
//! `Authorization: Bearer TOKEN`; the token is in `DATA/admin.token` (owner-only).
use crate::live_edit::{Job, Planner};
use std::{
    io::{self, Read, Write},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{atomic::{AtomicUsize, Ordering}, mpsc, Arc},
};
use tiny_http::{Header, Method, Request, Response, Server};

const MAX_BODY: u64 = 9 * 1024 * 1024;

/// The game loop's end: jobs to apply, and the player count it publishes.
pub struct Handle {
    pub jobs: mpsc::Receiver<Job>,
    pub players: Arc<AtomicUsize>,
    pub address: SocketAddr,
}

pub struct Paths {
    pub world: PathBuf,
    pub rules: PathBuf,
    /// The world builder's quests (`NAME.quests.sql`), saved only while they differ from the
    /// shipped quests, and the shipped dump they were edited from (`NAME.quests.base.sql`): the
    /// start merges them over the shipped dump as it is then (`quest_emotes::db::merge`).
    pub quests: PathBuf,
    pub quests_base: PathBuf,
}

fn write_private(path: &Path, text: &str) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(text.as_bytes())
}

fn atomic_write(path: &Path, text: &str) -> io::Result<()> {
    let temporary = path.with_extension("live.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
}

/// Saves the world builder's quests as a layer over the shipped ones: with the shipped dump they
/// were edited from, or nothing once they are the shipped quests again.
fn save_quests(paths: &Paths, quests: &str, shipped: &str) -> io::Result<()> {
    if quests == shipped {
        for path in [&paths.quests, &paths.quests_base] {
            match std::fs::remove_file(path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        return Ok(());
    }
    atomic_write(&paths.quests_base, shipped)?;
    atomic_write(&paths.quests, quests)
}

fn read_or_empty(path: &Path) -> io::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error),
    }
}

/// Start the endpoint. `world`/`rules` are the texts the server loaded at startup; `quests` the
/// quest database dump it runs (`None`: no quest tables), `shipped` the shipped one (canonical).
pub fn start(bind: SocketAddr, data: &Path, game_data: PathBuf, paths: Paths,
             base: std::collections::BTreeMap<u64, crate::content::Object>, quests: Option<String>, shipped: String) -> io::Result<Handle> {
    if !bind.ip().is_loopback() {
        return Err(io::Error::other("--admin-bind is local only: use a loopback address such as 127.0.0.1:19373"));
    }
    if paths.world==paths.rules || paths.quests==paths.world || paths.quests==paths.rules || paths.quests_base==paths.quests {
        return Err(io::Error::other("world, rules and quests paths must be different"));
    }
    let planner = Planner::new(game_data, &read_or_empty(&paths.world)?, &read_or_empty(&paths.rules)?, base, quests.unwrap_or_default())?;
    let mut secret = [0u8; 32];
    getrandom::getrandom(&mut secret).map_err(|error| io::Error::other(error.to_string()))?;
    let token: String = secret.iter().map(|b| format!("{b:02x}")).collect();
    let server = Server::http(bind).map_err(|error| io::Error::other(format!("admin endpoint {bind}: {error}")))?;
    write_private(&data.join("admin.token"), &token)?;
    let address = server.server_addr().to_ip().ok_or_else(|| io::Error::other("admin endpoint address"))?;
    let (sender, jobs) = mpsc::sync_channel(4);
    let players = Arc::new(AtomicUsize::new(0));
    let published = players.clone();
    std::thread::Builder::new().name("admin".into()).spawn(move || {
        let mut planner = planner;
        for request in server.incoming_requests() {
            // A browser closing its connection must not shut down the editor.
            let _ = handle(request, &token, &mut planner, &sender, &published, &paths, &shipped);
        }
    })?;
    println!("Live world editing on http://{address} (token in {}).", data.join("admin.token").display());
    Ok(Handle { jobs, players, address })
}

fn json(code: u16, value: serde_json::Value) -> Response<io::Cursor<Vec<u8>>> {
    Response::from_string(value.to_string()).with_status_code(code)
        .with_header(Header::from_bytes("Content-Type", "application/json").unwrap())
}

fn authorized(request: &Request, token: &str) -> bool {
    let expected = format!("Bearer {token}");
    request.headers().iter().find(|h| h.field.equiv("Authorization")).is_some_and(|h| {
        let given = h.value.as_str().as_bytes();
        // Constant time for equal lengths.
        given.len() == expected.len() && given.iter().zip(expected.as_bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0
    })
}

fn handle(mut request: Request, token: &str, planner: &mut Planner, jobs: &mpsc::SyncSender<Job>,
          players: &AtomicUsize, paths: &Paths, shipped: &str) -> io::Result<()> {
    if !authorized(&request, token) {
        return request.respond(json(401, serde_json::json!({"error": "missing or wrong admin token"})));
    }
    let response = match (request.method(), request.url()) {
        (Method::Get, "/admin/status") => json(200, serde_json::json!({
            "revision": planner.revision(), "objects": planner.objects(),
            "quests": crate::quest_emotes::tables().map_or(0, |t| t.quests().count()),
            "players": players.load(Ordering::Relaxed)})),
        (Method::Get, "/admin/quests") => json(200, serde_json::json!({
            "revision": planner.revision(), "quests": planner.quests()})),
        (Method::Post, "/admin/sync") => {
            let mut body = String::new();
            let read = request.as_reader().take(MAX_BODY + 1).read_to_string(&mut body);
            match read {
                Ok(_) if body.len() as u64 <= MAX_BODY => sync(&body, planner, jobs, paths, shipped),
                _ => json(413, serde_json::json!({"error": "request body over 9 MiB or not UTF-8"})),
            }
        }
        _ => json(404, serde_json::json!({"error": "unknown admin request"})),
    };
    request.respond(response)
}

fn sync(body: &str, planner: &mut Planner, jobs: &mpsc::SyncSender<Job>, paths: &Paths, shipped: &str) -> Response<io::Cursor<Vec<u8>>> {
    let Ok(request) = serde_json::from_str::<serde_json::Value>(body) else {
        return json(400, serde_json::json!({"error": "body must be JSON {base, world, rules, quests?}"}));
    };
    let (Some(base), Some(world), Some(rules)) =
        (request["base"].as_str(), request["world"].as_str(), request["rules"].as_str()) else {
        return json(400, serde_json::json!({"error": "body must be JSON {base, world, rules, quests?}"}));
    };
    let quests = request["quests"].as_str();
    if !request["quests"].is_null() && quests.is_none() {
        return json(400, serde_json::json!({"error": "quests must be the quest database dump (text)"}));
    }
    if base != planner.revision() {
        return json(409, serde_json::json!({"error": "the server runs a different revision; resync from it",
            "revision": planner.revision()}));
    }
    let plan = match planner.plan(world, rules, quests) {
        Ok(plan) => plan,
        Err(error) => return json(400, serde_json::json!({"error": error.to_string()})),
    };
    let (reply, answer) = mpsc::sync_channel(1);
    if jobs.send(Job { change: plan.change, reply }).is_err() {
        return json(503, serde_json::json!({"error": "the game loop has stopped"}));
    }
    // The loop drains jobs every pass (milliseconds); it replies or has stopped.
    let summary = match answer.recv() {
        Ok(Ok(summary)) => summary,
        Ok(Err(error)) => return json(400, serde_json::json!({"error": error})),
        Err(_) => return json(503, serde_json::json!({"error": "the game loop has stopped"})),
    };
    planner.commit(plan.committed);
    let saved = atomic_write(&paths.world, world).and_then(|()| atomic_write(&paths.rules, rules))
        .and_then(|()| if summary.quests.is_some() { save_quests(paths, planner.quests(), shipped) } else { Ok(()) });
    json(200, serde_json::json!({
        "revision": planner.revision(), "added": summary.added, "removed": summary.removed,
        "replaced": summary.replaced, "rules": summary.rules, "quests": summary.quests, "messages": summary.messages,
        "apply_us": summary.apply_us,
        "saved": saved.as_ref().map(|()| true).unwrap_or(false),
        "save_error": saved.err().map(|e| e.to_string()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream;

    fn call(address: SocketAddr, token: &str, method: &str, path: &str, body: &str) -> (u16, serde_json::Value) {
        let mut stream = TcpStream::connect(address).unwrap();
        // HTTP/1.0: a large reply (the quest dump) comes whole, not chunked.
        write!(stream, "{method} {path} HTTP/1.0\r\nHost: x\r\nAuthorization: Bearer {token}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        let mut text = String::new();
        stream.read_to_string(&mut text).unwrap();
        let code = text[9..12].parse().unwrap();
        let json = text.split("\r\n\r\n").nth(1).and_then(|b| serde_json::from_str(b).ok()).unwrap_or_default();
        (code, json)
    }

    #[test]
    fn sync_applies_through_the_game_loop_and_saves() {
        let folder = std::env::temp_dir().join(format!("pyrealium-admin-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let paths = Paths { world: folder.join("w.tsv"), rules: folder.join("w.rules.tsv"), quests: folder.join("w.quests.sql"),
            quests_base: folder.join("w.quests.base.sql") };
        let handle = start("127.0.0.1:0".parse().unwrap(), &folder, folder.clone(), paths, Default::default(), None, String::new()).unwrap();
        let token = std::fs::read_to_string(folder.join("admin.token")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(folder.join("admin.token")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // Stand-in game loop: apply nothing, answer.
        let jobs = handle.jobs;
        std::thread::spawn(move || {
            while let Ok(job) = jobs.recv() {
                let rules = job.change.rules.as_ref().map_or(0, |r| r.affected.len());
                let quests = job.change.quests.as_ref().map(|q| q.tables.quests().count());
                let _ = job.reply.send(Ok(crate::live_edit::Summary { rules, quests, ..Default::default() }));
            }
        });
        assert_eq!(call(handle.address, "wrong", "GET", "/admin/status", "").0, 401);
        let (code, status) = call(handle.address, &token, "GET", "/admin/status", "");
        assert_eq!(code, 200);
        let base = status["revision"].as_str().unwrap().to_owned();
        let rules = format!("{}\nstats\t470007d1\t5\t-\t-\t-\t-\t-\t-\t-\t-\t-\n", crate::world_rules::HEADER);
        let body = serde_json::json!({"base": base, "world": "", "rules": rules}).to_string();
        let (code, done) = call(handle.address, &token, "POST", "/admin/sync", &body);
        assert_eq!((code, done["rules"].as_u64(), done["saved"].as_bool()), (200, Some(1), Some(true)));
        assert_eq!(std::fs::read_to_string(folder.join("w.rules.tsv")).unwrap(), rules);
        assert_eq!(call(handle.address, &token, "POST", "/admin/sync", &body).0, 409, "stale base revision");
        let bad = serde_json::json!({"base": done["revision"], "world": "", "rules": "nonsense"}).to_string();
        assert_eq!(call(handle.address, &token, "POST", "/admin/sync", &bad).0, 400);
        // The quest database: the editor starts from what the server runs, then pushes a new dump
        // with the world and rules; the server checks it, swaps it in and saves it beside them.
        let (code, running) = call(handle.address, &token, "GET", "/admin/quests", "");
        assert_eq!((code, running["quests"].as_str(), running["revision"].as_str()), (200, Some(""), done["revision"].as_str()));
        let shipped = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/quests.sql")).unwrap();
        let d = crate::quest_emotes::db::open_dump(&shipped).unwrap();
        d.execute("INSERT INTO quest (name, journal_id, title_table, title_key, title, source) VALUES ('q', '0x40fff001', '0x25000001', '0x1', 'Q', 't')", []).unwrap();
        // No item needing the DATs (this folder has none): the quests' item rewards go.
        d.execute("DELETE FROM emote_action WHERE action IN ('GiveItem', 'GiveStack', 'RandomGive')", []).unwrap();
        let dump = crate::quest_emotes::db::export(&d).unwrap();
        let push = serde_json::json!({"base": done["revision"], "world": "", "rules": rules, "quests": dump}).to_string();
        let (code, pushed) = call(handle.address, &token, "POST", "/admin/sync", &push);
        assert_eq!((code, pushed["quests"].as_u64(), pushed["saved"].as_bool()), (200, Some(crate::quest_emotes::Tables::from_sql(&shipped).unwrap().quests().count() as u64 + 1), Some(true)), "{pushed}");
        assert_eq!(std::fs::read_to_string(folder.join("w.quests.sql")).unwrap(), dump);
        assert_eq!(std::fs::read_to_string(folder.join("w.quests.base.sql")).unwrap(), "", "the shipped quests it was edited from");
        assert_eq!(call(handle.address, &token, "GET", "/admin/quests", "").1["quests"].as_str(), Some(dump.as_str()));
        // Unchanged quests, or none sent, change nothing of them.
        let again = serde_json::json!({"base": pushed["revision"], "world": "", "rules": rules, "quests": dump}).to_string();
        let (code, same) = call(handle.address, &token, "POST", "/admin/sync", &again);
        assert_eq!((code, same["quests"].is_null(), same["revision"].clone()), (200, true, pushed["revision"].clone()));
        // A dump that does not check is refused whole, naming the row.
        let broken = dump.replace("'0x40fff001'", "'nonsense'");
        let refused = serde_json::json!({"base": pushed["revision"], "world": "", "rules": rules, "quests": broken}).to_string();
        let (code, why) = call(handle.address, &token, "POST", "/admin/sync", &refused);
        assert_eq!(code, 400, "{why}");
        assert_eq!(call(handle.address, &token, "GET", "/admin/quests", "").1["quests"].as_str(), Some(dump.as_str()));
        assert!(start("0.0.0.0:0".parse().unwrap(), &folder, folder.clone(),
            Paths { world: folder.join("a"), rules: folder.join("b"), quests: folder.join("c"), quests_base: folder.join("d") },
            Default::default(), None, String::new()).is_err(), "local only");
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
