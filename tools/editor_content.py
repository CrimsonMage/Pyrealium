"""The world builder's world as a content database: NAME.content.sqlite (edited) and its dump
NAME.content.sql, beside the world's name NAME.tsv.

It is the server's content database (`content_db`) without the shipped world's rows. It holds
this world's quests, its placements (the `builder` pack's `world_object` rows: new objects,
and prepared objects moved or changed, by placement id), the prepared placements it removes
(`world_tombstone`), and its creature rules, patrols and spawn groups (`creature_*`, `patrol`,
`spawn_group*`).

- The live link pushes the dump on every save. The server checks it, applies it and saves it
  as a layer over the shipped content (NAME.content.sql with NAME.content.base.sql).
- The editor's document code works on world text and rules text, so `world_text` /
  `write_world` and `rules_text` / `write_rules` read and write those rows as that text.
- A world saved by an older editor (NAME.tsv, NAME.rules.tsv, NAME.quests.sql) becomes its
  content database the first time it is opened (`ensure_database`), as the server does at start.
"""
import os
from pathlib import Path
import sqlite3
import subprocess
import sys

LIMIT = 8 * 1024 * 1024
SHIPPED = Path(__file__).resolve().parent.parent / "content" / "content.sql"
BUILDER = "builder"


def content_path(world_path):
    path = Path(world_path)
    return path.with_name(path.stem + ".content.sql")


def content_db_path(world_path):
    path = Path(world_path)
    return path.with_name(path.stem + ".content.sqlite")


def base_path(world_path):
    path = Path(world_path)
    return path.with_name(path.stem + ".content.base.sql")


def received_path(world_path):
    """Marks a world received from a live server (its edits push from then on)."""
    path = Path(world_path)
    return path.with_name(path.stem + ".content.received")


def mtime(path):
    try:
        return path.stat().st_mtime_ns
    except OSError:
        return None


def signature(world_path):
    """What changes when the content files are saved."""
    return mtime(content_path(world_path)), mtime(content_db_path(world_path))


def read_dump(path, limit=LIMIT):
    """A dump's text; a world's dump is refused past `limit` (`None`: no limit, the shipped one)."""
    with Path(path).open("rb") as file:
        data = file.read() if limit is None else file.read(limit + 1)
    if limit is not None and len(data) > limit:
        raise ValueError(f"{Path(path).name} exceeds {limit // (1024 * 1024)} MiB")
    return data.decode("utf-8")


def dump_database(path):
    """The SQL dump of the edited database, read only (the server makes it canonical)."""
    db = sqlite3.connect(f"{Path(path).resolve().as_uri()}?mode=ro", uri=True)
    try:
        return "\n".join(db.iterdump()) + "\n"
    finally:
        db.close()


def without_world(text):
    """A content dump without the shipped world's rows (the builder's own stay) and without the
    content files (`content_file`, its row tables and `content_document`: shipped data the world
    builder does not edit; the server keeps them in a merge)."""
    db = sqlite3.connect(":memory:")
    try:
        db.executescript(text)
        db.executescript("DELETE FROM world_evidence; "
                         "DELETE FROM world_object WHERE pack IN (SELECT name FROM world_pack WHERE kind <> 'builder'); "
                         "DELETE FROM world_pack WHERE kind <> 'builder';")
        files = [name for (name,) in db.execute("SELECT m.name FROM sqlite_master m WHERE m.type = 'table' AND (m.name IN "
                 "('content_file', 'content_document') OR EXISTS (SELECT 1 FROM pragma_table_info(m.name) WHERE name = 'file'))")]
        for name in sorted(files, key=lambda n: n == "content_file"):
            db.execute(f"DELETE FROM {name}")
        return "\n".join(db.iterdump()) + "\n"
    finally:
        db.close()


def has_local_edits(world_path):
    """Whether this world holds anything of its own: placements, removals or rules (a world just
    started from the shipped content holds none until its first save)."""
    database = content_db_path(world_path)
    if not database.exists():
        return False
    db = sqlite3.connect(f"{database.resolve().as_uri()}?mode=ro", uri=True)
    try:
        return any(db.execute(q).fetchone()[0] for q in (
            "SELECT COUNT(*) FROM world_object WHERE pack = 'builder'", "SELECT COUNT(*) FROM world_tombstone",
            "SELECT COUNT(*) FROM creature_stats", "SELECT COUNT(*) FROM creature_ai", "SELECT COUNT(*) FROM creature_loot",
            "SELECT COUNT(*) FROM patrol", "SELECT COUNT(*) FROM spawn_group"))
    finally:
        db.close()


