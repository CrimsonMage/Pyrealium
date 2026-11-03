# Pyrealium: an Asheron's Call 2 server

Pyrealium is a persistent Asheron's Call 2 server written in Rust. This repository
continues Thwargle's AC2E, itself derived from Vanquish's
[Pyrealium](https://github.com/Vanquish-6/Pyrealium) (see [Credits](#credits)). All of
its Rust code is in `crates/` (the server is `crates/pyrealium`); its launcher is
`start-pyrealium.bat` / `Start-Pyrealium.ps1`, with world content in `content/`, authoring
tools in `tools/` and the account page in `web/`. It speaks the native AC2
UDP protocol to AC2 clients. This is an experimental implementation, not yet a
complete retail emulation.

The Shelter sequence includes the digging-tool exchange, Scout potion rewards,
Menhir Glyph travel and an authored Vault completion step. Seven additional DAT
armor profiles join the racial starter clothes, with saved appearance replicated
to other players. Pyrealium sends native racial missile and auto-magic projectiles,
with damage after flight. Starter magic skills use their direct cast/impact path.
Full retail spell effects, ballistic physics and the Menhir history/cinematic
remain unfinished. See `GUIDE.md` for scope.

## Gameplay and recovered world content

The prepared world combines compatible captured placements with a separately
versioned reconstruction pack. It is playable content, not a complete recovered
retail server database. Source DATs, captures and reference SQL remain unchanged.

- **Population and AI:** native combat/AI properties identify supported enemies
  while service NPCs remain noncombat. Spawners manage creation, corpse retirement
  and regeneration; explicit camp rosters enforce group capacity. Creatures use
  recovered perception, wandering and homesickness properties with native terrain
  and collision checks. Movement uses native per-visual speed/acceleration and
  captured body scale, with time-based pursuit, wandering and homeward travel.
  Unrecovered AI timing and navigation rules are documented.
  The regional wilderness pack adds **3,495 creature slots in 2,071 spawner
  groups across 21 regions** on Osteth, Omishan and Linvak. Archived wiki
  regional rosters select native creatures; Pyreal ringway/town coordinates
  bound inferred placement areas. This reaches 1,842 previously empty outdoor
  landblocks while keeping clear of towns and existing content. Group sizes,
  exact locations and three-minute regeneration are reconstruction policy.
  A second wilderness pack adds **2,553 creatures in 1,555 managed groups**,
  including all five Knorr regions and Arramora, plus gaps in those original
  21 regions. It reaches 1,167 more outdoor landblocks and introduces 180
  previously unplaced native types. All 563 eligible types matched from the
  wiki regional rosters now have at least one placement in the combined world.
  This does not cover every dungeon, rare encounter or template variant; the
  [coverage audit](content/wilderness-coverage-v1.json) accounts
  for the remaining native records and withheld sources.
- **Quests and services:** readable signs send native descriptions. Supported
  doors, containers, merchants, lifestones, portals and tutorial actions have
  server handlers. The inferred pack adds 88 quest profiles, 143 camp groups and
  702 accepted placements, including 26 noncombat NPCs. These include counted
  hunts, boss hunts, three hunt elixirs and Gambling Debt; placing an NPC alone
  does not implement its dialogue, vendor stock or other quests.
  The wiki NPC pass adds **217 native conversation profiles** with **326 pages**
  covering quest requirements, starting areas, timers and listed rewards, plus
  **27 missing NPC placements** checked against native ground/collision. Double-click
  an otherwise unsupported NPC to speak; select an NPC and type `@talk` to browse
  information without invoking its working quest or shop action. These are authored
  replies, not recovered retail dialogue or 229 newly playable quests. The
  [NPC coverage audit](content/npc-dialogue-v1.audit.json) accounts for
  all 89 wiki category entries and additional town/quest NPCs, including 15 names
  without matching native identities and unresolved locations or actor variants.
  Arwic, Lost Wish and Esper Explorers now offer their **Obelisk Search** quests:
  visit each region's ruby, sapphire and emerald obelisks in order, then return
  for 80 XP and the **Find the Arwic Mines Overseer** referral. Nine obelisks
  are reconstructed near the documented ringway stops on verified native ground.
  Progress, deadlines, repeat cooldowns and rewards persist across restarts.
  The Overseer's **Arwic Mines and Crafting Quest** now continues that referral:
  recover a flat hammer from a mine Outcast, receive Iron and a learned sword
  recipe, then return the crafted sword for 150 XP, a glyph and a readable guide.
  Four Outcasts and two Edge Beetles use validated mine-room spawners and native
  combat/AI fields. Their room assignments and guaranteed eligible hammer drop
  are reconstructed. Temporary obelisk buffs remain unfinished.
  Thirty-five NPC hunting quests now support acceptance, counted kills,
  return-to-NPC rewards, prerequisites, deadlines and repeat timers. These
  include thirteen Ikeras hunts, fifteen Arramora hunts offered in Linvak Tukal,
  and seven Foundry hunts near Jethric's Outpost. Twenty-three native NPCs
  provide conversations; four missing speakers have inferred, ground-verified
  town placements. Three guide-listed Arramora camps add six creatures in
  managed two-child groups. Wiki rewards
  and native identities are retained; missing retail quest IDs and dialogue
  scripts are explicitly reconstructed. Quest changes send only modified entries,
  preventing large completed journals from filling the reliable network queue.
  See [hunting quest expansion](docs/ADMIN_CONTENT_RECOVERY.md#hunting-quest-expansion-v2-2026-11-01).
- **Profession crafting:** Skills → Craft → All professions exposes 26 disciplines
  and 357 supported DAT-backed recipes: armor, weapons, tools, refining, dyeing, surveying,
  practice and the two starter potions. Recipe levels, ingredient choices and
  quantities come from the installed DAT; materials can span backpack stacks.
  Crafting XP, rank increases, costs, finished item stats and gathering progress
  are saved atomically. Survey maps locate dry native terrain; tools harvest
  deposits and eligible owned corpses. Equipped creation tools boost matching
  professions; item-bound tool durability persists through transfers and restarts.
  Supplier purchases and refinery outputs merge compatible material stacks.
  Eight reconstructed workshop placements
  supplement the observed refinery; their native ground checks are audited.
  This is **not complete retail crafting**: enchantment extraction/application,
  general recipe discovery, exact critical/stat formulas and equipment wear
  remain open. Most recipe grants, some yields
  and output stat scaling are explicitly reconstructed. See
  [crafting coverage and controls](content/README.md#profession-crafting-and-gathering).
- **Loot and equipment:** native trophy probabilities supplement observed corpse
  loot; 312 recovered template rules now bound generic treasure quantities.
  The inferred treasure policy adds variable supported equipment/restorative
  drops and persistent armor ratings/tints. Sixty-nine observed weapon profiles
  now participate with durable damage rolls, native minimum-level checks and
  damage used by basic attacks and supported weapon-based skills. Weapon roll
  scaling is reconstructed; exact retail mutations remain unrecovered.
  Pickup, equipment and rewards are
  saved with the character. Recovered apparel, including gloves and headwear, supports equipping,
  removing and replacing items while preserving native models and race restrictions;
  unknown potion effects and full retail affixes remain unsupported.
- **Combat and movement:** repeated basic attacks retain their target, missile
  damage follows projectile flight, and local/remote swimming use native motion
  modes. Ground contact resets fall tracking. Native damage appearances, temporary
  ground blood decals and corpse names are connected; damage formulas, generic
  effect triggers and full retail physics still have documented gaps.
- **Travel and environment:** portal space covers destination streaming before
  revealing the world. Server time drives the sky cycle, and dungeon furnishings
  use their native cell-space positions. A bounded shoreline repair in block
  `74A9` uses the adjacent lake's 96.7 m height consistently for rendering,
  swimming and server geometry. It is an explicit content repair, not a DAT edit.
  Water uses continuous native textures and world-aligned UVs across the full
  14-block (about 2.2 km) landscape radius, replacing the nearby flat-blue
  distance cutoff. Surfaces are batched by material, with no overlapping near
  water layer during streaming.
  Terrain atlas edges are clamped to prevent opposite-edge texture bleed, and
  adjoining landblocks share the retail triangle-normal calculation for
  continuous lighting and slope-based materials, including block `86B0`.

Recovery tools audit Pyreal SQL, historical emulator tables, packet captures,
quest text, loot and native generator properties. Compatibility and provenance
reports separate direct evidence from inferred placements and rules. Missing
generator rosters, exact loot weights, complete dungeon populations and arbitrary
NPC scripts are still unresolved. See [content recovery](docs/ADMIN_CONTENT_RECOVERY.md),
[capture imports](docs/CAPTURE_REFERENCE_IMPORT.md) and the
[content manifest guide](content/README.md).

The world builder now supports creature stats, AI, weighted loot, patrol routes
and population groups in an overlay plus a `NAME.rules.tsv` sidecar. Its optional
loopback admin endpoint applies edits to a running server and saves them for
restart. Rejected edits preserve the current world; stale revisions stop sync
until the operator reconciles the files. Group overrides preserve the original
regional rosters, and live creatures use native movement descriptors. These are
operator-authored rules, not newly recovered retail data. See the
[world-builder guide](docs/content-editor.md) for setup and limits.

The server recognizes the original 7230 DAT profile and selects its own skill,
advancement and effect tables before loading persisted characters. Those tables
are validated against the archived 7230 portal records, including Flame Bolt;
Legions keeps its separate tables. A matching native 7230 client's full gameplay
acceptance is still unverified. See the [7230 profile notes](content/7230/README.md).

The optional GUN adapter service supports persisted friends/ignores, presence
and tells over custom TCP/XML. Enable it explicitly with `--gun-adapter-bind
127.0.0.1:12310` (`--gun-bind` remains an alias); it is disabled by default.
Archived retail captures instead use UDP port 2310, a sequenced handshake, and
different XML fields. A stock `GunDll.dll` transport is not implemented here. A native DirectPlay 8 codec now handles unsigned
framing, ordered fragment recovery, duplicate suppression and native XML envelopes.
Offline replay of 501 local retail captures recovers 303 XML messages; two
disconnect frames without captured version negotiation remain unresolved.
Live native sessions, retransmission timers and ticket authentication remain
unfinished. Adapter tests do not establish retail compatibility.
See the [GUN protocol and limitations](docs/gun-protocol.md).

## Missing content records

The preparation report (`content_meta.report` in the database, or `NAME.report.json` beside `pyrealium prepare-content` output) lists the exact record IDs and DAT
archives missing from each excluded server placement. These are different from
texture-resolution problems: `highres.dat` cannot supply absent entity, visual, or
room definitions. Counts grow as additional capture and regional content is
audited; use the generated report for the current totals and exact records.
Several placements can share a missing definition. The original starter-area
audit found 22 excluded placements (13 entity, 8 visual and 5 room records),
but those numbers do not describe the expanded world manifest.

If you have another complete patched Legions installation or backup, keep it in a
separate folder and compare it without replacing any active DATs:

```powershell
python .\Network\prepare_content.py "Asheron's Call 2 - Legions" .\Saved\candidate-content.tsv --compare-dat "D:\AC2-backup"
```

The companion `.report.json` lists both recovered placements and currently working
placements that the candidate would lose. This is a record-presence check, not proof
of dependency/protocol compatibility. Keep each installation's DAT set together;
a higher-resolution texture archive alone does not repair missing world records.
Without a matching source record, leave the placement excluded or create a clearly
identified replacement after its identity/location is established. The local
`C:\Turbine\AC2` copy was checked and recovered none of the original 22
starter-area exclusions.

## First checkout

```powershell
git clone https://github.com/CrimsonMage/Pyrealium.git
```

The server is self-contained: the DAT, resource, world, physics and protocol code it builds on lives
in `crates/` (taken from the AC2_Rusted client project at commit `bcf67b4b` and kept here as the
server's own). Nothing else needs to be checked out beside it.

Install Rust through rustup with `cargo` on PATH, and PostgreSQL 14 or newer
(the server is tested on 18). Python is needed only for the offline content-authoring
tools and the Python network tests, not to build or run the server. The verified
Rust toolchain is 1.99.1.

Supply your own retail Asheron's Call 2 **build 7230** installation (`portal.dat`,
`cell_1.dat`, `local_English.dat` and its high-resolution DATs together). The server
refuses other DAT sets. Game assets and decompiles are not included or required for
compilation. DATs stay on disk and are read on demand; they never go into the database.

## Run

One binary, one settings file, one database.

1. Create a PostgreSQL role (`createuser --pwprompt pyrealium`, and either let it create
   databases or `createdb --owner pyrealium pyrealium`).
2. `pyrealium init` (or the first `start-pyrealium.bat`) writes a commented
   `pyrealium.toml`. Set `dat_dir` and the `[database]` url.
3. `pyrealium` (or `start-pyrealium.bat`) starts everything: it creates and migrates the
   database, prepares the world content from `content` when it or the DATs
   changed, starts the account service (loopback HTTP 19400) and the game server (UDP
   9000/9001). `pyrealium stop` saves and stops it.

Every table sits in the database's default schema: accounts and one-use login tickets,
characters with their items, ground items, the world clock, GUN lists, and the prepared
placements and quest tables (`content_files`, `content_meta`). Characters are kept in memory while
the server runs and saved every `save.interval_seconds` (default 300), at logoff
and at shutdown, each save one transaction for every changed character and the ground
items, so a crash rolls the world back to the last save as a whole. Experience and gold
are columns, so an offline character can be read or credited with plain SQL while the
server is stopped. Back up with `pg_dump`. A second server on the same database refuses
to start; a crashed one leaves no lock behind.

An AC2 client signs in at the account service with a new account name and a password of
12–128 characters; first sign-in creates the account, later sign-ins require the same
password. `local` and `admin` are reserved names. `[server] auth = "developer"` (loopback
only) trusts the client's account name instead; `local-accounts` combines both.

`[world] test_map_travel = true` (loopback only) enables the `@maptravel` test command: a
living character out of combat may travel to a map point, with a five-second cooldown.

Docker: `PYREALIUM_DATS=/path/to/dataset-7230 docker compose -f deploy/docker/compose.yml up --build`
runs PostgreSQL and the server together. Hosting with systemd: `deploy/pyrealium.service`
and [online hosting](docs/online-hosting.md).

## Build and verify

```powershell
.\Build-Pyrealium.ps1 -Test
.\Build-AC2Network.ps1 -Test
python .\test_export_source.py
```

The Rust tests start their own throwaway PostgreSQL cluster (the system's `initdb` and
`postgres` must be on PATH), or use `PYREALIUM_TEST_DATABASE_URL` (a role that may create
databases). `Build-AC2Network.ps1` builds the server and the native protocol bridge
(`crates/ac2-network-bridge/`), the scripted AC2 client the network tests drive.

Network integration tests prepare their own content and isolated stores:

```powershell
# Optional when the DATs are outside this checkout:
$env:AC2_DAT_DIRECTORY = 'D:\Games\AC2'
python .\Network\test_network.py
python .\Network\test_auth.py
python .\Network\test_gameplay.py
python .\Network\test_armor.py
python .\Network\test_shelter.py
python .\Network\test_corpse_loot.py
python .\Network\test_outdoor_population.py
python .\Network\test_recovered_equipment.py
python .\Network\test_active_skills.py
python .\Network\test_live_world.py
python .\Network\test_inferred_content.py
python .\Network\test_regional_population.py
python .\Network\test_world_clock.py
python .\Network\test_falling.py
python .\Network\test_environment_content.py
python .\Network\test_map_travel.py
python .\Network\test_map_travel.py --disabled
```

Pass `-Python 'path\to\python.exe'` to the build/server scripts if Python is not
on PATH.

## Credits

- **Vanquish ([Vanquish-6](https://github.com/Vanquish-6))**: author of
  [Pyrealium](https://github.com/Vanquish-6/Pyrealium), the AC2 server this project is
  derived from (from commit `7c2e856840b04302d1c78f057dcf8094326472a4`). Pyrealium's code
  is copyright (C) 2026 Vanquish, AGPL-3.0-only.
- **[Thwargle](https://github.com/Thwargle)**: author of AC2E, the Pyrealium-derived server
  and its Unreal client that this repository continues: Legions compatibility, starter
  combat and content rules, recovered world content, crafting, quests and NPC
  interactions, authenticated hosting, source export and the build/launch helpers.
- **[CrimsonMage](https://github.com/CrimsonMage)**: this repository's server-only
  continuation (7230 profile, live gameplay, tradeskills, effects, quests as data, the
  AC2 Vault quest import, the world builder) and the
  [AC2_Rusted](https://github.com/CrimsonMage/AC2_Rusted) crates it builds on.

This repository starts a new history; the earlier commits were made in the AC2E project.
`NOTICE` records each party's modifications.

## Project files

- [Pyrealium setup and hosting](GUIDE.md) and the [content manifest guide](content/README.md).
- [Network tests and the native protocol bridge](tests/network/README.md) (`crates/ac2-network-bridge/`).
- [Native gameplay changes and validation](docs/NATIVE_GAMEPLAY_PASS.md).
- [Content recovery and reconstruction policy](docs/ADMIN_CONTENT_RECOVERY.md).
- `tools/ac2/`: DAT inspection and reference preparation tools.
- `crates/`: the server's DAT, resource, world, physics and protocol crates.

Retail DATs, original client files, decompile archives, and generated runtime assets
remain local inputs. Keep the licenses and notices included with the reference and
server projects when using or distributing their code.

The server and native bridge retain their AGPL-3.0-only licenses and upstream
notices. See `LICENSING.md` and `crates/ac2-network-bridge/NOTICE.md` for component scope.
The repository includes no license grant for proprietary retail assets. AC1
content, the old native-client launcher, machine-specific SDKs, credentials, and
prior build/capture output are not part of this source project.
