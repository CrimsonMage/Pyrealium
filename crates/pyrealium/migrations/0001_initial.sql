-- Pyrealium schema 1. Three schemas after ACE's auth / shard / world split.
-- u64 identities and DIDs are stored as their i64 bit pattern (item ids read back negative).

CREATE SCHEMA auth;
CREATE SCHEMA shard;
CREATE SCHEMA world;

-- Accounts: name is the canonical lowercase login name ([a-z0-9_]{1,32}).
CREATE TABLE auth.accounts (
    id            bigserial PRIMARY KEY,
    name          text NOT NULL UNIQUE CHECK (name ~ '^[a-z0-9_]{1,32}$'),
    password_hash text NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    last_login_at timestamptz
);

-- One-use game login tickets; the token itself is never stored, only its SHA-256.
CREATE TABLE auth.login_tickets (
    token_hash bytea PRIMARY KEY,
    account    text NOT NULL,
    ip         inet NOT NULL,
    expires_at timestamptz NOT NULL,
    used_at    timestamptz
);
CREATE INDEX login_tickets_expiry ON auth.login_tickets (expires_at);

-- Characters. Scalar progression and placement are columns, so an offline character can be
-- read or credited with plain SQL; nested per-character books are JSONB.
CREATE TABLE shard.characters (
    id                bigint PRIMARY KEY,
    account           text NOT NULL,
    name              text NOT NULL,
    name_key          text NOT NULL UNIQUE,          -- lower(name): names are unique ignoring case
    deleted           boolean NOT NULL DEFAULT false,
    instance          integer NOT NULL DEFAULT 0,    -- avatar lifetime stamp
    xp                bigint NOT NULL,
    available_xp      bigint NOT NULL,
    gold              bigint NOT NULL,
    health            bigint NOT NULL,
    vigor             bigint NOT NULL,
    cell              bigint,                        -- last saved placement (NULL: never placed)
    x                 real,
    y                 real,
    z                 real,
    heading           smallint,
    item_high_water   bigint NOT NULL,
    training_kit      boolean NOT NULL,
    training_supplies boolean NOT NULL,
    starter_gear      boolean NOT NULL,
    creation          jsonb,                         -- exact client creation request
    binding           jsonb,                         -- lifestone placement
    ui                bytea,                         -- native UI settings bytes
    gameplay_options  bytea,                         -- native gameplay option bytes
    hotbar            jsonb NOT NULL,
    skills            jsonb NOT NULL,
    professions       jsonb NOT NULL,
    quests            jsonb NOT NULL,
    tradeskills       text,                          -- tradeskills.rs progress line
    saved_at          timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX characters_account ON shard.characters (account);

-- Items owned by a character. The primary key makes an item id unique across every character.
CREATE TABLE shard.items (
    id             bigint PRIMARY KEY,
    character_id   bigint NOT NULL REFERENCES shard.characters (id) ON DELETE CASCADE,
    definition     bigint NOT NULL,
    visual         bigint NOT NULL,
    quantity       bigint NOT NULL,
    stack_limit    bigint NOT NULL,
    equipment_mask integer NOT NULL,
    equipped       boolean NOT NULL,                 -- false: bag slot; true: equipment slot
    slot           integer NOT NULL,
    roll           jsonb                             -- persisted equipment roll, if any
);
CREATE INDEX items_character ON shard.items (character_id);

-- Items lying in the world.
CREATE TABLE shard.ground_items (
    id   bigint PRIMARY KEY,
    item jsonb NOT NULL,
    cell bigint NOT NULL,
    x    real NOT NULL,
    y    real NOT NULL,
    z    real NOT NULL,
    heading smallint NOT NULL
);

-- Small world-wide values (clock origin, ...).
CREATE TABLE shard.world_state (
    key   text PRIMARY KEY,
    value jsonb NOT NULL
);

-- GUN friend / ignore lists, by account.
CREATE TABLE shard.gun_users (
    account text PRIMARY KEY,
    lists   jsonb NOT NULL
);

-- Prepared content (written by content preparation, read at server start).
CREATE TABLE world.prep_meta (
    id          boolean PRIMARY KEY DEFAULT true CHECK (id),
    input_hash  text NOT NULL,
    version     integer NOT NULL,
    report      jsonb NOT NULL,
    prepared_at timestamptz NOT NULL DEFAULT now()
);
-- Content files produced by preparation (world placements TSV, quest and emote tables), by name.
CREATE TABLE world.files (
    name text PRIMARY KEY,
    body text NOT NULL
);