def content_text(world_path):
    """The content to push: the edited database when it is newer than the dump, else the dump,
    else None (none here yet: the live link fetches the server's). A world started here from the
    shipped content, with nothing of its own yet, also fetches the server's first: the server may
    hold this world's edits."""
    sql, database = content_path(world_path), content_db_path(world_path)
    if not base_path(world_path).exists() and not received_path(world_path).exists() and not has_local_edits(world_path):
        return None
    sql_time, db_time = mtime(sql), mtime(database)
    if db_time is not None and (sql_time is None or db_time > sql_time):
        return dump_database(database)
    if sql_time is not None:
        return read_dump(sql)
    return None


def save_dump(world_path, text):
    """Writes the server's canonical dump as NAME.content.sql (only when it differs). The edited
    database holds the same rows and may be open in a tool, so it is never rebuilt here: when it
    was the newer file it stays the newer one."""
    path, database = content_path(world_path), content_db_path(world_path)
    if path.exists() and read_dump(path) == text:
        return False
    database_newer = mtime(database) is not None and (mtime(path) is None or mtime(database) > mtime(path))
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(text, encoding="utf-8")
    os.replace(temporary, path)
    if database_newer:
        os.utime(database)
        if mtime(database) <= mtime(path):
            stamp = mtime(path) + 1_000_000
            os.utime(database, ns=(stamp, stamp))
    return True


def _build(database, text):
    temporary = database.with_name(database.name + ".tmp")
    if temporary.exists():
        temporary.unlink()
    db = sqlite3.connect(temporary)
    try:
        db.executescript(text)
        db.commit()
    finally:
        db.close()
    os.replace(temporary, database)


def ensure_database(world_path, shipped=SHIPPED):
    """NAME.content.sqlite: as it is, else from NAME.content.sql, else migrated from an older
    world's files (or a new world: the shipped content without the world's rows)."""
    world = Path(world_path)
    sql, database = content_path(world), content_db_path(world)
    if database.exists() and (not sql.exists() or mtime(database) >= mtime(sql)):
        return database
    if sql.exists():
        _build(database, read_dump(sql))
        return database
    old_quests, old_base = world.with_name(world.stem + ".quests.sql"), world.with_name(world.stem + ".quests.base.sql")
    shipped_view = without_world(read_dump(shipped, limit=None)) if shipped.exists() else ""
    start = read_dump(old_quests) if old_quests.exists() else shipped_view
    if not start:
        raise ValueError(f"No {sql.name} yet, and no shipped content to start it from")
    _build(database, start)
    old_world = world if world.exists() and world.suffix == ".tsv" else None
    if old_world is not None and old_world.read_text(encoding="utf-8").strip():
        write_world(world, old_world.read_text(encoding="utf-8"), save=False)
    old_rules = world.with_name(world.stem + ".rules.tsv")
    if old_rules.exists() and old_rules.read_text(encoding="utf-8").strip():
        write_rules(world, old_rules.read_text(encoding="utf-8"), save=False)
    export(world)
    if old_quests.exists() or old_world is not None or old_rules.exists():
        # A migrated world: the shipped content it was edited from.
        base_path(world).write_text(read_dump(old_base) if old_base.exists() else shipped_view, encoding="utf-8")
    return database


def _connect(world_path):
    db = sqlite3.connect(ensure_database(world_path))
    db.execute("PRAGMA foreign_keys = ON")
    # A database built before spawn chances (schema 16) gets the column, empty (as the server's
    # `open_dump` adds it to an older dump).
    if "chance" not in [c[1] for c in db.execute("PRAGMA table_info(spawn_group_member)")]:
        db.execute("ALTER TABLE spawn_group_member ADD COLUMN chance TEXT")
    return db


