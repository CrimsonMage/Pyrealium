# Stationary world content

Game-data mode loads the bundled snapshot automatically, independent of the
working directory. Use the normal server options and:

```powershell
pyrealium serve --game-data PATH
```

`--content content/world.tsv` instead loads an editable external manifest.
`--content none` disables world content. Selection-only mode without game data
does not load placements. The compiled bundle changes when the server is rebuilt;
operator edits to an external manifest require the explicit file option and a
server restart. There is no hot reload.

`--content observed` selects the separately bundled 277-object observation pack:
the 247 table-derived placements plus 30 actors recovered from three supplied
packet captures. Six capture inputs were examined; the other three supplied no
additional supported placed actors. It includes Banderling Ravers/Thrashers, Idol/Surrogate Spawns,
Furs Turfna, eggs, chests and other world objects at their captured positions.
It does not populate the human starter dungeon or supply AI, trading, damage,
loot, quest interactions, death or respawn behavior. The regular default pack
continues to use only the table-derived placements.

## Regional wilderness population (2026-10-31)

`regional-content-v1.json` joins the archived AC2 wiki's regional creature lists
to Pyreal AC2UtilSQL ringway/town geography and installed native creature
templates. `regional-world-v1.tsv` and its ground-verification report add 3,495
accepted slots in 2,071 groups, across 21 Osteth/Omishan/Linvak regions. There are
326 placed native creature types; 329 eligible types were considered. Preparation
verifies catalog/manifest hashes and the installed entity/visual/cell records.

The separate `regional-creature-combat-v1.tsv` and `regional-creature-ai-v1.tsv`
enable native stats and perception/wandering/home behavior. Regional actors use
native stats even when the built-in development profile weakens the same
template for practice encounters. Explicit external encounter overrides still
apply. Generic variable loot and native trophy slots use the existing pipeline.

These are inferred generator locations: one candidate per landblock inside the
convex envelope of documented ringway anchors, with town and existing-content
exclusion distances, native dry-ground/contact/walkability checks, and one or two
simultaneous children. A wiki `group` label means elite/fellowship difficulty,
not a recovered spawn count. Regeneration is 180 seconds after death, with corpse
retirement acknowledged before a replacement appears. The `8140c00000` identity
prefix is reserved for this pack; random capture IDs sharing only `8140c0` are
ordinary existing content and retain priority.

