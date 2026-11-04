//! Local live-edit endpoint for the world builder (loopback only).
//!
//! `GET /admin/status`, `GET /admin/content` (`{revision, content}`: the world builder's content
//! database dump the server runs, without the shipped world's rows, for the editor to start from;
//! `/admin/quests` answers the same) and `POST /admin/sync` (JSON `{base, content}`: the edited
//! dump, with its placements, removed base placements, rules and quests, and the revision it was
//! edited from). The request thread parses, checks and diffs (`live_edit::Planner`); the game
//! loop applies the result between packets and replies; then the dump is saved as a layer over
//! the shipped content (`NAME.content.sql` with `NAME.content.base.sql`), so a restart gives the
//! same world. `GET /admin/strings?entity=0x47...` (or
//! `?table=0x25...`) lists a DAT string table's lines for the quest editor (`dat_lines`). Every request carries
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
    /// The world builder's content (`NAME.content.sql`), saved only while it differs from the
    /// shipped content, and the shipped dump it was edited from (`NAME.content.base.sql`): the
    /// start merges it over the shipped dump as it is then (`content_db::merge`).
    pub content: PathBuf,
    pub content_base: PathBuf,
}

/// What the server runs of a world builder's content dump: its placements and rules as the
/// readers' text, and the base placements it removes.
pub fn world_of(content: &str) -> io::Result<(String, String, std::collections::BTreeSet<u64>)> {
    if content.trim().is_empty() { return Ok((String::new(), String::new(), Default::default())); }
    let db = crate::content_db::open_dump(content)?;
    Ok((crate::content_db::overlay::builder_text(&db)?, crate::content_db::overlay::rules_text(&db)?,
        crate::content_db::overlay::tombstones(&db)?))
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
fn save_content(paths: &Paths, content: &str, shipped: &str) -> io::Result<()> {
    if content == shipped {
        for path in [&paths.content, &paths.content_base] {
            match std::fs::remove_file(path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
        }
        return Ok(());
    }
    atomic_write(&paths.content_base, shipped)?;
    atomic_write(&paths.content, content)
}

/// Start the endpoint. `content` is the world builder's content dump the server runs (without the
/// shipped world's rows; `None`: no content database), `shipped` the shipped one in the same form.
pub fn start(bind: SocketAddr, data: &Path, game_data: PathBuf, paths: Paths,
             base: std::collections::BTreeMap<u64, crate::content::Object>, content: Option<String>, shipped: String) -> io::Result<Handle> {
    if !bind.ip().is_loopback() {
        return Err(io::Error::other("--admin-bind is local only: use a loopback address such as 127.0.0.1:19373"));
    }
    if paths.content == paths.content_base {
        return Err(io::Error::other("the content and its base must be different files"));
    }
    // The DAT's string tables for the quest editor's line picker (none if the DAT will not open).
    let lines = crate::dat_lines::Lines::open(&game_data).ok();
    let content = content.unwrap_or_default();
    let (world, rules, hidden) = world_of(&content)?;
    let planner = Planner::new(game_data, &world, &rules, hidden, base, content)?;
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
        let (mut planner, mut lines) = (planner, lines);
        for request in server.incoming_requests() {
            // A browser closing its connection must not shut down the editor.
            let _ = handle(request, &token, &mut planner, &mut lines, &sender, &published, &paths, &shipped);
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

#[allow(clippy::too_many_arguments)]
fn handle(mut request: Request, token: &str, planner: &mut Planner, lines: &mut Option<crate::dat_lines::Lines>, jobs: &mpsc::SyncSender<Job>,
          players: &AtomicUsize, paths: &Paths, shipped: &str) -> io::Result<()> {
    if !authorized(&request, token) {
        return request.respond(json(401, serde_json::json!({"error": "missing or wrong admin token"})));
    }
    let response = match (request.method(), request.url()) {
        (Method::Get, "/admin/status") => json(200, serde_json::json!({
            "revision": planner.revision(), "objects": planner.objects(),
            "quests": crate::quest_emotes::tables().map_or(0, |t| t.quests().count()),
            "players": players.load(Ordering::Relaxed)})),
        (Method::Get, "/admin/content" | "/admin/quests") => json(200, serde_json::json!({
            "revision": planner.revision(), "content": planner.quests(), "quests": planner.quests()})),
        (Method::Post, "/admin/sync") => {
            let mut body = String::new();
            let read = request.as_reader().take(MAX_BODY + 1).read_to_string(&mut body);
            match read {
                Ok(_) if body.len() as u64 <= MAX_BODY => sync(&body, planner, jobs, paths, shipped),
                _ => json(413, serde_json::json!({"error": "request body over 9 MiB or not UTF-8"})),
            }
        }
        (Method::Get, url) if url.starts_with("/admin/strings?") => strings(&url["/admin/strings?".len()..], lines.as_mut()),
        _ => json(404, serde_json::json!({"error": "unknown admin request"})),
    };
    request.respond(response)
}

/// `entity=0x47...` (the NPC's own table) or `table=0x25...`: `{table, lines: [{key, text}]}`.
fn strings(query: &str, lines: Option<&mut crate::dat_lines::Lines>) -> Response<io::Cursor<Vec<u8>>> {
    let Some(lines) = lines else { return json(503, serde_json::json!({"error": "the server's DAT string tables are not available"})) };
    let value = |name: &str| query.split('&').find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
        .and_then(|v| u32::from_str_radix(v.trim_start_matches("0x"), 16).ok());
    let table = match (value("entity"), value("table")) {
        (Some(entity), None) => match lines.table_of(entity) {
            Some(table) => table,
            None => return json(404, serde_json::json!({"error": "that entity has no named string table"})),
        },
        (None, Some(table)) => table,
        _ => return json(400, serde_json::json!({"error": "ask for entity=0x47... or table=0x25..."})),
    };
    match lines.lines(table) {
        Ok(found) => json(200, serde_json::json!({"table": format!("0x{table:08x}"),
            "lines": found.into_iter().map(|(key, text)| serde_json::json!({"key": format!("0x{key:08x}"), "text": text})).collect::<Vec<_>>()})),
        Err(error) => json(404, serde_json::json!({"error": error.to_string()})),
    }
}

fn sync(body: &str, planner: &mut Planner, jobs: &mpsc::SyncSender<Job>, paths: &Paths, shipped: &str) -> Response<io::Cursor<Vec<u8>>> {
    let usage = || json(400, serde_json::json!({"error": "body must be JSON {base, content}: the revision and the content dump"}));
    let Ok(request) = serde_json::from_str::<serde_json::Value>(body) else { return usage() };
    let (Some(base), Some(content)) = (request["base"].as_str(), request["content"].as_str()) else { return usage() };
    if base != planner.revision() {
        return json(409, serde_json::json!({"error": "the server runs a different revision; resync from it",
            "revision": planner.revision()}));
    }
    let (world, rules, hidden) = match world_of(content) {
        Ok(parts) => parts,
        Err(error) => return json(400, serde_json::json!({"error": error.to_string()})),
    };
    let plan = match planner.plan(&world, &rules, &hidden, Some(content)) {
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
    let saved = save_content(paths, planner.quests(), shipped);
    json(200, serde_json::json!({
        "revision": planner.revision(), "added": summary.added, "removed": summary.removed,
        "replaced": summary.replaced, "rules": summary.rules, "quests": summary.quests, "messages": summary.messages,
        "apply_us": summary.apply_us, "content": planner.quests(),
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
        let paths = Paths { content: folder.join("w.content.sql"), content_base: folder.join("w.content.base.sql") };
        // What the world builder is handed: the shipped content without the world's rows.
        // (No item needing the DATs either: this folder has none, so the quests' item rewards go.)
        let shipped = {
            let full = crate::content_db::without_world(&std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/content.sql")).unwrap()).unwrap();
            let d = crate::content_db::open_dump(&full).unwrap();
            d.execute_batch("DELETE FROM emote_action WHERE action IN ('GiveItem', 'GiveStack', 'RandomGive'); DELETE FROM quest_recipe;").unwrap();
            crate::content_db::export(&d).unwrap()
        };
        let handle = start("127.0.0.1:0".parse().unwrap(), &folder, folder.clone(), paths, Default::default(), Some(shipped.clone()), shipped.clone()).unwrap();
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
                let _ = job.reply.send(Ok(crate::live_edit::Summary { rules, quests, removed: job.change.removed.len(), ..Default::default() }));
            }
        });
        assert_eq!(call(handle.address, "wrong", "GET", "/admin/status", "").0, 401);
        let (code, status) = call(handle.address, &token, "GET", "/admin/status", "");
        assert_eq!(code, 200);
        let base = status["revision"].as_str().unwrap().to_owned();
        let (code, running) = call(handle.address, &token, "GET", "/admin/content", "");
        assert_eq!((code, running["content"].as_str().map(str::len), running["revision"].as_str()), (200, Some(shipped.len()), Some(base.as_str())));
        // The world builder's edits: a creature rule and a quest, as rows of its content dump.
        let d = crate::content_db::open_dump(&shipped).unwrap();
        d.execute("INSERT INTO creature_stats (target, health, source) VALUES ('0x470007d1', 5, 't')", []).unwrap();
        d.execute("INSERT INTO quest (name, journal_id, title_table, title_key, title, source) VALUES ('q', '0x40fff001', '0x25000001', '0x1', 'Q', 't')", []).unwrap();
        let dump = crate::content_db::export(&d).unwrap();
        let push = serde_json::json!({"base": base, "content": dump}).to_string();
        let (code, pushed) = call(handle.address, &token, "POST", "/admin/sync", &push);
        assert_eq!((code, pushed["rules"].as_u64(), pushed["saved"].as_bool()), (200, Some(1), Some(true)), "{pushed}");
        assert_eq!(pushed["quests"].as_u64(), Some(crate::quest_emotes::Tables::from_sql(&shipped).unwrap().quests().count() as u64 + 1));
        assert_eq!(std::fs::read_to_string(folder.join("w.content.sql")).unwrap(), dump);
        assert_eq!(std::fs::read_to_string(folder.join("w.content.base.sql")).unwrap(), shipped, "the shipped content it was edited from");
        assert_eq!(call(handle.address, &token, "GET", "/admin/content", "").1["content"].as_str(), Some(dump.as_str()));
        assert_eq!(call(handle.address, &token, "POST", "/admin/sync", &push).0, 409, "stale base revision");
        // The same dump again changes nothing; a dump that does not check is refused whole.
        let again = serde_json::json!({"base": pushed["revision"], "content": dump}).to_string();
        let (code, same) = call(handle.address, &token, "POST", "/admin/sync", &again);
        assert_eq!((code, same["quests"].is_null(), same["rules"].as_u64(), same["revision"].clone()), (200, true, Some(0), pushed["revision"].clone()));
        let broken = serde_json::json!({"base": pushed["revision"], "content": dump.replace("'0x40fff001'", "'nonsense'")}).to_string();
        assert_eq!(call(handle.address, &token, "POST", "/admin/sync", &broken).0, 400);
        assert_eq!(call(handle.address, &token, "POST", "/admin/sync", "{\"base\": \"x\"}").0, 400, "no content");
        // Back to the shipped content: the layer files go.
        let back = serde_json::json!({"base": pushed["revision"], "content": shipped}).to_string();
        let (code, reverted) = call(handle.address, &token, "POST", "/admin/sync", &back);
        assert_eq!((code, reverted["saved"].as_bool()), (200, Some(true)));
        assert!(!folder.join("w.content.sql").exists() && !folder.join("w.content.base.sql").exists());
        assert!(start("0.0.0.0:0".parse().unwrap(), &folder, folder.clone(),
            Paths { content: folder.join("c"), content_base: folder.join("d") }, Default::default(), None, String::new()).is_err(), "local only");
        std::fs::remove_dir_all(&folder).unwrap();
    }

}