def export(world_path):
    """NAME.content.sql from the edited database (the server returns its canonical text)."""
    path = content_path(world_path)
    text = dump_database(content_db_path(world_path))
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(text, encoding="utf-8")
    os.replace(temporary, path)
    # The database stays the newer file: it is the edited one.
    stamp = (mtime(path) or 0) + 1_000_000
    os.utime(content_db_path(world_path), ns=(stamp, stamp))
    return text


def _bare(value):
    return value[2:] if isinstance(value, str) and value.lower().startswith("0x") else value


def world_text(world_path):
    """The builder's placements as world text (`pyrealium-world-v3`, as the document reads it)."""
    db = _connect(world_path)
    try:
        rows = db.execute("SELECT o.iid, o.entity, o.visual, o.cell, o.x, o.y, o.z, o.qw, o.qx, o.qy, o.qz, o.scale "
                          "FROM world_object o JOIN world_pack p ON p.name = o.pack WHERE p.kind = 'builder' "
                          "ORDER BY o.ord").fetchall()
    finally:
        db.close()
    lines = ["pyrealium-world-v3"]
    for row in rows:
        fields = [_bare(v) for v in row[:4]] + [str(v) for v in row[4:11]] + [row[11] if row[11] is not None else "-"]
        lines.append("\t".join(fields))
    return "\n".join(lines) + "\n"


def write_world(world_path, text, save=True):
    """Replaces the builder's placements with the rows of world text (v2 or v3); a placement's
    captured name (from an older world) is kept by its identity. `save` exports the dump."""
    lines = [line for line in text.splitlines() if line and not line.startswith("#")]
    header, rows = (lines[0] if lines else "pyrealium-world-v2"), lines[1:]
    if header not in ("pyrealium-world-v1", "pyrealium-world-v2", "pyrealium-world-v3", "pyrealium-world-v4"):
        raise ValueError("Unsupported world text header")
    db = _connect(world_path)
    try:
        names = {iid.lower(): name for iid, name in db.execute(
            "SELECT iid, name FROM world_object WHERE pack = ? AND name IS NOT NULL", (BUILDER,))}
        # After every shipped pack (content_db::overlay::BUILDER_ORD): this database holds none.
        db.execute("INSERT OR IGNORE INTO world_pack (name, ord, kind, source) VALUES (?, 1000, 'builder', "
                   "'the world builder''s placements')", (BUILDER,))
        db.execute("DELETE FROM world_object WHERE pack = ?", (BUILDER,))
        for ord_, line in enumerate(rows):
            f = line.split("\t")
            if header == "pyrealium-world-v1":
                raise ValueError("Open a pyrealium-world-v1 file once with the server (it converts it)")
            scale = f[11] if len(f) > 11 and f[11] != "-" else None
            name = f[12] if len(f) > 12 and f[12] != "-" else names.get("0x" + f[0].lower())
            db.execute("INSERT INTO world_object (pack, iid, ord, entity, visual, cell, x, y, z, qw, qx, qy, qz, scale, name, source) "
                       "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'the world builder')",
                       (BUILDER, "0x" + f[0], ord_, "0x" + f[1], "0x" + f[2], "0x" + f[3], *f[4:11], scale, name))
        db.commit()
    finally:
        db.close()
    if save:
        export(world_path)


def _field(value):
    if value is None:
        return "-"
    if isinstance(value, float):
        return repr(value) if value != int(value) else f"{value:.1f}"
    return str(value)


def rules_text(world_path):
    """The builder's creature rules as rules text (`pyrealium-world-rules-v1`)."""
    db = _connect(world_path)
    try:
        out = ["pyrealium-world-rules-v1"]
        for kind, query in (
                ("stats", "SELECT target, health, damage, armor, level, xp, gold, interval_ms, range, leash, respawn_ms "
                          "FROM creature_stats ORDER BY target"),
                ("ai", "SELECT target, perception, homesick, wandering, wander_range, wander_probability, aggressive, "
                       "idle_only, loot_proof FROM creature_ai ORDER BY target"),
                ("loot", "SELECT target, weight, items FROM creature_loot ORDER BY target, ord"),
                ("patrol", "SELECT actor, pause_ms, points FROM patrol ORDER BY actor")):
            for row in db.execute(query):
                out.append("\t".join([kind, _bare(row[0])] + [_field(v) for v in row[1:]]))
        for gid, capacity, regen, members in db.execute(
                "SELECT g.id, g.capacity, g.regen_ms, group_concat(substr(m.placement, 3) || coalesce(':' || m.chance, ''), ',') FROM spawn_group g "
                "JOIN spawn_group_member m ON m.spawn_group = g.id GROUP BY g.id ORDER BY g.id"):
            out.append("\t".join(["group", _bare(gid), _field(capacity), _field(regen), members]))
        return "\n".join(out) + "\n"
    finally:
        db.close()


