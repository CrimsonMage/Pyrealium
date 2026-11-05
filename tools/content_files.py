"""The shipped content files (creature, loot, skill and route tables, the bundled world, the JSON
documents) as the server reads them: rebuilt from the content database's rows
(`crates/pyrealium/src/content_db/files.rs`), the same text the files held.

`text(path)` takes a file's path under a content folder (`content/portal-routes.tsv`): the file
itself where one is on disk (a folder of files a tool wrote), else its text from that folder's
`content.sql` (its `content.sqlite` when that is newer). `write(folder, {name: text})` puts files
a generator made into the folder's `content.sql` (`pyrealium-content import-files`).
"""
from pathlib import Path
import shutil
import sqlite3
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
_OPEN = {}


def _database(folder):
    """The content database of `folder` (cached per dump state), or None when it has none."""
    folder = Path(folder).resolve()
    dump, built = folder / "content.sql", folder / "content.sqlite"
    if not dump.exists():
        return None
    use_built = built.exists() and built.stat().st_mtime_ns >= dump.stat().st_mtime_ns
    key = (folder, dump.stat().st_mtime_ns, use_built and built.stat().st_mtime_ns)
    if key not in _OPEN:
        for old in [k for k in _OPEN if k[0] == folder]:
            _OPEN.pop(old).close()
        if use_built:
            db = sqlite3.connect(f"{built.as_uri()}?mode=ro", uri=True)
        else:
            db = sqlite3.connect(":memory:")
            db.executescript(dump.read_text(encoding="utf-8"))
        _OPEN[key] = db
    return _OPEN[key]


def _locate(path):
    """(content folder, file name) for a path under a content folder (`7230/` names included)."""
    path = Path(path)
    for folder, name in ((path.parent, path.name), (path.parent.parent, f"{path.parent.name}/{path.name}")):
        if (folder / "content.sql").exists():
            return folder, name
    return path.parent, path.name


# The world files read whole are world packs (`files::PACK_FILES`); any other pack is NAME.tsv.
PACK_FILES = {"world.tsv": "bundled-world"}


def pack_text(db, pack):
    """A world pack's canonical text (`world::pack_text`): the header of its rows' width, then its rows."""
    rows, width = [], None
    for values in db.execute("SELECT iid, entity, visual, cell, x, y, z, qw, qx, qy, qz, scale, name FROM world_object "
                             "WHERE pack = ? ORDER BY ord", (pack,)):
        fields = [v[2:] if i < 4 and v.startswith("0x") else v for i, v in enumerate(values) if v is not None]
        width = len(fields) if width is None else max(width, len(fields))
        rows.append("\t".join(fields))
    header = {11: "pyrealium-world-v2", 12: "pyrealium-world-v3"}.get(width, "pyrealium-world-v4")
    return header + "\n" + "".join(row + "\n" for row in rows)


def rebuild(db, name):
    """A content file's text from the database `db`, or None when it holds no such file."""
    have = db.execute("SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'content_file'").fetchone()
    if not have:
        return None
    pack = PACK_FILES.get(name, name[:-4] if name.endswith(".tsv") and "/" not in name else None)
    if pack and db.execute("SELECT 1 FROM world_pack WHERE name = ?", (pack,)).fetchone():
        return pack_text(db, pack)
    row = db.execute("SELECT rows_in, header, footer FROM content_file WHERE name = ?", (name,)).fetchone()
    if row is None:
        for table in ("content_document", "world_evidence"):
            body = db.execute(f"SELECT body FROM {table} WHERE name = ?", (name,)).fetchone()
            if body:
                return body[0]
        return None
    rows_in, text, footer = row
    lines = []
    columns = [c[1] for c in db.execute(f"PRAGMA table_info({rows_in})") if c[1] not in ("file", "ord", "note", "family")]
    for values in db.execute(f"SELECT note, {', '.join(columns)} FROM {rows_in} WHERE file = ? ORDER BY ord", (name,)):
        if values[0] is not None:
            lines.append(values[0])
        fields = []
        for v in values[1:]:
            if v is None:
                break
            fields.append(v)
        lines.append("\t".join(fields))
    return text + "".join(line + "\n" for line in lines) + footer


def text(path):
    """A content file's text: the file on disk, else rebuilt from its folder's content database;
    None when neither has it."""
    path = Path(path)
    if path.exists():
        return path.read_text(encoding="utf-8")
    folder, name = _locate(path)
    db = _database(folder)
    return rebuild(db, name) if db is not None else None


def exists(path):
    return text(path) is not None


def write(folder, files, binary=None):
    """Puts generated content files ({name: text}, names as under content/) into `folder`'s
    content.sql through the server's own import (`pyrealium-content import-files`)."""
    folder = Path(folder).resolve()
    with tempfile.TemporaryDirectory() as staging:
        staging = Path(staging)
        shutil.copy(folder / "content.sql", staging / "content.sql")
        for name, body in files.items():
            (staging / name).parent.mkdir(parents=True, exist_ok=True)
            (staging / name).write_text(body, encoding="utf-8")
        command = [str(binary)] if binary else ["cargo", "run", "-q", "-p", "pyrealium", "--bin", "pyrealium-content", "--"]
        subprocess.run(command + ["import-files", str(staging), "--force"], check=True, cwd=ROOT)
        shutil.copy(staging / "content.sql", folder / "content.sql")
