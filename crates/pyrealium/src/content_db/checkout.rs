//! The content database written out as the files it was imported from, and read back: what a
//! generator under `tools/` works on (`scripts/with-content-files.sh`). `checkout` writes every
//! table file, document, world pack and evidence file the database holds into a folder; the set is
//! consistent as written (each pack report's digests name the pack and catalog texts written beside
//! it). `checkin` imports a folder's files and packs (`files::import`, `world::import_packs`).
use std::io;
use std::path::Path;

use rusqlite::Connection;

use super::{files, sql, world};

/// What `checkout` wrote.
pub struct Written { pub files: usize, pub packs: usize, pub evidence: usize }

fn put(dir: &Path, name: &str, text: &str) -> io::Result<()> {
    let path = dir.join(name);
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    std::fs::write(&path, text).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display())))
}

/// Writes the database's content files into `dir` (replacing files of the same names).
pub fn checkout(db: &Connection, dir: &Path) -> io::Result<Written> {
    let mut written = Written { files: 0, packs: 0, evidence: 0 };
    let names: Vec<&str> = files::FILES.iter().map(|(n, _)| *n).chain(files::DOCUMENTS).collect();
    for name in names {
        if let Some(text) = files::rebuild(db, name)? {
            put(dir, name, &text)?;
            written.files += 1;
        }
    }
    let packs: Vec<String> = db.prepare("SELECT name FROM world_pack WHERE kind <> 'builder' ORDER BY ord").map_err(sql)?
        .query_map([], |r| r.get(0)).map_err(sql)?.collect::<Result<_, _>>().map_err(sql)?;
    for pack in packs {
        let name = files::PACK_FILES.iter().find(|(_, p)| *p == pack).map_or(format!("{pack}.tsv"), |(n, _)| n.to_string());
        if let Some(text) = world::pack_text(db, &pack)? {
            put(dir, &name, &text)?;
            written.packs += 1;
        }
    }
    let evidence: Vec<(String, String)> = db.prepare("SELECT name, body FROM world_evidence ORDER BY name").map_err(sql)?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?))).map_err(sql)?.collect::<Result<_, _>>().map_err(sql)?;
    for (name, body) in evidence {
        let value: serde_json::Value = serde_json::from_str(&body).map_err(|e| io::Error::other(format!("{name}: {e}")))?;
        put(dir, &name, &(serde_json::to_string_pretty(&value).map_err(io::Error::other)? + "\n"))?;
        written.evidence += 1;
    }
    Ok(written)
}

/// Imports `dir`'s content files, documents, world packs and evidence into the database.
pub fn checkin(db: &mut Connection, dir: &Path) -> io::Result<(files::Imported, world::Imported)> {
    let imported = files::import(db, dir)?;
    let packs = world::import_packs(db, dir)?;
    Ok((imported, packs))
}

/// The repository's content (`content/content.sql`) checked out into a temporary folder, for tests
/// that read content files by path. The folder is named for the dump's digest, so every test
/// process of one dump shares it (made once, atomically) and a changed dump makes a new one.
pub fn repository() -> &'static Path {
    static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let shipped = std::fs::read_to_string(content.join(super::DUMP)).expect("the repository's content.sql");
        let digest = world::sha256_hex(shipped.as_bytes());
        let dir = std::env::temp_dir().join(format!("pyrealium-content-{}", &digest[..16]));
        if !dir.join(super::DUMP).exists() {
            let staging = std::env::temp_dir().join(format!("pyrealium-content-{}-{}", &digest[..16], std::process::id()));
            let db = super::open_dump(&shipped).expect("the repository's content.sql");
            checkout(&db, &staging).expect("checking out the repository's content");
            std::fs::write(staging.join(super::DUMP), &shipped).expect("copying content.sql");
            // Another process may have made it meanwhile: its copy is the same.
            if std::fs::rename(&staging, &dir).is_err() { let _ = std::fs::remove_dir_all(&staging); }
        }
        dir
    })
}

#[cfg(test)]
mod tests {
    /// The shipped dump checked out and checked in again is the same dump.
    #[test]
    fn a_checkout_checks_in_as_the_same_content() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content").join(crate::content_db::DUMP);
        let shipped = std::fs::read_to_string(path).unwrap();
        let db = crate::content_db::open_dump(&shipped).unwrap();
        let dir = std::env::temp_dir().join(format!("pyrealium-checkout-{}", std::process::id()));
        let written = super::checkout(&db, &dir).unwrap();
        assert!(written.files > 40 && written.packs > 15 && written.evidence > 10);
        let mut again = crate::content_db::open_dump(&shipped).unwrap();
        super::checkin(&mut again, &dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let (before, after) = (crate::content_db::export(&db).unwrap(), crate::content_db::export(&again).unwrap());
        // `source` names where rows came from; everything else is the same.
        let strip = |t: &str| t.lines().map(|l| l.split(", imported by").next().unwrap_or(l).to_owned()).collect::<Vec<_>>();
        assert!(strip(&before) == strip(&after), "the checked-in content differs");
    }
}
