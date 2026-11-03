-- Pyrealium schema 1 (SQLite). STRICT tables: SQLite enforces each column's declared type.
-- u64 identities and DIDs are stored as their i64 bit pattern (item ids read back negative).
-- Booleans are 0/1 integers; times are Unix seconds (UTC).

-- Accounts: name is the canonical lowercase login name ([a-z0-9_]{1,32}).
CREATE TABLE accounts (
    id            INTEGER PRIMARY KEY,
    name          TEXT NOT NULL UNIQUE
                  CHECK (length(name) BETWEEN 1 AND 32 AND name NOT GLOB '*[^a-z0-9_]*'),
    password_hash TEXT NOT NULL,                     -- Argon2id PHC string (algorithm, salt, hash)
    created_at    INTEGER NOT NULL DEFAULT (unixepoch()),
    last_login_at INTEGER
) STRICT;

-- One-use game login tickets; the token itself is never stored, only its SHA-256.
CREATE TABLE login_tickets (
    token_hash BLOB PRIMARY KEY,
    account    TEXT NOT NULL,
    ip         TEXT NOT NULL,
    expires_at INTEGER NOT NULL,
    used_at    INTEGER
) STRICT;
CREATE INDEX login_tickets_expiry ON login_tickets (expires_at);

-- Characters. Scalar progression and placement are columns, so an offline character can be
-- read or credited with plain SQL; nested per-character books are JSON text.
CREATE TABLE characters (
    id                INTEGER PRIMARY KEY,
    account           TEXT NOT NULL,
    name              TEXT NOT NULL,
    name_key          TEXT NOT NULL UNIQUE,          -- lower(name): names are unique ignoring case
    deleted           INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1)),
    instance          INTEGER NOT NULL DEFAULT 0,    -- avatar lifetime stamp
    xp                INTEGER NOT NULL,
    available_xp      INTEGER NOT NULL,
    gold              INTEGER NOT NULL,
    health            INTEGER NOT NULL,
    vigor             INTEGER NOT NULL,
    cell              INTEGER,                       -- last saved placement (NULL: never placed)
    x                 REAL,
    y                 REAL,
    z                 REAL,
    heading           INTEGER,
    item_high_water   INTEGER NOT NULL,
    training_kit      INTEGER NOT NULL CHECK (training_kit IN (0, 1)),
    training_supplies INTEGER NOT NULL CHECK (training_supplies IN (0, 1)),
    starter_gear      INTEGER NOT NULL CHECK (starter_gear IN (0, 1)),
    creation          TEXT,                          -- exact client creation request (JSON)
    binding           TEXT,                          -- lifestone placement (JSON)
    ui                BLOB,                          -- native UI settings bytes
    gameplay_options  BLOB,                          -- native gameplay option bytes
    hotbar            TEXT NOT NULL,                 -- JSON
    skills            TEXT NOT NULL,                 -- JSON
    professions       TEXT NOT NULL,                 -- JSON
    quests            TEXT NOT NULL,                 -- JSON
    tradeskills       TEXT,                          -- tradeskills.rs progress line
    saved_at          INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;
CREATE INDEX characters_account ON characters (account);

-- Items owned by a character. The primary key makes an item id unique across every character.
CREATE TABLE items (
    id             INTEGER PRIMARY KEY,
    character_id   INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    definition     INTEGER NOT NULL,
    visual         INTEGER NOT NULL,
    quantity       INTEGER NOT NULL,
    stack_limit    INTEGER NOT NULL,
    equipment_mask INTEGER NOT NULL,
    equipped       INTEGER NOT NULL CHECK (equipped IN (0, 1)),  -- 0: bag slot; 1: equipment slot
    slot           INTEGER NOT NULL,
    roll           TEXT                              -- persisted equipment roll (JSON), if any
) STRICT;
CREATE INDEX items_character ON items (character_id);

-- Items lying in the world.
CREATE TABLE ground_items (
    id      INTEGER PRIMARY KEY,
    item    TEXT NOT NULL,                           -- JSON
    cell    INTEGER NOT NULL,
    x       REAL NOT NULL,
    y       REAL NOT NULL,
    z       REAL NOT NULL,
    heading INTEGER NOT NULL
) STRICT;

-- Small world-wide values (clock origin, ...), JSON.
CREATE TABLE world_state (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

-- GUN friend / ignore lists, by account (JSON).
CREATE TABLE gun_users (
    account TEXT PRIMARY KEY,
    lists   TEXT NOT NULL
) STRICT;

-- Prepared content (written by content preparation, read at server start). One row.
CREATE TABLE content_meta (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    input_hash  TEXT NOT NULL,
    version     INTEGER NOT NULL,
    report      TEXT NOT NULL,                       -- JSON
    prepared_at INTEGER NOT NULL DEFAULT (unixepoch())
) STRICT;
-- Content files produced by preparation (world placements TSV, quest and emote tables), by name.
CREATE TABLE content_files (
    name TEXT PRIMARY KEY,
    body TEXT NOT NULL
) STRICT;
