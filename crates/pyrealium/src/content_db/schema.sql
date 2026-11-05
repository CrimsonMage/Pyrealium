-- The content database's tables, in dump order (crates/pyrealium/src/content_db.rs). Hex identifiers
-- are text (`0x4700157d`), as they are written everywhere else; blank optional fields are NULL.
CREATE TABLE IF NOT EXISTS quest (
  name TEXT PRIMARY KEY CHECK (name <> ''),
  journal_id TEXT NOT NULL CHECK (journal_id GLOB '0x[0-9a-fA-F]*'),
  title_table TEXT NOT NULL CHECK (title_table GLOB '0x[0-9a-fA-F]*'),
  title_key TEXT NOT NULL CHECK (title_key GLOB '0x[0-9a-fA-F]*'),
  title TEXT NOT NULL CHECK (title <> ''),
  goal INTEGER CHECK (goal >= 0),
  credit TEXT NOT NULL DEFAULT 'killer' CHECK (credit IN ('killer', 'fellowship')),
  min_level INTEGER CHECK (min_level >= 0),
  time_limit_seconds INTEGER CHECK (time_limit_seconds > 0),
  max_solves INTEGER CHECK (max_solves > 0),
  repeat_seconds INTEGER CHECK (repeat_seconds > 0),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS quest_stage (
  quest TEXT NOT NULL REFERENCES quest(name) ON UPDATE CASCADE ON DELETE CASCADE,
  stage INTEGER NOT NULL CHECK (stage BETWEEN 0 AND 255),
  completed INTEGER NOT NULL DEFAULT 0 CHECK (completed IN (0, 1)),
  journal_text TEXT NOT NULL CHECK (journal_text <> ''),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (quest, stage)
);
CREATE TABLE IF NOT EXISTS emote_set (
  name TEXT PRIMARY KEY CHECK (name <> ''),
  priority INTEGER NOT NULL,
  owner_entity TEXT CHECK (owner_entity GLOB '0x47[0-9a-fA-F]*' OR owner_entity GLOB '[a-z]*'),
  owner_iid TEXT CHECK (owner_iid GLOB '0x[0-9a-fA-F]*' OR owner_iid GLOB '[a-z]*'),
  trigger TEXT NOT NULL CHECK (trigger IN ('Use', 'Give', 'Kill', 'ItemUsedOn', 'Enter', 'Crafted', 'EscortLost',
    'QuestSuccess', 'QuestFailure', 'Goto', 'PlayerDeath', 'QuestExpired', 'Logout', 'EscortAbandoned',
    'EnterWorld', 'UseItem', 'Bind', 'Travel', 'Ask', 'Talk')),
  item TEXT CHECK (item GLOB '0x[0-9a-fA-F]*' OR item GLOB '[a-z]*'),
  owner_slots TEXT REFERENCES instance_group(name) ON UPDATE CASCADE,
  phrase TEXT CHECK (phrase IS NULL OR (phrase <> '' AND trigger = 'Ask')),
  quest TEXT REFERENCES quest(name) ON UPDATE CASCADE,
  stage_min INTEGER CHECK (stage_min BETWEEN 0 AND 255),
  stage_max INTEGER CHECK (stage_max BETWEEN 0 AND 255),
  range REAL CHECK (range > 0),
  probability REAL CHECK (probability > 0 AND probability <= 1),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS emote_action (
  emote_set TEXT NOT NULL REFERENCES emote_set(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  action TEXT NOT NULL CHECK (action <> ''),
  delay_ms INTEGER NOT NULL DEFAULT 0 CHECK (delay_ms BETWEEN 0 AND 3600000),
  p1 TEXT NOT NULL DEFAULT '',
  p2 TEXT NOT NULL DEFAULT '',
  p3 TEXT NOT NULL DEFAULT '',
  p4 TEXT NOT NULL DEFAULT '',
  p5 TEXT NOT NULL DEFAULT '',
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (emote_set, ord)
);
CREATE TABLE IF NOT EXISTS alias (
  name TEXT PRIMARY KEY CHECK (name GLOB '[a-z]*' AND name NOT GLOB '*[^a-z0-9_]*'),
  kind TEXT NOT NULL CHECK (kind IN ('entity', 'effect', 'hex', 'placement', 'entities', 'objects')),
  value TEXT NOT NULL CHECK (value <> ''),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS quest_counter (
  quest TEXT NOT NULL REFERENCES quest(name) ON UPDATE CASCADE ON DELETE CASCADE,
  name TEXT NOT NULL CHECK (name GLOB '[a-z]*' AND name NOT GLOB '*[^a-z0-9_]*'),
  idx INTEGER NOT NULL CHECK (idx BETWEEN 0 AND 255),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (quest, name),
  UNIQUE (quest, idx)
);
CREATE TABLE IF NOT EXISTS template (
  name TEXT PRIMARY KEY CHECK (name GLOB '[a-z]*' AND name NOT GLOB '*[^a-z0-9_]*'),
  doc TEXT NOT NULL DEFAULT '',
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS template_param (
  template TEXT NOT NULL REFERENCES template(name) ON UPDATE CASCADE ON DELETE CASCADE,
  name TEXT NOT NULL CHECK (name GLOB '[a-z]*' AND name NOT GLOB '*[^a-z0-9_]*'),
  ord INTEGER NOT NULL DEFAULT 0,
  doc TEXT NOT NULL DEFAULT '',
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (template, name)
);
CREATE TABLE IF NOT EXISTS template_set (
  template TEXT NOT NULL REFERENCES template(name) ON UPDATE CASCADE ON DELETE CASCADE,
  name TEXT NOT NULL CHECK (name <> '' AND name NOT GLOB '*.*'),
  priority INTEGER NOT NULL DEFAULT 0,
  owner_entity TEXT, owner_iid TEXT, trigger TEXT NOT NULL, item TEXT, quest TEXT,
  stage_min TEXT, stage_max TEXT, range TEXT, probability TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (template, name)
);
CREATE TABLE IF NOT EXISTS template_action (
  template TEXT NOT NULL,
  emote_set TEXT NOT NULL,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  action TEXT NOT NULL CHECK (action <> ''),
  delay_ms TEXT NOT NULL DEFAULT '0',
  p1 TEXT NOT NULL DEFAULT '', p2 TEXT NOT NULL DEFAULT '', p3 TEXT NOT NULL DEFAULT '',
  p4 TEXT NOT NULL DEFAULT '', p5 TEXT NOT NULL DEFAULT '',
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (template, emote_set, ord),
  FOREIGN KEY (template, emote_set) REFERENCES template_set(template, name) ON UPDATE CASCADE ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS template_use (
  name TEXT PRIMARY KEY CHECK (name <> '' AND name NOT GLOB '*.*'),
  template TEXT NOT NULL REFERENCES template(name) ON UPDATE CASCADE,
  priority INTEGER NOT NULL DEFAULT 0,
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS template_arg (
  use_name TEXT NOT NULL REFERENCES template_use(name) ON UPDATE CASCADE ON DELETE CASCADE,
  param TEXT NOT NULL,
  value TEXT NOT NULL DEFAULT '',
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (use_name, param)
);
CREATE TABLE IF NOT EXISTS instance_group (
  name TEXT PRIMARY KEY CHECK (name GLOB '[a-z]*' AND name NOT GLOB '*[^a-z0-9_]*'),
  doc TEXT NOT NULL DEFAULT '',
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS instance_lane (
  instance_group TEXT NOT NULL REFERENCES instance_group(name) ON UPDATE CASCADE ON DELETE CASCADE,
  lane INTEGER NOT NULL CHECK (lane BETWEEN 0 AND 255),
  entry TEXT NOT NULL CHECK (entry <> ''),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (instance_group, lane)
);
CREATE TABLE IF NOT EXISTS spawn_slot (
  name TEXT NOT NULL CHECK (name GLOB '[a-z]*' AND name NOT GLOB '*[^a-z0-9_]*'),
  instance_group TEXT NOT NULL,
  lane INTEGER NOT NULL,
  iid TEXT NOT NULL CHECK (iid GLOB '0x[0-9a-fA-F]*'),
  arrival TEXT,
  step INTEGER CHECK (step BETWEEN 0 AND 255),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (name, lane),
  FOREIGN KEY (instance_group, lane) REFERENCES instance_lane(instance_group, lane) ON UPDATE CASCADE ON DELETE CASCADE
);
CREATE TABLE IF NOT EXISTS quest_recipe (
  recipe TEXT PRIMARY KEY CHECK (recipe GLOB '0x[0-9a-fA-F]*'),
  ingredient TEXT NOT NULL,
  quantity INTEGER NOT NULL CHECK (quantity BETWEEN 1 AND 65535),
  output TEXT NOT NULL,
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS placement_hint (
  entity TEXT PRIMARY KEY CHECK (entity GLOB '0x47[0-9a-fA-F]*'),
  what TEXT NOT NULL CHECK (what <> ''),
  name TEXT NOT NULL,
  north REAL,
  east REAL,
  cell TEXT CHECK (cell GLOB '0x[0-9a-fA-F]*'),
  x REAL,
  y REAL,
  quests TEXT NOT NULL,
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS quest_item (
  entity TEXT PRIMARY KEY CHECK (entity GLOB '0x47[0-9a-fA-F]*'),
  visual TEXT CHECK (visual GLOB '0x1f[0-9a-fA-F]*'),
  stack_limit INTEGER CHECK (stack_limit BETWEEN 1 AND 65535),
  equipment_mask INTEGER CHECK (equipment_mask BETWEEN 0 AND 2047),
  physical TEXT CHECK (physical GLOB '0x47[0-9a-fA-F]*'),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS world_pack (
  name TEXT PRIMARY KEY CHECK (name <> ''),
  ord INTEGER NOT NULL UNIQUE CHECK (ord >= 0),
  kind TEXT NOT NULL CHECK (kind IN ('observed', 'authored', 'capture', 'recovered', 'inferred', 'builder')),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS world_evidence (
  name TEXT PRIMARY KEY CHECK (name <> ''),
  body TEXT NOT NULL CHECK (json_valid(body)),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS world_object (
  pack TEXT NOT NULL REFERENCES world_pack(name) ON UPDATE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  iid TEXT NOT NULL CHECK (iid GLOB '0x[0-9a-fA-F]*' AND length(iid) = 18),
  entity TEXT NOT NULL CHECK (entity GLOB '0x[0-9a-fA-F]*' AND length(entity) = 10),
  visual TEXT NOT NULL CHECK (visual GLOB '0x[0-9a-fA-F]*' AND length(visual) = 10),
  cell TEXT NOT NULL CHECK (cell GLOB '0x[0-9a-fA-F]*' AND length(cell) = 10),
  x TEXT NOT NULL CHECK (x <> ''),
  y TEXT NOT NULL CHECK (y <> ''),
  z TEXT NOT NULL CHECK (z <> ''),
  qw TEXT NOT NULL CHECK (qw <> ''),
  qx TEXT NOT NULL CHECK (qx <> ''),
  qy TEXT NOT NULL CHECK (qy <> ''),
  qz TEXT NOT NULL CHECK (qz <> ''),
  scale TEXT CHECK (scale <> ''),
  name TEXT CHECK (length(name) <= 256),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT,
  PRIMARY KEY (pack, iid),
  UNIQUE (pack, ord)
);
CREATE TABLE IF NOT EXISTS creature_stats (
  target TEXT PRIMARY KEY CHECK (target GLOB '0x47[0-9a-fA-F]*' OR target GLOB '0x8140[0-9a-fA-F]*'),
  health INTEGER, damage INTEGER, armor INTEGER, level INTEGER, xp INTEGER, gold INTEGER,
  interval_ms INTEGER, range REAL, leash REAL, respawn_ms INTEGER,
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS creature_ai (
  target TEXT PRIMARY KEY CHECK (target GLOB '0x47[0-9a-fA-F]*' OR target GLOB '0x8140[0-9a-fA-F]*'),
  perception REAL, homesick REAL, wandering INTEGER CHECK (wandering IN (0, 1)), wander_range REAL,
  wander_probability REAL, aggressive INTEGER CHECK (aggressive IN (0, 1)), idle_only INTEGER CHECK (idle_only IN (0, 1)),
  loot_proof INTEGER CHECK (loot_proof IN (0, 1)),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS creature_loot (
  target TEXT NOT NULL CHECK (target GLOB '0x47[0-9a-fA-F]*' OR target GLOB '0x8140[0-9a-fA-F]*'),
  ord INTEGER NOT NULL CHECK (ord >= 1),
  weight INTEGER NOT NULL CHECK (weight >= 1),
  items TEXT NOT NULL,
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (target, ord)
);
CREATE TABLE IF NOT EXISTS patrol (
  actor TEXT PRIMARY KEY CHECK (actor GLOB '0x8140[0-9a-fA-F]*'),
  pause_ms INTEGER NOT NULL CHECK (pause_ms >= 0),
  points TEXT NOT NULL CHECK (points <> ''),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS spawn_group (
  id TEXT PRIMARY KEY CHECK (id GLOB '0x[0-9a-fA-F]*'),
  capacity INTEGER NOT NULL CHECK (capacity >= 1),
  regen_ms INTEGER CHECK (regen_ms >= 0),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS spawn_group_member (
  spawn_group TEXT NOT NULL REFERENCES spawn_group(id) ON UPDATE CASCADE ON DELETE CASCADE,
  placement TEXT NOT NULL CHECK (placement GLOB '0x8140[0-9a-fA-F]*'),
  -- The member's spawn chance (ACE SelectAProfile), 0 < chance <= 1; NULL fills whenever free.
  chance TEXT CHECK (chance IS NULL OR (CAST(chance AS REAL) > 0 AND CAST(chance AS REAL) <= 1)),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> ''),
  PRIMARY KEY (spawn_group, placement)
);
CREATE TABLE IF NOT EXISTS world_tombstone (
  iid TEXT PRIMARY KEY CHECK (iid GLOB '0x8140[0-9a-fA-F]*' AND length(iid) = 18),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
-- Content files as rows (`content_db::files`). A file is its header (the lines before its first
-- row: the format line and its comments, kept as written, since some readers check them), its rows
-- in a table named for what they hold (each field exactly as the file wrote it; NULL where a row
-- has fewer fields; `note` is the comment lines written before the row), and its footer. A file
-- whose rows are a world pack's names the pack instead. Documents are JSON files kept whole.
CREATE TABLE IF NOT EXISTS content_file (
  name TEXT PRIMARY KEY CHECK (name <> ''),
  rows_in TEXT NOT NULL,
  header TEXT NOT NULL,
  footer TEXT NOT NULL DEFAULT '',
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS content_document (
  name TEXT PRIMARY KEY CHECK (name GLOB '*.json'),
  body TEXT NOT NULL CHECK (json_valid(body)),
  family TEXT NOT NULL DEFAULT 'hand',
  source TEXT NOT NULL CHECK (source <> '')
);
CREATE TABLE IF NOT EXISTS creature_combat (
  file TEXT NOT NULL REFERENCES content_file(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  entity TEXT NOT NULL CHECK (entity <> ''),
  health TEXT,
  damage TEXT,
  interval_ms TEXT,
  armor TEXT,
  xp TEXT,
  level TEXT,
  variance TEXT,
  attack TEXT,
  defense TEXT,
  perception TEXT,
  homesick TEXT,
  damage_type TEXT,
  max_vigor TEXT,
  note TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  PRIMARY KEY (file, ord)
);
CREATE TABLE IF NOT EXISTS creature_ai_profile (
  file TEXT NOT NULL REFERENCES content_file(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  entity TEXT NOT NULL CHECK (entity <> ''),
  perception TEXT,
  homesick TEXT,
  wandering TEXT,
  wander_range TEXT,
  wander_probability TEXT,
  friend_table TEXT,
  free_attacking TEXT,
  idle_only TEXT,
  loot_profile TEXT,
  loot_proof TEXT,
  note TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  PRIMARY KEY (file, ord)
);
CREATE TABLE IF NOT EXISTS creature_skill (
  file TEXT NOT NULL REFERENCES content_file(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  entity TEXT NOT NULL CHECK (entity <> ''),
  skills TEXT,
  note TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  PRIMARY KEY (file, ord)
);
CREATE TABLE IF NOT EXISTS definition (
  file TEXT NOT NULL REFERENCES content_file(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  entity TEXT NOT NULL CHECK (entity <> ''),
  visual TEXT,
  label TEXT,
  note TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  PRIMARY KEY (file, ord)
);
CREATE TABLE IF NOT EXISTS loot_item (
  file TEXT NOT NULL REFERENCES content_file(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  entity TEXT NOT NULL CHECK (entity <> ''),
  visual TEXT,
  name TEXT,
  note TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  PRIMARY KEY (file, ord)
);
CREATE TABLE IF NOT EXISTS observed_loot (
  file TEXT NOT NULL REFERENCES content_file(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  monster TEXT NOT NULL CHECK (monster <> ''),
  profile TEXT,
  observed_count TEXT,
  items TEXT,
  note TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  PRIMARY KEY (file, ord)
);
CREATE TABLE IF NOT EXISTS encounter_rule (
  file TEXT NOT NULL REFERENCES content_file(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  kind TEXT NOT NULL CHECK (kind <> ''),
  v1 TEXT,
  v2 TEXT,
  v3 TEXT,
  v4 TEXT,
  v5 TEXT,
  v6 TEXT,
  v7 TEXT,
  v8 TEXT,
  note TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  PRIMARY KEY (file, ord)
);
CREATE TABLE IF NOT EXISTS protected_entity (
  file TEXT NOT NULL REFERENCES content_file(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  entity TEXT NOT NULL CHECK (entity <> ''),
  note TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  PRIMARY KEY (file, ord)
);
CREATE TABLE IF NOT EXISTS crafting_level (
  file TEXT NOT NULL REFERENCES content_file(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  skill TEXT NOT NULL CHECK (skill <> ''),
  level TEXT,
  curve_xp TEXT,
  learning_limit TEXT,
  crafting_xp TEXT,
  note TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  PRIMARY KEY (file, ord)
);
CREATE TABLE IF NOT EXISTS skill (
  file TEXT NOT NULL REFERENCES content_file(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  id TEXT NOT NULL CHECK (id <> ''),
  record TEXT,
  digest TEXT,
  fingerprint TEXT,
  race TEXT,
  level TEXT,
  credits TEXT,
  cap TEXT,
  curve TEXT,
  kind TEXT,
  required TEXT,
  forbidden TEXT,
  note TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  PRIMARY KEY (file, ord)
);
CREATE TABLE IF NOT EXISTS skill_advancement (
  file TEXT NOT NULL REFERENCES content_file(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  curve TEXT NOT NULL CHECK (curve <> ''),
  fingerprint TEXT,
  digest TEXT,
  xp TEXT,
  note TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  PRIMARY KEY (file, ord)
);
CREATE TABLE IF NOT EXISTS skill_effect (
  file TEXT NOT NULL REFERENCES content_file(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  skill TEXT NOT NULL CHECK (skill <> ''),
  record TEXT,
  kind TEXT,
  reset_s TEXT,
  vigor TEXT,
  range TEXT,
  targets TEXT,
  right_hand TEXT,
  left_hand TEXT,
  story TEXT,
  fx TEXT,
  damage_table TEXT,
  effects TEXT,
  note TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  PRIMARY KEY (file, ord)
);
CREATE TABLE IF NOT EXISTS skill_missile (
  file TEXT NOT NULL REFERENCES content_file(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  skill TEXT NOT NULL CHECK (skill <> ''),
  record TEXT,
  hook TEXT,
  entity TEXT,
  note TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  PRIMARY KEY (file, ord)
);
CREATE TABLE IF NOT EXISTS skill_travel (
  file TEXT NOT NULL REFERENCES content_file(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  skill TEXT NOT NULL CHECK (skill <> ''),
  class TEXT,
  destinations TEXT,
  note TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  PRIMARY KEY (file, ord)
);
CREATE TABLE IF NOT EXISTS item_appearance (
  file TEXT NOT NULL REFERENCES content_file(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  entity TEXT NOT NULL CHECK (entity <> ''),
  visual TEXT,
  instances_pre2012 TEXT,
  instances_2012_2017 TEXT,
  mods TEXT,
  note TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  PRIMARY KEY (file, ord)
);
CREATE TABLE IF NOT EXISTS portal_route (
  file TEXT NOT NULL REFERENCES content_file(name) ON UPDATE CASCADE ON DELETE CASCADE,
  ord INTEGER NOT NULL CHECK (ord >= 0),
  portal TEXT NOT NULL CHECK (portal <> ''),
  entity TEXT,
  cell TEXT,
  x TEXT,
  y TEXT,
  z TEXT,
  dest_cell TEXT,
  dest_x TEXT,
  dest_y TEXT,
  dest_z TEXT,
  heading TEXT,
  level TEXT,
  note TEXT,
  family TEXT NOT NULL DEFAULT 'hand',
  PRIMARY KEY (file, ord)
);