Author with `tools/author_regional_population.py`, ground with the Rust
`regional_population` example, then run `tests/network/prepare_content.py` and rebuild.
Exact commands, source links, caveats and acceptance checks are in
[content recovery](../../docs/ADMIN_CONTENT_RECOVERY.md#regional-wilderness-pack-v1-2026-10-31).

## Knorr, Arramora and wilderness expansion (2026-11-01)

`wilderness-content-v1.json` supplements the regional pack with the archived
Knorr portal map, Arramora camp map and all 27 regional creature rosters.
`wilderness-world-v1.tsv` contains **2,553 additional creatures in 1,555 groups**,
using 443 native templates, including 180 with no prior world placement.
All 563 eligible roster types now occur somewhere in the combined world;
this does not establish coverage of every habitat or native variant. The
prepared production world contains 12,379 total objects.

Habitat restrictions distinguish Arramora's southern Thornlings, central golems,
northeastern Niffis and northern Olthoi. The specific Harvester quest places its
targets southwest. Foundry and Wilds solo/group areas and southern Gardens
families have separate constraints. Map envelopes, camp positions, one/two-child
capacities and 180-second refill are inferred. Native terrain checks and
town/content separation apply. The reserved identity prefix is `8140c10000`.

The combat and AI catalogs feed normal encounters and level-aware variable
loot, native trophy probabilities and recovered quantity limits. No generic
potion-only fallback is added. `wilderness-coverage-v1.json` accounts for all
2,024 native records with health/damage fields, including protected NPCs,
alternate templates, special encounters and unresolved habitats. The Olthoi
Soldier Grub's mound-dependent script is withheld. Ordinary single-kill quest
targets such as Drudge Robbers are not categorically excluded from this pack.

`hunting-content-v1.json` implements ten NPC hunts at eight speakers. Vreema's
Bone/Rime/Uncharneled Golem chain requires completing each previous hunt;
Gregor Adulen, Nola Vontag and Tankaru provide other Arramora hunts. Brelar
Marlage, Illiana Bain, Shurakhe and Endaraen Brightstar provide Foundry hunts.
Two missing Jethric speakers use `hunting-world-v1.tsv`, prefix `8140d2`.
All eight speakers remain noncombat NPCs. Level gates, ten-kill objectives,
seven-day deadlines, return rewards and documented repeat timers persist.
Kills before acceptance do not count; kills alone do not award the hunt XP.

The later retail quest IDs were absent from the recovered admin quest export.
These quests use authored IDs `4000fc01`–`4000fc0a`, summarized dialogue and
immutable `qh1`/`qk1` journal rules. Never rewrite released policy in place.
Old `q6` kill records retain their replay semantics. See
[sources, limits and regeneration commands](../../docs/ADMIN_CONTENT_RECOVERY.md#wilderness-and-npc-hunting-expansion-2026-11-01).

### Hunting expansion v2

`hunting-content-v2.json` adds 25 hunts (35 total, at 23 distinct native NPCs):
thirteen Ikeras quests, nine more Arramora quests offered in Linvak Tukal, and
three Foundry quests. It extends Gregor's Harvester quest into Worker Nymphs
and Drones, adds Groli's Crone/Thornling/Reaper chain and Eliza's Niffis chain,
and implements the Ikeras Spawn, Niffis/Nefane and Sclavus chains. Qualifying
variants come from the exact native quest-counter death effects; similarly
named Gauntlet actors do not count. Fixed XP and unlimited deadlines are
supported where the guide specifies them.

`hunting-world-v2.tsv` adds Furakhe at Jethric's Outpost, Ingara at Ikeras,
and guide-anchored Vigilant Spawn, Olthoi Worker Nymph and Dread Dark Mu-miyah
camps on Arramora. All eight placements passed native terrain/collision checks.
The camps use two simultaneous children and 120-second regeneration; those
capacities/timers are inferred, not recovered retail generators. NPCs remain
noncombat; creatures use native combat/AI and the existing loot pipeline.

New IDs are `4000fc0b`–`4000fc23`; new journal records are `qh2`/`qk2`.
Replay of `qh1`/`qk1` retains the original ten-quest catalog, so Gregor's old
repeat turn-ins cannot silently start a newly added chain. Neither released
catalog may be edited in place. The coverage audit includes all 35 hunts.
See [evidence and verification](../../docs/ADMIN_CONTENT_RECOVERY.md#hunting-quest-expansion-v2-2026-11-01).

## Human Shelter quest placements

`human-shelter-world.tsv` holds Rescue the Shelter Hold's placements in the human start
instance (`0299`) that no capture of the human Shelter has: the Menhir Shard (copied from the
identical `028D` copy's capture), and, inferred from the DAT dungeon, the Menhir Portal at the
second portal arch, training stones 7 to 9, the Shelter Sentry and Scout, and the Drudge
Diggers and Sentinels. `prepare_content.py --strip-training-placements` appends it after it strips the training kit's
placements and mirrors the captured `0298` Shelter into `0299` (`--no-shelter-quest` leaves it
out; without the strip it is not merged). The file's header gives the evidence for each row.

## Scripted quests as data

Rescue the Shelter Hold is four tables in the shape of ACEmulator's emote tables
(`src/quest_emotes.rs`), read at start from beside the world table (`--content`; `prepare_content.py`
copies them there). A bad row refuses the start with its file and
line. Every row carries its evidence in `source`.

- `quest-registry.tsv`: quest, journal_id, title_table, title_key, title, goal (a kill task's
  count), credit (`killer`), min_level, time_limit_seconds, max_solves, repeat_seconds, source.
  A run starts when SetStage leaves stage 0 (or a completed stage) for an underway stage; the
  start is refused below min_level, before repeat_seconds have passed since the last solve, or
  after max_solves solves. An underway run older than time_limit_seconds counts as stage 0. The
  start and solve times are durable quest records (`started`, `solved`); runs recorded before
  them have no times (no time limit runs out for them; a completed stage counts as solved).
- `quest-stages.tsv`: quest, stage, completed (0/1), journal_text, source.
- `emote-sets.tsv`: set, owner_entity, owner_iid, trigger (Use, Give, Kill, ItemUsedOn, Enter,
  QuestSuccess, QuestFailure; an Enter set names the world object it is around in owner_iid and
  fires when a player's position comes within its range), item, quest, stage_min, stage_max, range (metres, default 4),
  probability (default 1), source. The first matching set in file order runs.
- `emote-actions.tsv`: set, order, action, delay_ms (0; delays are not scheduled yet), p1..p5,
  source. Actions: Tell (StringInfo table, key), Message (Pyrealium text where retail has none),
  AwardXP, AwardLevelProportionalXP (percent of the level's experience span, up to two decimals;
  min; max, empty for none), GiveItem (entity, count), TakeItem
  (entity, 1), SetStage (quest, stage; prints the stage's journal line), IncrementQuest (quest,
  capped at its goal), InqStage (quest, min, max, success set, failure set), InqQuestCount
  (quest, count, success, failure), InqItem (entity, success, failure), InqLevel (quest,
  success, failure: the player's level against the quest's min_level), InqQuestReady (quest,
  success, failure: its repeat delay and solve limit), InqSolved (quest, success, failure: a
  prerequisite), AwardGold (amount), MoveTo (cell, x, y, z, heading), ApplyEffect (an `Effect`
  state, WState 0x6f DID; seconds: a quest's curse or blessing, durable in the quest book and
  shown on the client's effect strip through `WM_Effect` 0x17; it outlives a death, Pyrealium policy)
  and RemoveEffect (the state; `WM_Effect` 0x16). RandomGive (`0x47...:weight,...`: one item drawn by weight from the
  use's roll), IncrementCounter (quest, counter, cap) and InqCounter (quest, counter, count,
  success, failure): several counters in one quest. A Crafted set names the crafted item's entity
  in owner_entity and runs when the player's craft makes that item. The registry's credit
  `fellowship` also runs a quest's Kill sets for the killer's fellows in the world (quest
  progress only: items and experience stay the killer's; Pyrealium policy, no range is recovered). The effect carries no stat change of its own
  here: a row applies what its source gives, and says so. SetKingdom (1 Order, 2 Dominion,
  3 Shadow; 0 none), InqKingdom (kingdom or 0 for any membership, success, failure),
  AwardKingdomPoints (points, max; empty or 0 for no cap) and InqKingdomPoints (points, success,
  failure) keep a character's kingdom and Kingdom Points in the quest book (records `kingdom,N`,
  `kp,N`; the client's display of either is not traced). StartEscort (`0x47...`) makes the
  nearest living, unengaged creature of that entity within 30 m follow the player at 2.5 m,
  rejoining it from past 60 m (a portal); EndEscort releases it; its death runs the quest's
  EscortLost sets (owner_entity: the escorted entity; distances Pyrealium policy). GiveItem grants any item
  entity: the Shelter's pinned profiles, or one read from the DATs at start as the tradeskills
  read theirs (class defaults, the physical record's visual; `quest_emotes::install_items`).

Each table may be split into families: `<table>-<family>.tsv` beside the base file (for example
`quest-registry-vault.tsv`) is read after it, in name order, with the same header, and shares its
quests and sets. `prepare_content.py` copies the families with the base tables (and removes stale
ones); a launcher that copies the tables must copy `<table>-*.tsv` for each of the four too.

The `-vault` family is generated by `tools/import_vault_quests.py` from the AC2 Vault's 242-quest
library, resolved against the DAT index and the prepared world (never edit it by hand). It ships
the quests whose data resolves fully, that no older catalog runs, and whose shape the generator
knows: so far the hunts, single or in a prerequisite chain at one NPC (13 quests). Nine of them
replace hunting-catalog hunts, which `hunting-retired-vault.tsv` (generated) retires: their
profiles stay for the replay of the hunt records made while they ran, the hunting handler no
longer offers or counts them (record version `qh3`), the Vault rows keep the catalog's journal
ids, and at start `Store::migrate_retired_hunts` moves each character's hunt state onto them
(`qm1`). The Vault's values win where it gives them; the targets stay the catalog's where it took
them from the DAT's NPC tables. A hunt's rows come from the DAT where it has
them: the quest table's title and its numbered phase lines (the kills left, counting down, then
"Report to X."), the NPC table's offer, "not done yet", reward, "no task today" and "come back
when stronger" lines (by their key families; their roles are Pyrealium's reading of their text), and
from the Vault: the level, time limit, repeat delay and experience. Each kill advances one phase.
`reports/vault-quests-coverage.md` has the coverage, the ranked engine gaps, the generator's
reasons and the overlaps with the older catalogs (with the fields that differ for each catalog
hunt whose rows generate); `reports/vault-quests.drafts.json` has each quest's parsed draft with
every resolved field, its confidence and its source.

`vault-quest-overrides.tsv` is the hand-edited input to that importer: what you know that it could
not resolve. It is read before resolving and an override always wins; the rows it touches cite it
as "user override: <source>". Columns: vault_id, field, key, value, source (where the value comes
from; required). Fields:

- `start_npc`: key `entity` (an EntityDesc DID; its placement nearest the Vault's start
  coordinates) or `iid` (a placement id in the prepared world).
- `kill_target`: key the target's name as the walkthrough has it (or `*`), value EntityDesc DIDs,
  comma-separated.
- `reward_item`: key the reward's name, value `0x47...` or `0x47...,count` (1 to 16). The item must
  be readable from the DATs at server start (`quest_emotes::install_items` refuses the start
  otherwise).
- `xp`: key `fixed` (value: experience) or `percent` (value: `P` or `P,cap`, P with up to two
  decimals).
- `quote`: key the quote's number in the draft (1 = first), value `table:key` (hex).
- `prerequisite`: value the Vault id or title of the quest that must be solved first.
- `step`: key its order, value `talk NPC`, `kill N X`, `loot ITEM` (dropped by the kill before
  it), `give ITEM to NPC`, `use ITEM on X`, `open OBJECT`, `craft ITEM`, `effect 0x6f... SECONDS`,
  `lift 0x6f...`, `escort NAME` (a spawned creature, or an `npc` override's `0x47...`; its death
  sends the quest back to its offer), `arrive OBJECT [N m]` (an Enter set at a placed object,
  radius 10 m unless given; ends the escort) or `travel 12.3N 45.6E`; any step row replaces the
  walkthrough's parsed steps. Consecutive `kill` steps run at once (a counter each).
- `item` (an item's name, its EntityDesc), `npc` / `object` (a name, `0x47...` or a placement
  iid), `chest` (the object `open` names, what it gives: `0x47...[,count]` separated by `;`),
  `random_reward` (`0x47...:weight,...`), `credit` (`fellowship` or `killer`), `drop_reward`
  (`qp` / `kp`, `yes`: the Vault's points are not awarded), `ignore` (`chest`, `crafting`,
  `random`, `fellowship`, `pet`, `kingdom`, `escort`, `defend`, `yes`: the walkthrough only
  mentions it).

The importer writes rows for hunts (the DAT quest table's phases), and for any walkthrough the step
grammar expresses: parsed prose only when nothing it names is left out of the steps, else written
steps. Journal lines are the quest table's where one names the step's NPC, else Pyrealium text from the
step; a quest with no DAT title takes the Vault's (the Shelter's precedent). An NPC giving several
quests offers them in turn (a prerequisite unsolved, a level not reached or a repeat not due falls
on to the next). A hunting-catalog NPC retires when every hunt it gives has rows (the Vault's, or
the catalog's own values where the Vault has none, marked Pyrealium policy).

Lines starting with `#` are comments. A bad row stops the importer naming its file and line.
Placements are not overrides: put a quest giver or a creature in a world table (the world builder,
or by hand) and the next run finds it in the prepared world. `--gaps FILE` writes what still stops
each unshipped quest (quest, vault_id, gap, who, detail, draft, link; `who` is `you` for what an
override or a placement closes, `us` for engine or generator work), so a re-run shows the list
shrink.

## Retail capture population

`capture-spawns.tsv` holds 14,340 stationary objects read from retail packet
captures and not already placed by the other packs. Most come from captures
made on the 2012–2017 servers shortly before they closed: a human character from
creation and the starter dungeon (landblock `0298`) through Arwic, Kehan and the
Osteth vaults, plus two other players' sessions. The training pack loads it
after `world-observed-expanded.tsv` and `training-world.tsv` during preparation.
`capture-spawns-report.json` records each capture, its hash and why objects
were skipped.

The captures are **not a build-compatible spawn database**. Their visual DIDs
can exist in a different DAT build while naming unrelated models (including a
pavilion over an Arwic house). The launcher runs `tests/network/prepare_content.py`:
it admits additional capture visuals only when they match the installed entity's
resolved `PhysObj`/template chain or a curated entity/visual pair. Others remain
in the source TSV and are listed in `incompatible_capture_placements` in the
generated `.report.json`; they are not moved to invented positions or silently
assigned a replacement model. The compiled training preset uses only curated
observations and authored training placements. Use the prepared external content
manifest to add the compatible portion of the capture pack.

Recovering withheld placements requires the matching capture-build DAT set or
a verified mapping of entity/visual identities and world geometry between builds.
High-resolution textures cannot resolve this identity mismatch. The report
distinguishes incompatible observations from genuinely absent DAT records.
Lifestone snapshots within 12 metres are merged across outdoor subcell/block
boundaries, keeping the earlier stable IID. Different dungeon rooms and distinct
lifestone entity types are kept separate.

`placement-replacements.json` records explicit duplicate-observation decisions
that cannot be resolved by capture ID or cell ownership. Orven and Adaro each had
an older unverified Arwic observation alongside a semantically mapped,
native-ground-verified recovery at a different subcell/elevation. Preparation
retains the recovered placement and withholds the older row. Both endpoints are
pinned by source filename, ID and row SHA256; changed evidence stops preparation.
If the replacement fails admission, the original is kept. The generated report
lists applied and unavailable replacements. Source manifests are unchanged, and
no other actors are deduplicated by name, template or proximity. Restart the game
server to load a newly prepared manifest; existing sessions do not hot-reload it.

Regenerate it with `python tools/import_capture_corpus.py --game GAME_DIR CAPTURE_DIR...`,
naming your capture folders. It uses the same decoder as the
observation pack below, with these differences for the newer captures:

- World objects are accepted in both the `81200000` and `81300000` id ranges,
  and server-sent static objects whose `40` ids encode their landblock.
- Captures made on the player's machine record an IP length of 0; the frame
  length is used instead.
- Messages are rebuilt as they complete, and an unfinished one is dropped when
  more than 8,192 or 16 MiB are pending, so hour-long captures fit in memory.
- Objects later attached to an owner (Physics Parent `0x10087`) or put in a
  container (Contain `0x100cb`) are skipped, so worn and carried items are not
  placed in the world.
- The first capture to place objects in a cell owns that cell.

These are snapshot positions. Creatures stand where they were seen; wandering,
AI, spawn generators and respawn schedules are not reconstructed.

## Creature combat stats

`creature-combat.tsv` lists 317 candidate creature profiles with the combat fields their
`portal.dat` entity records carry: health, natural damage, armor, death XP and
level, with an interval column that is 0 when the record has no natural weapon
time. Parent records are merged first, as the 2012 server's
`Entity.LoadFromPortalDat` does. Regenerate it with:

```powershell
python tools/import_creature_combat.py PORTAL_DAT content/creature-entities.txt content/creature-combat.tsv
```

## Capture-observed actors

`observed-world.tsv` contains the observation pack; `observed-actors.json`
records each source capture SHA256, original IID, entity/visual, position,
quaternion, scale, native masks, message SHA256 and aggregate rejection reasons.
Original capture build identity and checksums have not been verified. Positions
are first materialization observations, not proven spawn origins or schedules.
These records remain distinct from table-derived and operator-authored content.

The offline importer `tools/import_observed_population.py` accepts repeated
`--capture` arguments plus `--base`, `--game-data`, `--output` and `--report`.
It reads bounded classic little-endian Ethernet/IPv4/UDP PCAPs, checks framing,
and reassembles blobs by directed endpoints, recipient, iteration, blob ID,
queue and fragment count. Conflicting duplicates and oversized tables/messages
stop conversion. IP fragments and unsupported transport packets are not used.
Reassembly is not authenticated and does not establish complete capture coverage.

The object decoder is grounded in VisualDesc `0063d7f0`, PhysicsDesc `00641530`,
SliderData `0067cfb0` and map packing `00640f40`, FXScalarAndTarget `004d5720`
and map packing `006410e0`, and gmWeenieDesc `006037c0`. It accepts the narrow
parent-visual-only profile and supported placed physics fields. It excludes
player namespaces, contained/equipped objects, missile/behavior-list profiles,
custom names and unknown description fields. It checks all remaining lengths,
finite transforms, quaternion norm and backing DAT records before placement.

Native packets are not replayed into the server or client. Runtime creates fresh
minimal stationary descriptions with local identities/stamps, observed entity,
visual, transform and optional gmWeenieDesc scale. Captured modes, effects,
sliders, velocity, look/target state and other description overrides are omitted.
This preserves a placement snapshot, not exact live behavior or full appearance.
Two-decimal position keys prevent duplicate placements; source IID repetition
is retained only at the first supported observation in each capture.
For overlapping inputs, the first accepted capture owns a cell; later captures
do not add actors to that cell. This avoids stacking snapshots from different
times. Input order and all exclusions are recorded in the report. It does not
establish a complete or simultaneous population across different cells.
New IIDs derive from capture hash and original IID in the reserved local
`81408...` range, with collisions rejected. Global and per-cell limits remain.

`lifestones.tsv` contains 89 placements imported from the supplied lifestone
database table snapshot dated 2012-08-11. Its source SHA256 is retained in the
file. These are imported placements, not positions recovered from the client
executable. Their original client/build identity has not been established.
They do not populate the human starter dungeon.

`world.tsv` is the combined pack: those 89 lifestones plus 158 portal placements
from the supplied portal table snapshot dated 2012-02-28. The full 160 portal
records, including target telepad IDs, string tokens and eligibility fields,
are preserved in `portals.json`. Two locations (`30780019` and `2e720032`) have
no backing terrain in the supplied assets and are marked `missing-cell` rather
than spawned. None of these placements are claimed to originate from client
decompilation, and the source snapshot's build identity remains unknown.

Portal visuals use four individually pinned qualities layouts, then verify
their physical definitions and visual records. The converter retains the
source quaternions instead of reducing them to byte headings. Source row ordinal
is part of portal IID allocation for this immutable snapshot; regenerating from
a reordered table is a content migration, not a supported hot reload.

The visual chain is checked against the supplied assets: definition `470007de`,
qualities `810007de`, physical definition `47000075`, visual `1f00002d`.
The importer requires the exact qualities SHA256 recorded in the manifest before
reading its physical-definition field at byte 236. It also checks the complete
16-byte physical definition, the visual record and each backing terrain block.
It does not apply these offsets to arbitrary definitions.

The content currently supplies stationary presentation and target visibility.
Lifestone binding, death/resurrection, summon restrictions and use scripts are
not implemented. Portal travel, triggers and destination transfer are also
unfinished. Level/hero/kingdom restrictions in the source table have not
been translated into gameplay rules. NPC AI, quests, merchants, loot and portals
require additional data and behavior; this is not a full world import.

Regenerate the included file from the supplied table and matching assets:

```powershell
python tools/import_lifestones.py --source PATH --game-data PATH --output content/lifestones.tsv
```

The converter parses numeric rows; it does not execute SQL. Unknown table
columns, definitions, rotations, duplicate IDs, missing assets or terrain records
stop the import before the output is written.

The combined importer is `tools/import_world.py`. It accepts `--portals`,
`--lifestones`, `--game-data`, `--output` and `--report`. Missing portal cells
stop conversion by default. `--allow-unresolved` explicitly permits a partial
manifest and retains every excluded portal in the report. No SQL is executed.

## Definition catalog and explicit placements

`definitions.tsv` contains 7,583 full entity/visual mappings from the supplied
workbook's `Entity` worksheet, filtered against actual DAT record presence.
Source annotations remain labels only; they are not injected as native names
or treated as NPC behavior, spawn positions, loot or balance. The catalog's
source SHA256 is embedded in its header. Another 319 rows with unsupported or
abbreviated visual mappings are retained in `unresolved-definitions.json`.
No automatic NPC placements are inferred from this catalog.

`tools/import_definitions.py` regenerates the catalog with `--source`,
`--game-data`, `--output` and `--report`. Reading legacy XLS requires `xlrd`;
the Rust server gains no dependencies. Numeric Excel cells containing full
all-digit hex identifiers are interpreted as their displayed integer strings
in base 16, not as decimal DIDs. The source workbook is never modified.

To author a stationary NPC/object at a position chosen by the operator:

The graphical tool is `tools/content_editor.py`. On Windows, double-click
`OpenContentEditor.cmd` in the emulator directory, or run:

```powershell
python tools/content_editor.py --game-data PATH
```

Each launch writes a diagnostic log under
`%LOCALAPPDATA%\Pyrealium\content-editor\logs`, including Python callback
exceptions and native fatal tracebacks where Python can capture them. This
keeps errors available when the `pythonw` launcher has no console. Closing the
editor waits for its current DAT read to drain before destroying Tk resources.

It requires Python with Tk support and uses no third-party GUI packages. Search
the catalog by label, entity or visual identifier; select an entry; choose the
game-data folder and content file; then enter a cell, X/Y/Z, heading and optional
scale. The **3D world** tab reads actual DAT creature meshes, the selected
interior room and linked nearby rooms. Drag to orbit, right-drag to pan, and use
the wheel to zoom. **F** or **Focus creature** returns to the selected placement.
Shift-click a world surface to set X/Y/Z through a ray intersection with its
actual triangles. **Cut away ceiling** helps inspect an interior from above.
The camera stays fixed while coordinates, heading or scale change.

Outdoor cells (`0001`–`0040` in a block) load the actual height block and one
neighbor ring. Heights use the native 256-entry height table, not a linear guess;
holes, triangle cuts, surface selection, UV tiling and vertex-color variation
come from the DAT definitions and recovered client formulas. Real terrain
material-stage textures are displayed with native vertex colors. Ceiling cutaway
starts disabled outdoors. Shift-clicking another outdoor cell or block updates
the cell ID and local X/Y/Z together; interior room picks retain their room ID.
Saved neighboring outdoor placements receive the proper block offset.
Road/corner surface blending, water and procedural scenery remain unfinished
and are reported in Details. This preview does not establish traversability.

**Play animation** loops a real native clip for the selected supported creature.
The clip list shows recovered asset IDs and periods; the mapped idle clip is
selected first where available. Pause or choose another clip, and change
**Speed** from 0.1 to 3.0. Playback evaluates native bone keys, quaternion
interpolation, hierarchy, inverse bind matrices and skin weights. Time advances
without accumulating catch-up frames. Only the selected preview is animated;
this does not run AI or gameplay state. Unsupported graph branches are listed
in Details. Root-motion application, graph blending, event hooks and attachment
animation remain unfinished.

The **Position map** tab remains available, with a stable view, coordinate grid,
pan, zoom and **Fit cell**. Clicking it sets X/Y while retaining Z. Existing
placements can supply a position and entity for a new addition; their identities
are retained. Neither preview changes existing objects.

The Windows 3D view uses embedded OpenGL with actual native UV texture pixels,
linear filtering, cutout alpha, a depth buffer and preview lighting. DXT1/3/5
mips are decoded read-only from `portal.dat` and optional `highres.dat`; base
material properties are applied before ordered instance overrides. It requires
no extra Python rendering packages. If Windows cannot create its OpenGL context,
the position aid falls back to sampled triangle colors and reports the reason
in **Details**. Full native shader passes, translucency blending, collision and
native BSP visibility remain unfinished. Some attached visual groups and nearby props remain
unsupported; **Details** lists omissions instead of substituting made-up models.
Where a supported first material layer is decoded, alpha testing, its constant
reference, blend factors and texture-alpha use follow that layer. Zero-filled
skin vertex-color fields are not used as tint, and texture alpha is not treated
as transparency when the material ignores it. Full conditional/multi-pass shader
selection remains outside the preview renderer.
Loading is bounded to 32 cells and 50,000 scene triangles; the viewport draws up
to 30,000, prioritizing the selected creature and reporting truncation. DAT work
runs on one background worker so the editor remains responsive. Native in-game
appearance remains unverified. Python syntax and read-only DAT extraction were
checked; no GUI or game was launched by the agent.

The default output is `content/local-world.tsv`, a new standalone pack of your
additions. To include base placements, choose a separate copy of `world.tsv` as
the output. Serve the resulting file through `--content PATH` on the next normal
server start. The tool does not reload or restart a running server.

The command-line interface remains available:

```powershell
python tools/place_content.py --catalog content/definitions.tsv --game-data PATH --output content/world.tsv --entity ENTITY_HEX --cell CELL_HEX --position X Y Z --heading 0
```

The tool requires an unambiguous catalog entry, existing entity/visual/cell data,
valid coordinates and available per-cell/global capacity. It allocates an unused
IID in the operator range beginning `8140000000020000`, marks the placement as
operator-authored, flushes the temporary file and replaces the manifest atomically.
An exclusive `.edit.lock` prevents overlapping tool saves, and a changed-file
check rejects external edits during validation. If a process crashes while
holding that lock, close all placement tools and remove only the corresponding
stale `.edit.lock` before retrying. It does not alter a
running server. Regenerating `world.tsv` from source tables replaces such local
edits, so retain them separately before regeneration. No made-up demonstration
NPCs were added to the shipped pack.

## Manifest format

The first line can be `pyrealium-world-v1`. Following nonempty lines contain eight
tab-separated fields: object IID, entity DID, visual DID, cell, X, Y, Z, heading.
Lines starting with `#` are comments. IDs are hexadecimal, coordinates are
decimal and heading is a decimal byte (0..255). Object IDs use the local
`8140` namespace and must be unique. Entity/visual DIDs use `47`/`1f` prefixes.
`pyrealium-world-v2` instead has eleven fields, replacing heading with quaternion
W, X, Y, Z. All quaternion components must be finite with norm within 0.01 of 1.
The combined pack uses v2; existing v1 files remain supported by the server.
`pyrealium-world-v3` adds a twelfth field: a positive scale up to 100, or `-` to
leave scale inherited. This is local content policy. The observation pack uses
v3 and writes the scale as native gmWeenieDesc bit `0x80`; v1/v2 encodings are
unchanged. The authoring tool accepts v2 and v3. Its optional `--scale NUMBER`
upgrades a v2 file to v3 while preserving inherited scale (`-`) for every prior
object. Both interfaces allocate additions only in the operator IID range
`8140000000020000` through `814000000002ffff`.

The runtime rejects files larger than 4 MiB, placement lines over 512 bytes,
more than 16,384 objects or more than 64 objects in one cell. Positions must be
finite and bounded. All referenced definitions, visuals and backing cell data
must exist before the server opens its sockets. This checks data presence,
not arbitrary entity/visual compatibility or terrain traversability.

Outdoor cell IDs 1..64 within a block use its `ffff` terrain record. Supported
interior cells use their own record. Reserved IDs and the gap before `0100`
are rejected. The player movement data-presence check uses the same mapping,
with a bounded cache of previously accepted cells. It does not enforce collision.

Objects are created after scene readiness, destroyed when their owner-visible
cell is left, and recreated when that cell is entered again. Visibility follows
the existing same-cell policy; nearby-cell/outdoor-block visibility is unfinished.
Encoded spawn/destruction payloads are immutable and shared between recipients.
Content stays outside the player-session index and player count limit.
Object instance stamp 1 is local policy for these immutable server lifetimes;
runtime replacement/hot reload is not supported.

No tests or native-client checks were run for this addition. Compilation and
read-only asset inspection do not prove in-game appearance or interactions.

## Pinned upstream continuation

`capture-spawns-upstream.tsv` is the unmodified v4 capture snapshot from Pyrealium
revision `468793018b332b87228794b0720ea1ce0400f96d` (22,422 rows). Its optional
last column preserves captured names. Preparation merges it after existing inputs,
keeps earlier IIDs/cell snapshots, validates native qualities/physical visuals and
reports conflicts. It does not enable all 22,422 observations. Current installed
DATs admitted 1,276 total placements before the subsequent corpus audit below.

`upstream-content.json` records provenance. Combat values were regenerated from
local DATs against the broader upstream candidate list: 317 usable profiles,
406 failed/missing candidates. The upstream skill/advancement catalog was not
copied because the installed DATs disagree with many of its records.

The supplied 2017 SQL archive has no entity/quest population rows and its 160
portal rows duplicate the retained portal metadata. Use
`tools/audit_pyreal_sql.py` to inspect it without executing SQL. The raw archive
and dump stay outside source control.

## Combined packet and historical emulator audit

`capture-spawns-audited.tsv` adds 121 conservative observations in 86 uncovered
cells from the supplied MEGA capture/emulator collections. Its report records
capture/message hashes, initial state and exact-row exclusions for historical
looted/dead observations. Preparation keeps one source snapshot per cell,
preserves original coordinates, validates installed entity/model mappings and
admits 1,392 combined placements after all filters. A location observed once is
not proof of a retail spawn schedule.

The native offline decoder and bounded PCAP/PCAPNG audit tools can regenerate
this data without publishing captures or account/player information. See
`docs/CAPTURE_REFERENCE_IMPORT.md` in the repository root for provenance,
commands, verification and the limits of the historical emulator/SQL references.

The historical `creature-entities.txt` list includes town NPCs and is only an
extraction candidate list. It does not establish attackability. Runtime encounters
require a recovered `creature-combat.tsv` row or an explicit authored rule;
services and configured quest guides remain protected even if their templates
carry combat statistics. The current 1,392-object pack has 510 combat actors,
including 135 outdoor placements in 64 blocks. This is incomplete observed
coverage, not retail world generators. `@spawns` lists nearby admitted actors;
it does not generate creatures.

## Population recovered with source-build DATs

The later 2017-client comparison supersedes the coverage counts above.
`recovered-population.tsv` contains 4,217 placements translated by native
localized name, AI table and role, with source visuals and native ground contact
verified. Coordinates, rotations and scales remain those captured. Its report
records every mapping, capture/message hash and both client DAT fingerprints.
The manifest has a required content hash and LF line endings.

The recovered rows include 3,931 monsters (3,742 outdoors), 261 scripted actors
and 25 vendors. The merged pack has 3,872 outdoor combat creatures in 1,642
blocks and 78 native vendors. One capture snapshot per region prevents repeated
journeys/respawns from being stacked. Curated and training cells remain prior.
`population-protected.txt` prevents recovered service/scripted identities from
becoming generic combat encounters through inherited stats.

The normal launcher prepares this pack automatically. Rebuild the server after
changing combat/protection catalogs, then restart it. `@spawns` lists nearby
live monsters on a local test server. A verified Cutter Beetle location is
`78B8002E`, approximately 44.9N, 5.4W.

Restored placements do not imply completed quest scripts, retail loot tables,
vendor inventory parity, or retail spawn schedules. Refer to
`docs/CAPTURE_REFERENCE_IMPORT.md` for the full source audit, regeneration
commands, validation and remaining gaps.

## Population ownership

Eligible combat placements become server-managed **point spawners**. The server
withdraws the imported monster lifetimes before accepting clients, then the
spawners create their children. Each point has capacity one, including a corpse
that is still present for loot. Death schedules corpse removal and regeneration
separately; replacement waits for confirmed removal. Movement and visibility
changes do not vacate a slot. Vendors, protected quest actors, doors and other
fixed content do not enter this controller.

`development-encounters.tsv` retains its legacy `npc` regeneration field. An
optional `spawner` row overrides initial delay and regeneration in milliseconds
for a placed combat template: `spawner<TAB>entity<TAB>initial_ms<TAB>regen_ms`.
Without an override, initial delay is zero and the existing authored interval
applies (30 seconds for imported native combat profiles). These are Pyrealium rules,
not recovered retail generation schedules. Lootable corpses remain for 60
seconds, so their replacement may wait longer than the regeneration interval.

`native-generators.report.json` records the installed DAT evidence: 639 templates
carry generator properties and 2,111 profile labels exist, but the parsed outdoor
and interior placements contain no generator profiles, and the encounter table
is empty. Five unsupported template parses are recorded. The template properties
include regeneration periods and quantity overrides; the profile child lists,
region definitions and generator locations have not been recovered. A profile
name is insufficient to bind a camp or treasure generator to a captured monster.
The current point spawners preserve verified capture positions, full quaternions
and scale; they do not add random regional populations.

An optional `spawn-group<TAB>group_hex_id<TAB>capacity<TAB>child_iid_hex,...`
row binds an explicit roster of existing monster points. Capacity is 1–64 and
cannot exceed the roster size. Each child belongs to at most one group; services,
unknown actors and duplicate children are rejected. Pending creation, live
children, corpses and pending removal all occupy capacity. Group rows do not
invent positions, clone templates, or change the members' regeneration policy.
No production grouping is inferred from proximity: native generator rosters and
quantity overrides for these placements have not been recovered.

## Loot and idle AI

`observed-loot.tsv` contains 698 observed corpse bundles mapped to 146 installed
monster templates; `loot-items.tsv` contains 222 native item definitions. The
source/mapping evidence and DAT hashes are in `observed-loot.report.json`.
The runtime admits 146 creature tables, 125 native death-loot profiles and 222
items with the current DATs. Exact creature tables take precedence over a shared native
`NPC_DeathLootProfile`. `NPC_LootProof` suppresses drops. Unknown profiles return
an empty corpse, never the old universal potion. Existing authored tutorial
quest-item/gear rewards remain separate from generic corpse loot.

These are observed base-item bundles. Observed frequencies supply Pyrealium weights;
retail probabilities, randomized affixes/quality and unobserved tables have not
been recovered. A capture is not proof of the complete retail loot generator.
The separate `native-trophies.json` now provides explicit installed-DAT trophy
probabilities: matching trophies are removed from observed bundles and rolled
with their native slot rates. Generic treasure weights remain unrecovered.
Each corpse rolls once, supports up to 32 items, and retains unclaimed items on
close/reopen or a full backpack. Single pickup and Loot All use journaled grants;
claimed inventory survives restart. Unclaimed corpses remain temporary.

`native-loot-limits.json` contains 312 inherited native quantity overrides, guarded
by hashes of the entity inheritance chain and property type registry. These
minimum/maximum controls replace the inferred/observed generic item count where
present (for example, Baron Colier: 3–6). Zero is honored. Native trophy slots
remain separate and explicit operator loot tables take precedence. The creature's
native level is a fallback for reconstructed item stat rolls; quality/variance
overrides are retained as evidence without inventing their retail formula.
Uniform quantity sampling, the supported equipment/restorative pool and equipment
rolls remain Pyrealium reconstruction. Observed supported equipment also receives durable
rolls. Reopening a corpse never rerolls its items, and pickup preserves their stats.
Regenerate with `tools/import_loot_limits.py <DAT-dir> content/native-loot-limits.json`
from `emulator`; the report lists five undecoded entity layouts withheld.

`treasure-weapons.json` adds 69 observed weapon identities to generic treasure,
with entity/qualities/physical/visual hashes checked against the mounted DAT.
Native `ImplementType`, valid/preferred hand slots and `Usage_MinLevel` are
preserved. Generic rolls exclude weapons above the creature's level; explicit
observed bundles retain their historical eligibility. Overcharged Hive is withheld
from this pool because its additional `0x2d8` usage restriction is not implemented.
All 69 defaults omit generated damage. The documented Pyrealium fallback is
`(6 + 2 * treasure level)` with ±15% per-item variance; it is not the recovered
retail treasure formula, affix system or per-hit weapon variance. A roll's treasure
level is at least its native use level, but does not become a new equip requirement.
Inspection reports both levels and damage. Equip/replacement validates the native
minimum, and basic attacks/weapon-based skills consume the equipped damage rating;
periodic damage does not add that rating again. Weapon grants use the distinct
`W` journal record, preserving their stats through pickup, equip and restart
without relabeling them as crafted armor. Existing unrolled damage values
are not migrated. Rolled weapons use the existing authored three-gold equipment
sale policy; native economy and mutation tables remain open.
Regenerate from `emulator` with
`tools/import_treasure_weapons.py <DAT-dir> content/treasure-weapons.json`.
`tests/network/test_weapon_loot.py` exercises a four-weapon corpse, native level refusal,
pickup/inspection, Flame Bolt damage and burns, remote FX and equipped restart
persistence over UDP. The regional population test validates both armor and weapon
rolls for every placed creature template.

`creature-ai.tsv` preserves inherited native perception, homesickness, wandering
enable/radius/probability, free-attack/idle-only flags and loot-profile bindings.
Absent properties stay absent. No wandering range is invented for a creature
without one. The runtime uses native collision for wandering, pursuit and line
of sight. Nearby actors scan once per second on staggered schedules; distant
unobserved actors do not load collision merely to roam. Uniform disk wandering
destinations, the missing-probability fallback (0.2), and generic
hostility exclusions are Pyrealium policy, not recovered retail AI scripts.

Creature locomotion now resolves the placed `VisualDesc` parent chain to its
`MotionInterpDesc` (`0x32`) and native Y scale. `CMotionInterp` selects the ground
peace/combat mode's forward speed (default-mode fallback), multiplies speed by
visual and instance scale, and scales acceleration only for bodies above unit
size. The server applies those limits with elapsed time, caps stall catch-up at
250 ms, and collision-checks movement in segments no larger than 0.2 m. Homeward
travel spends the same distance budget along recorded segments rather than
retracing just one segment per tick. All 8,612 installed combat actors resolved
native profiles in the population audit; none use the synthetic-fixture fallback.
At unit instance scale, native forward limits include Idol Spawn 3.9 m/s,
Red Soldier Wasp 6.5 m/s, Crawlspace Vermin 7.475 m/s and Drudge Outcast 7.8 m/s.
Captured instance scale can further change these values. These are locomotion
limits, not a claim that retail AI always requests maximum speed: original AI
pace choices, speed-altering effects, flight paths and swimming navigation remain
separate gaps. Source: `ac2_motion_interp.rs`, `motion_interp.rs`, and the admin
client's `CMotionInterp::GetDesiredMotionValues` (0050B660).

`native-quests.json` is a version-fingerprinted authoring catalog of native quest
text, phases, related quests and entity/effect links. It does not enable quests
whose prerequisites, NPC tables and reward flow remain unresolved. Recovery
methods and runtime changes are documented in `docs/ADMIN_CONTENT_RECOVERY.md`.

`recovered-quest-text.json` adds historical authoring evidence for 291 missing
fields across 98 native quests. Installed parent StringTables must be byte-identical
to the archived Pyreal parent; every private redirect is fingerprinted. Existing
translations are never replaced, grammar markers remain raw, and these strings
are not a live localization overlay.

`reference-tables.json` catalogs the additional Pyreal/AC2Util SQL tables and
the archived wiki's HTTrack cache. It includes 184 NPC guide rows, 67 incursion
starter rows, 162 quest-index rows, 791 bestiary rows, and native-title candidate
links to 83 wiki articles/quests. It also retains 46 incursion reference rows,
eight repeat-timer conflicts, and selected loot/dungeon tables. Full article
text is generated under `Saved/ContentRecovery/wiki-content.json`.

Both catalogs have `runtime_enabled: false`. Guide coordinates lack exact cell,
height, heading and generator rosters. Quest kill targets are not simultaneous
spawn counts; sampled trophy percentages and likely drop-count ranges are not
retail loot weights. Follow the source hashes and resolve version conflicts
before authoring gameplay. Neither catalog is loaded by the game server.

The user-authorized `inferred-content-v1.json` pack now derives a separate playable
reconstruction from those references: 88 quest profiles, 143 capacity-limited
camp rosters and 26 additional noncombat NPC placements. `inferred-world-v1.tsv`
contains 702 accepted native-ground placements, with source/manifest hashes and
rejections in its report. `prepare_content.py` appends this pack after captures;
it never rewrites them. Additional native combat/AI profiles live in the matching
`inferred-creature-*-v1.tsv` files. Generic loot now varies and supported armor
has persistent ratings/tints. These are inferred rules, not recovered retail
weights. See `docs/ADMIN_CONTENT_RECOVERY.md` for exact assumptions, provenance,
limits, regeneration commands and immutable journal-version requirements.

## Ringway exploration quests

`exploration-content-v1.json` implements the three regional Obelisk Search
conversations and the Find the Arwic Mines Overseer referral using native
quest/entity IDs and recovered journal text. Guide XP, deadlines and cooldowns
are recorded as reconstruction policy. Versioned `q9` journal events preserve
ordered visits, return-to-Explorer rewards and referral completion across restart.

`exploration-world-v1.tsv` adds nine stationary obelisks near the recovered
ringway directions; it adds no NPC copies. These placements are inferred and
native-ground checked, with hashes and zero rejected locations in the matching
report. `prepare_content.py` requires that evidence and installed visual/cell
compatibility before including them. The authoring script refuses to overwrite
an existing policy version; changed reward/state semantics require a new version.

Temporary obelisk buffs and a live journal countdown remain unfinished. The
subsequent mining/crafting lesson is implemented below. See the
[recovery notes](../../docs/ADMIN_CONTENT_RECOVERY.md#ringway-exploration-pack-v1-2026-11-01)
for exact assumptions and `tests/network/test_exploration_quests.py` for the real UDP
conversation/restart test.

## Native skill effects

`skill-effects.tsv` imports 1,602 use records from the installed Legions DATs.
`skill-effects.report.json` fingerprints the source skill/effect records and
records 325 withheld catalog entries (320 short roots and five ambiguous curves).
The importer checks the native object graph and typed list fields; instant
health/vigor values use field 36, and periodic values use field 38 in this build.
Scalar values must not be interpreted as references to graph object zero.
The server verifies the source hashes before admitting this profile.

`skill-runtime.report.json` inventories all 1,927 catalog entries. Currently 538
have executable core effects, including **235 racial profiles** (some already
used by starter attacks), and nine shared passive masteries are implemented.
These counts do not waive training, race, level, prerequisites or equipment
requirements. The remaining entries include passives, 472 profiles needing
specialized handlers and 29 recalls needing destination/entitlement handling.
The existing bound-lifestone recall retains its separate implementation.

The supported core includes direct damage, instant/periodic vigor drains, health/vigor restoration, periodic
healing and burns, regeneration multipliers, armor and melee/missile/magic
attack/defense ratings. Flame Bolt (`59`, native record `7000172d`) requires a
stave and uses its DAT cost, range, cooldown, rank-scaled damage, burn and story/FX.
Ease Suffering (`55`) exercises the self/other healing path. Casts validate the
learned skill, implement, target, visibility, range and cooldown before admission.
Failed admission does not spend vigor or cooldown. Burns use the existing kill,
quest-credit, loot and respawn paths; they cannot follow a corpse into its new life.
Damage casts still cannot attack protected NPCs or other players.

Vigor drain handling adds core execution for Dizzying Fumes, Disease Arrow,
Aggravated Energy, Energy Vortex, Convulsions, Neurotoxin, Embolism, Lightning Bolt,
Lightning Strike, Infect and Spiritual Malady. Native rank tables, training and
implement requirements still apply. Drains clamp to the target's remaining vigor;
misses apply neither instant drain nor periodic effects. Health and vigor ticks
can share a compatible timer, and caster cost is paid once. Creature vigor is
replicated to observers, selection vitals and examination, restored on evade/respawn, and does not
cause death or loot when it reaches zero. Creature spell AI and native ability
costs remain unfinished; ordinary creature melee attacks still cost no vigor.

This is **core single-target execution**, not complete retail spell behavior.
Area selection, multi-hit timing, combos, summons, status handlers, retail effect
stacking categories and persistence of timed buffs remain unfinished. Compound
effects with unsupported required components or incompatible timer layouts are
withheld. Generic rank damage is added to the existing authored encounter damage;
spell release timing, creature ratings and the 75%-at-equal-rating hit curve
(bounded to 5–95%) are Pyrealium policy. Shared mastery attack/defense formulas come
from the recovered 2012 Pyreal `Player.cs`, but that does not verify current retail
balance. Timed effects are bounded runtime state; health, vigor costs and rewards
use the durable player journal.

The importer and runtime were adapted from
[Pyrealium at 4687930](https://github.com/Vanquish-6/Pyrealium/tree/468793018b332b87228794b0720ea1ce0400f96d/emulator)
(AGPL-3.0), then corrected for the installed Legions schema and integrated with
Pyrealium's existing encounters. `skill-travel.tsv` retains 30 recovered travel records
as evidence, using the supplied Pyreal `Pyreal_DB_Insert_Telepad.sql`; it does not
enable those routes automatically. Raw DATs and reference archives stay unchanged.

Regenerate the import with `tools/import_skill_effects.py --game-data <DAT-dir>
--telepads <Pyreal_DB_Insert_Telepad.sql> --output content/skill-effects.tsv
--travel content/skill-travel.tsv` from `emulator`. Regenerate runtime coverage
with `cargo run --example skill_coverage > content/skill-runtime.report.json`.
The importer tests check native field decoding. Rust tests cover casting rules,
cooldowns, mastery progression, periodic effects, expiry and durable burn kills.
`tests/network/test_active_skills.py` checks trained Flame Bolt over UDP with the actual
DATs, including stave requirements, protected targets, observer animation/FX and
burn ticks, using disposable characters and an isolated server.
`tests/network/test_vigor_loot.py` checks trained Lightning Bolt, observer vigor updates,
live examination, native boss loot bounds, ownership, stable corpse rolls and
preservation of rolled armor/level through inventory pickup over real UDP.

## Profession crafting and gathering

### Arwic Mines lesson

Speak to the existing **Arwic Mines Overseer at 44.0N, 6.1W**. Enter the
**New Arwic Mines at 45.6N, 2.6W**, defeat a Drudge Outcast and loot its Crude
Flat Hammer. Return in peace stance with the hammer in your backpack; double-click
the Overseer or use the hammer on him. You keep the hammer and receive 20 Iron
plus the special **Arwic Mines Short Sword** recipe under Skills → Craft → Quest
Recipes. Craft it from 20 Iron without a forge, skill rank or gold cost, then
return it the same way for 150 XP, an Arwic Outlands Glyph and The Shaper's Guide.
Double-click the book to read each of its 15 recovered pages in chat.

`arwic-mines-v1.json` pins the native four-phase journal text, recipe/item
identities, quality fingerprints, book pages and guide evidence. The learned
recipe persists and is rejected by validation/execution before the lesson.
Versioned `m1` transactions save progression, ingredients, output and rewards
together, including journal replay and compaction. Split Iron stacks are
accepted; insufficient space, stale ingredient IDs and duplicate turn-ins cannot
consume materials or duplicate rewards. The lesson repeats after 20 hours; a
retained hammer can be reused. Existing `q9` referral journal semantics stay frozen.

`arwic-mines-world-v1.tsv` adds four Outcasts and two Edge Beetles in six
collision-checked interior rooms in `7cb9`. Two additional capture anchors
failed the walking-patch checks and remain withheld. The existing Taskmaster
and Overseer are retained. Two bounded spawner groups own the six children;
the existing 30-second respawn delay never replaces a still-present corpse.
Native Outcast combat and AI properties are recovered reproducibly by
`tools/author_arwic_creatures.py <DAT-directory> <content-directory>`, with input
and output hashes in `arwic-mines-creatures-v1.audit.json`.

Room-to-template assignments are inferred from older capture positions and the
guide because those capture DIDs identify different items in the installed DAT.
The guaranteed hammer for an eligible quest holder supplements ordinary loot;
its original drop probability and spawn schedules are unrecovered. No wandering
radius is invented where the native template has none. The glyph reward is
implemented; the separate Arwic Vault unlocking/quest chain is not completed by
this lesson. `tests/network/test_arwic_mines.py` verifies the full exchange over UDP,
including three server restarts. Native-DAT tests separately check production
monster stats, aggression and corpse-aware spawner refill.

### Recovered profession catalog

`professions-v1.json` is generated by `tools/author_professions.py` using the
bounded native WState decoder in `tools/craft_catalog.py`. It contains 26 skills,
357 recipes (317 production/practice/potion, 20 refining, 15 surveying, four
dyeing and one learned Arwic quest recipe), 847 item definitions, 14 workshop definitions, 56 mine definitions,
90 survey maps, six gathering tools and 1,396 eligible butchery entity profiles.
`professions-v1.audit.json` records the source WState SHA-256 fingerprints and
each excluded recipe with its reason. Raw DATs and reference sources are untouched.

Recovered inputs include skill names/icons/caps, cumulative advancement curves,
per-use XP where present, 22-hour learning-limit curves, recipe levels, ingredient
ordinals/types/quantities, success output weights, gold costs, workshop effects,
map regions, mine materials/yields/variance/uses and the 1,001-sample native
success table (`6f002fb9`). Production skills reach 500; gathering/refining reach
150. The legacy recipe index is supplemented by the installed Legions ordering
table and Empyrean dye recipe. Unsupported quest entitlements and special
mutation handlers are excluded rather than silently spending their materials.

### Playing the implemented loop

- **Skills → Craft → All professions:** choose a profession and recipe. The
  ingredient window shows level, cost and success chance. Use −/+ to change the
  output level, fill from the backpack, or click a row to cycle matching items.
  Requirements can span multiple stacks. Failed manufacture consumes its inputs;
  rejected requests consume nothing. Leftover quantities return atomically.
- **Raise:** spends that profession's saved XP at the native cumulative-curve
  difference. The 22-hour learning cap and reset persist through reconnects,
  server restarts and journal compaction. Combat XP remains separate.
- **Workshops:** manufacture/refine requires the matching native effect within
  eight meters and the same visible scene. `workshops-content-v1.json` adds
  ground-checked yards near Arwic, Cragstone and the observed Omishan refinery.
  Eight new placements are in `workshops-world-v1.tsv`; their coordinates are
  reconstructed, not recovered retail positions. The ordinary world preparation
  pipeline requires a fresh source hash and collision report before admitting them.
- **Supply:** authored quartermasters offer 100-unit bundles of the basic and
  advanced raw materials. Purchases and refinery outputs merge into compatible
  stacks, including when the backpack is full. This deliberately permits crafting while broader
  gathering/loot coverage is unfinished; stock and prices are not retail economy.
- **Surveying:** create a continent/material map, examine its coordinates and
  use it within 40 meters on dry ground. Map generation checks native terrain;
  the selected location is journaled. A 15-minute per-continent cooldown resets
  when the map is found. The deposit appears beside the finder on reachable
  ground and retains its native three charges across restarts. Located deposits
  expire after one hour. At most 12 unfinished maps/deposits can be active.
- **Gathering:** carry a pickaxe (iron), mattock (silver), chisel (crystal), rock
  hammer (stone) or hatchet (wood). Double-click the deposit or target it and use
  the tool. Higher-level tools require gathering rank at least three times their
  level; level-one tools are always usable. Direct deposit use selects the best
  usable tool. Materials merge into compatible backpack stacks. Tool wear, XP,
  gathered materials and deposit depletion commit together.
- **Butchery:** select a defeated creature from your own recent kill and use a
  hunting knife within four meters. Native `ButcheryProfile` eligibility is
  required. Looting the corpse first does not remove the harvesting entitlement;
  it expires with the recent-kill reward window (60 seconds). Each corpse instance
  can be butchered only once. Friendly actors cannot supply butchery materials.
- **Creation tools:** equip a flat hammer (weapons), shaping hammer (armor),
  magnifying glass (tools), crucible (refining) or athame (implemented spellbinding
  component/practice recipes). A matching profession must be at least ten times
  the tool's level. Each tool level adds one effective rank as a provisional
  bonus; base rank, advancement costs and learning limits do not change. The
  recipe window labels its unmodified probability **Base chance**. Tool wear is
  charged on successful and failed attempts. Exhausted creation tools remain
  inspectable/discardable but give no bonus and cannot be re-equipped.
- **Dyeing:** native plants and race-specific armor predicates choose the finish.
  Dyeing retains the selected item's saved armor/damage/level, costs native gold,
  always succeeds and grants no XP. Other matching garments remain unchanged.

### Reconstruction boundaries

This is not a complete retail crafting implementation. The following policies
are intentionally explicit so later recovery can replace them:

- Most enabled recipes are default grants; the Arwic sword requires its saved
  quest entitlement. General discovery, account restrictions and most
  quest-specific recipes are not implemented. The existing
  Field Supplies/Potion Practice lessons still use their separate authored flow.
- Manufacture XP is twice `(difficulty + 1)` when no native per-use curve survives;
  trivial recipes more than 30 difficulty below rank grant none. Failure gives no
  XP. Refining interpolates 33% at rank one to 83% at 150 and always succeeds.
- Armor/weapon output statistics retain recovered base values plus two per output
  level after level one, bounded to 1,000; armor receives a saved tint. These are
  provisional playable values, not recovered retail mutation or critical formulas.
  Crafted stats participate in equipment/combat and inspection. Apparel recovery
  uses unambiguous native worn/pile appearance families, never guessed meshes.
- Mining starts from native continent yields (25/30/35) and ±15% variance. Survey
  and tool bonuses scale to +40 and +50 respectively. Tool durability defaults to
  20 durability where the template lacks a maximum; each use has a 50% wear roll.
  Durability is saved on the item, so transfer/pickup cannot repair a tool.
  Old character-based wear records migrate on use. Mining critical results,
  timed gathering animations and crafted armor/weapon wear remain unfinished.
- Butchery's original material tables are missing. Eligible arthropod families
  yield chitin; other eligible families yield hide, at a reconstructed base of
  five with ±15% variance. Bone and sinew currently come from supply stock/loot,
  not a claimed retail butchery distribution.
- Spellbinding practice and concrete component recipes work. Enchantment
  extraction, totem application, harmonic stones and item binding require saved
  enchantment/slot/durability state and compatible effect handlers. They are not
  enabled. Major/minor extraction must destroy the source and transfer its actual
  eligible enchantments; substituting an armor upgrade would be incorrect.
- Active survey maps cannot be dropped or traded; survey jobs are owner-bound.
  Abandoning maps and specialist summoned mines
  need separate implementations.

The native data is the authority for recovered values. Historical descriptions
helped bound reconstruction: [crafting overview](https://asheron.fandom.com/wiki/AC2%3ACrafting_2.0_-_The_Ultimate_Newbie_Guide),
[mining](https://www.20oz.net/index.php/Mining.html),
[refining](https://www.20oz.net/index.php/Refining.html),
[toolmaking](https://20oz.net/index.php/Toolmaking.html) and
[spellbinding](https://20oz.net/index.php/Special%253ASearch/Spellbinding.html).
These span different retail eras and do not establish exact Legions server formulas.

### Verification and regeneration

Set `PYTHONPATH` to the absolute path of the repository's `tools/ac2`, then run `tools/author_professions.py <DAT-directory>`
and `tools/recover_apparel.py --dat <DAT-directory>` from the emulator directory.
Workshop source is authored by `tools/author_workshops.py`; validate its placements
with the `population_authoring` example before running `tests/network/prepare_content.py`.
Do not treat source files alone as proof that the production manifest includes them.

Unit tests cover native recipe assets, XP caps/rank costs, partial stacks,
manufacture failure, insufficient gold, ownership/replay, dye preservation,
refining quantities, full-backpack gathering, transferred tool wear, creation-tool
rank/type restrictions, exhausted tools, quest armor sale restrictions, duplicate butchery,
full-backpack supply/refinery merging, survey cooldowns and durable deposit charges. `tests/network/test_professions.py` exercises
the crafting-to-mining loop over UDP and restarts an isolated server mid-deposit.
`tests/network/test_crafting.py` crafts over UDP (wrong ingredient, unknown recipe, ownership,
combat and replay rejected) and verifies the saved potion and quest results across a restart.
Production account/character stores are not used by these tests.

## Wiki NPC conversations and placements

`npc-dialogue-v1.json` binds 217 exact native entity variants to 326 authored
conversation pages covering 229 quest topics. These summarize wiki facts and
selected walkthroughs; they are not recovered retail speech. Empty lore sections
are not replaced with invented quotations. Mirror aliases with identical quest
bodies share a topic, and discontinued quests are labeled rather than activated.

Ordinary double-click/use reaches this catalog after working quest, merchant,
tutorial and service handlers. `@talk` browses the selected NPC's information
without accepting a quest, spending items or opening its shop. Success returns
native use completion, named speech, and a readable MessageDialog. Repeat use
cycles pages; switching speakers resets browsing. Both paths require a living
player out of combat, a visible noncombat NPC and a distance of at most four
metres. The one-second limit applies across both paths. No rewards or journal
state are changed by informational conversation.

The [audit](npc-dialogue-v1.audit.json) covers 225 wiki names: all 89 NPC-category
entries, town NPC lists and exact quest-starter matches. Seventy-four category
entries match native data; 15 do not. Aliases and multiple actor variants explain
the different name/profile counts. `npc-world-v1.tsv` adds 27 previously missing
actors, including Brisin, Old Herrigar, Sachem Mimbu, Dark Agent, Justiciar Maronak,
Dell, Otonku, Feral Pet Lasher, Rashan's friendly spectre, the Gurog Supreme
Warlord and several Greenstone NPCs. Their coarse wiki positions are adjusted
by at most 32 metres to dry, reachable native terrain, at least six metres from
existing actors. This is inferred placement, not a recovered retail transform.
Existing identities are retained to avoid duplicate Orven/Adaro-style placements.

Lataua's coordinate is withheld: her article repeats a Cavendo position while
her town and quest walkthrough specify Zu. Ambiguous native variants are also
withheld. The Gurog High Shaman's proposed location failed native walkable-ground
validation and remains unplaced. Hostile Zroth/Rashan variants and
non-NPC quest-starter objects cannot receive friendly conversation through this
catalog. All catalog variants pass native combat classification checks.

This adds conversations and population, **not complete quest state machines**.
Most listed quests still need eligibility, item exchanges, stage transitions,
dungeon access and replay-safe rewards. Consignment, kingdom services and
unsupported vendor inventories are not synthesized from an NPC's name.

Regenerate with `tools/author_npc_dialogue.py` from the repository root. It reads
the retained wiki cache and `Saved/current-native-templates.json`; their hashes
are recorded in the manifest. Run the `population_authoring` example with the
DAT directory, `content/npc-dialogue-v1.json`, output `content/npc-world-v1.tsv`,
report `content/npc-world-v1.report.json`, and the prepared production world.
Run the authoring script again to annotate the audit with verified placements,
then `tests/network/prepare_content.py` to install the pack. The preparation step
rejects stale proof or mismatched native visuals. Source DATs remain unchanged.

`tests/npc_population.rs` validates placement, duplicate prevention and all 217
native noncombat identities. `tests/network/test_npc_dialogue.py` exercises every
profile over UDP, actual new placements, topic cycling, admission restrictions,
and preservation of the existing explorer quest action in an isolated store.
`tests/network/test_npc_authoring.py` verifies shared quest starters and avoids binding
a person named in an NPC's location as a second quest starter.
