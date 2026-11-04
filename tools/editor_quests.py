"""The world builder's quest database: NAME.quests.sql beside the content file NAME.tsv.

It is the server's quest content (`quest_emotes::db`): the live link loads it from the server
the first time (what the server runs), pushes it with the world and rules on every save, and
the server checks it, swaps it in and saves it beside its overlay. It is edited as
NAME.quests.sqlite in any SQLite tool (DB Browser for SQLite, sqlite3): **Edit quests** makes
that database from the dump, and a save in the tool pushes it live (the editor sends the
database's SQL dump; the server returns its canonical text, which becomes the new
NAME.quests.sql). The dump stays the file to keep or check in.
"""
import os
from pathlib import Path
import sqlite3
import subprocess
import sys

# A quest database dump is a few hundred KiB; refuse anything absurd.
LIMIT = 8 * 1024 * 1024


def quests_path(world_path):
    path = Path(world_path)
    return path.with_name(path.stem + ".quests.sql")


def quests_db_path(world_path):
    path = Path(world_path)
    return path.with_name(path.stem + ".quests.sqlite")


def mtime(path):
    try:
        return path.stat().st_mtime_ns
    except OSError:
        return None


def signature(world_path):
    """What changes when either quest file is saved."""
    return mtime(quests_path(world_path)), mtime(quests_db_path(world_path))


def read_dump(path):
    with path.open("rb") as file:
        data = file.read(LIMIT + 1)
    if len(data) > LIMIT:
        raise ValueError(f"{path.name} exceeds 8 MiB")
    return data.decode("utf-8")


def dump_database(path):
    """The SQL dump of an edited database, read only (the server makes it canonical)."""
    db = sqlite3.connect(f"{path.resolve().as_uri()}?mode=ro", uri=True)
    try:
        return "\n".join(db.iterdump()) + "\n"
    finally:
        db.close()


def quest_text(world_path):
    """The quests to push: the edited database when it is newer than the dump, else the dump,
    else None (none here yet: the live link fetches the server's)."""
    sql, database = quests_path(world_path), quests_db_path(world_path)
    sql_time, db_time = mtime(sql), mtime(database)
    if db_time is not None and (sql_time is None or db_time > sql_time):
        return dump_database(database)
    if sql_time is not None:
        return read_dump(sql)
    return None


def save_dump(world_path, text):
    """Writes the server's canonical dump as NAME.quests.sql (only when it differs). When the
    database was the newer file (its rows were pushed), it stays the newer one: it holds the
    same rows, and a tool may have it open, so Edit quests must not rebuild it."""
    path, database = quests_path(world_path), quests_db_path(world_path)
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


def build_database(world_path):
    """NAME.quests.sqlite from NAME.quests.sql, unless the database is already the newer one."""
    sql, database = quests_path(world_path), quests_db_path(world_path)
    if not sql.exists():
        raise ValueError(f"No {sql.name} yet: connect the live server once to load its quests")
    if database.exists() and mtime(database) >= mtime(sql):
        return database
    temporary = database.with_name(database.name + ".tmp")
    if temporary.exists():
        temporary.unlink()
    db = sqlite3.connect(temporary)
    try:
        db.executescript(read_dump(sql))
        db.commit()
    finally:
        db.close()
    os.replace(temporary, database)
    return database


def open_in_tool(path):
    """Opens the database in the system's SQLite tool."""
    if sys.platform.startswith("win"):
        os.startfile(path)  # noqa: S606 - the user's own file in their own tool
    elif sys.platform == "darwin":
        subprocess.Popen(["open", str(path)])
    else:
        subprocess.Popen(["xdg-open", str(path)])