def write_rules(world_path, text, save=True):
    """Replaces the builder's creature rules with the rows of rules text. `save` exports the dump."""
    db = _connect(world_path)
    try:
        db.executescript("DELETE FROM spawn_group_member; DELETE FROM spawn_group; DELETE FROM patrol; "
                         "DELETE FROM creature_loot; DELETE FROM creature_ai; DELETE FROM creature_stats;")
        value = lambda f, i: None if i >= len(f) or f[i] in ("-", "") else f[i]
        loot_ord = {}
        for line in text.splitlines()[1:]:
            if not line or line.startswith("#"):
                continue
            f = line.split("\t")
            src = "the world builder"
            if f[0] == "stats":
                db.execute("INSERT INTO creature_stats (target, health, damage, armor, level, xp, gold, interval_ms, range, leash, "
                           "respawn_ms, source) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)", ["0x" + f[1]] + [value(f, i) for i in range(2, 12)] + [src])
            elif f[0] == "ai":
                db.execute("INSERT INTO creature_ai (target, perception, homesick, wandering, wander_range, wander_probability, "
                           "aggressive, idle_only, loot_proof, source) VALUES (?,?,?,?,?,?,?,?,?,?)",
                           ["0x" + f[1]] + [value(f, i) for i in range(2, 10)] + [src])
            elif f[0] == "loot":
                loot_ord[f[1]] = loot_ord.get(f[1], 0) + 1
                db.execute("INSERT INTO creature_loot (target, ord, weight, items, source) VALUES (?,?,?,?,?)",
                           ("0x" + f[1], loot_ord[f[1]], f[2], f[3], src))
            elif f[0] == "patrol":
                db.execute("INSERT INTO patrol (actor, pause_ms, points, source) VALUES (?,?,?,?)", ("0x" + f[1], f[2], f[3], src))
            elif f[0] == "group":
                db.execute("INSERT INTO spawn_group (id, capacity, regen_ms, source) VALUES (?,?,?,?)", ("0x" + f[1], f[2], value(f, 3), src))
                for member in f[4].split(","):
                    member, _, chance = member.partition(":")
                    db.execute("INSERT INTO spawn_group_member (spawn_group, placement, chance, source) VALUES (?,?,?,?)",
                               ("0x" + f[1], "0x" + member, chance or None, src))
            else:
                raise ValueError(f"unrecognized rule row `{f[0]}`")
        db.commit()
    finally:
        db.close()
    if save:
        export(world_path)


def tombstones(world_path):
    """The prepared placements this world removes (16-digit lower-case ids)."""
    db = _connect(world_path)
    try:
        return {_bare(iid).lower() for (iid,) in db.execute("SELECT iid FROM world_tombstone")}
    finally:
        db.close()


def set_tombstones(world_path, add=(), remove=()):
    """Removes prepared placements from this world (`add`), or brings them back (`remove`)."""
    db = _connect(world_path)
    try:
        for iid in add:
            db.execute("INSERT OR IGNORE INTO world_tombstone (iid, source) VALUES (?, 'removed in the world builder')", ("0x" + _bare(iid).lower(),))
        for iid in remove:
            db.execute("DELETE FROM world_tombstone WHERE lower(iid) = ?", ("0x" + _bare(iid).lower(),))
        db.commit()
    finally:
        db.close()
    export(world_path)


def open_in_tool(path):
    """Opens the database in the system's SQLite tool."""
    if sys.platform.startswith("win"):
        os.startfile(path)  # noqa: S606 - the user's own file in their own tool
    elif sys.platform == "darwin":
        subprocess.Popen(["open", str(path)])
    else:
        subprocess.Popen(["xdg-open", str(path)])
