# Pyrealium — Asheron's Call 2 Emulator

Pyrealium is a persistent multiplayer AC2 server. It continues Thwargle's AC2E, which was derived from Vanquish's Pyrealium at `7c2e856840b04302d1c78f057dcf8094326472a4`. It serves AC2 clients over the native protocol. It is a Rust server, independent of the AC1 ACE code and database; it is not an ACE plugin or a claim of complete retail AC2 emulation.

## Running

Start the server with `pyrealium` (or `start-pyrealium.bat`) from the directory holding
`pyrealium.toml`; `pyrealium init` writes one to edit. See the README's Run section for the
database. Game builds use `target/release/pyrealium.exe`.

World time advances while idle and across server restarts. The server keeps its origin
in the database (`shard.world_state`, key `clock_origin`) and sends native time
synchronization every five seconds. Clients use the installed DAT calendar: a day lasts
7,347 real seconds (2h 2m 27s), with lighting and weather selected from its authored sky
keys. New worlds establish their own origin; this does not reproduce the date of a
historical retail server.

For local population testing, enter `@spawns` in chat while outdoors. It lists
the nearest five live imported creatures with map coordinates and distance.
This requires local test map travel (enabled by the default loopback launcher).
It does not add spawns or bypass interaction distance checks. A verified Edge
Beetle location is approximately **42.2N, 7.0W**, cell `76B5002B`.

The regional wilderness expansion adds 3,495 creature slots in 2,071 managed
groups, using 326 native creature templates across 21 regions. The prepared world
now has 9,801 placements; 8,056 outdoor creature points span 3,623 landblocks.
Two-client network tests found, killed and looted Drudge Outcasts around
**38.6N, 22.6W** (Esper Reaches), Akeha Mimbu around **10.3N, 40.6E** (Kydi Delta),
and Undead Stony Armoredillos around **15.0S, 4.7W** (Rings of Oriad).
These are inferred placements grounded on native terrain, not recovered retail
generator coordinates. See the regional population section of the recovery guide.

Wild creatures now use native perception and defined wandering controls. Their
movement reads native motion descriptors, inherited visual scale and captured
instance scale instead of giving every creature the same 2 m/s pace. Speed and
acceleration apply to pursuit, wandering and homeward travel, with short native
collision checks and positions shared with nearby players. Corpse loot combines observed item
profiles, recovered native trophy rates and the versioned inferred treasure
policy. Supported equipment can roll persistent armor ratings and palette tints.
There is no universal potion fallback. Full retail treasure probabilities,
weapon/jewelry affixes and AI/generator scripts remain unverified. See
`content/README.md` for catalog coverage and spawner group configuration,
and [content recovery](../docs/ADMIN_CONTENT_RECOVERY.md) for the inferred quest,
camp and loot rules and their evidence.

Attacking retains the selected target and combat stance, including after a kill.
Use Z/Stop Combat to return to peace before changing equipment. Corpse pickup,
Loot All and Transmute All temporarily enter peace and restore combat afterward,
without restarting attacks on the corpse.

`start-pyrealium.bat` (or `Start-Pyrealium.ps1`) builds the server if it is missing and
runs it with `pyrealium.toml` beside the script, writing a commented one on the first run.
Parameters: `-Config`, `-Rebuild`, `-SkipBuild`, `-Stop`. Rust is needed for builds, not for
starting an existing binary. In another terminal run `Launch-AC2.ps1 -Online`. Sign in with a
new account name (1–32 letters, digits or underscores) and a password (12–128 characters).
First login creates the account; later logins verify the saved password. Account names are
case-insensitive. The server owns character creation, progression and inventory.

Starting a second server on the same database fails with a clear message; stop the first
(`pyrealium stop`) before rebuilding or changing its settings.

## Initial gameplay ruleset

The compatible capture pack was audited at 5,606 placements, including 4,428 combat point spawners and 78 vendor identities. The separately versioned inferred pack adds 702 accepted placements in 143 camp groups plus noncombat NPCs, with 88 quest profiles. Counts depend on the installed content and recovered evidence; the generated report next to the prepared TSV is authoritative. Version mapping, per-cell snapshot selection and duplicate/dead-object filtering withhold incompatible observations. Authored portal routes and compatible lifestone fixtures use the native binding/recall handlers. Recovered NPC or vendor identity does not imply complete scripting or retail stock.

The authored starter encounter supports targeting, range-checked melee/missile/magic attacks, learned racial starter skills, retaliation, health/vigor, death/despawn/respawn, XP and gold. Claimed loot is saved with the character (periodic save, logoff, shutdown) and survives restart; unclaimed corpse contents expire. Separate authored tutorial rewards remain independent of recovered creature loot tables. The Legions profile validates racial starter gear, and equipped weapons publish their native modes/animations to nearby players. The human starter circuit can be completed through its reward, outdoor exit and recall; it is authored development content, not full retail tutorial parity. Complete quest/crafting scripts, original loot generation probabilities and affixes, PvP and complete anti-cheat enforcement remain unfinished.

The native server/client layer also supports persistent character appearance, multiple players, scene visibility, cell changes, server position corrections, local/tell/emote chat, owned inventory replication, item use/equipment requests, death/resurrection, and authenticated ticket login. Client compatibility details and verification commands are in `../tests/network/README.md`.

Unarmed starter attacks now publish native martial-arts mode and attack stories;
supported starter swords retain one-handed mode. The Idol uses Melee_Normal1,
which exists in its Legions animation map, instead of the unavailable Auto1 key.
The scene snapshot retains the selected combat mode for observers entering later.

## Authenticated hosting

The account service runs inside the game server at `http://127.0.0.1:19400` (`[accounts]
web_bind`); the client login form defaults to that address. `[server] game_host` sets the
game address returned to clients. Accounts and login tickets live in the database's `auth`
schema; back the database up with `pg_dump`. Ticket mode rejects development/no-ticket
login. The client can also use `Launch-AC2.ps1 -Online -AccountUrl http://127.0.0.1:19400 -Account NAME`.

To host the account service apart from the game, set `[accounts] enabled = false` for the
game server and run `pyrealium accounts` with the same settings file elsewhere; both use the
same database.

Reserved names (`local`, `admin`, `administrator`, `system`) and names that already own
characters without an account (development logins) cannot be registered. To use a
development account locally, set `[server] auth = "developer"` (loopback only).

For external hosting, put the loopback account service behind a TLS reverse proxy and set
`[server] bind = "0.0.0.0:PORT"`, `game_host`, and `[accounts] public_origin =
"https://accounts.example.com"`. The proxy must overwrite `X-Forwarded-For` with the client's
single IPv4 address; tickets bind to that address. Publish both UDP ports. Remote deployment
itself has not been performed.

The account backend serves `/source.zip` with the source used to build this modified server, preserving the AGPL source offer. Build after changing source; do not point the offer at unmodified upstream. `export_source.py` packages the current server source, docs, dependency lockfile, and build/launch helpers using explicit source directories. It works in this monorepo and in an unpacked source download, and excludes game DATs and any database contents.

## Local map testing

The default loopback launcher enables `--test-map-travel true`. A client sends
`@maptravel LAND_X LAND_Y`; the server announces the private
`[Pyrealium:map-travel-v1]` capability to clients when it is on. Land coordinates cover
`[0, 2040)` on each axis. The server decodes height samples and triangle diagonals
from LandscapeDefs / CLandBlockData, rejects absent ground, cave openings and
unwalkable slopes, persists the move and replicates it through normal visibility.

Travel requires a living character out of combat and has a five-second cooldown.
It does not grant tutorial progress. `-DisableMapTravel` disables it in the
launcher. The raw server defaults to disabled, and rejects the feature on a
non-loopback bind. Existing Pyrealium servers without this capability retain
normal gameplay travel. This is a testing extension, not a retail feature.

## Implementation notes

### Shelter continuation, armor and effects

The Shelter Guard, Lieutenant, Sentry, Scout and Advance Scout use native DAT
StringInfo dialogue. Training stones show their original text. `@quest shelter`
reports the ordered per-character progress. After the introductions:

1. Defeat a Drudge Digger and collect the Imbued Digging Tool.
2. Speak to the Shelter Scout within four metres, out of combat. The tool is
   consumed once; the Scout grants the three native tutorial potions and 40 XP.
   The inventory must have room for all three rewards after consuming the tool.
3. Defeat a Drudge Sentinel for the Menhir Glyph. Select the gateway and use the
   glyph in the backpack while nearby. Its consumption and Vault placement are
   saved together. An unrelated or distant target cannot consume it.
4. Speak to the Advance Scout in the Vault, then use the shard for the authored
   completion step and 65 XP. The exit portal leads to the outdoor starter hub.

Each exchange changes the character's quest stage, inventory and progression together and
is saved with the character. They preserve inventory, progression and travel across restart.
Failed or repeated exchanges grant
nothing. The glyph gateway can be reused after unlocking it, allowing return
after death or recall without requiring another consumable glyph.

Placement provenance matters: the Guard/Lieutenant mirror the observed 0298
training instance; its corresponding cells match 0299 after the identity word.
The Advance Scout at 03f40154 is an observed placement. The new Sentry, Scout,
two Drudges, gateways and shard use explicitly authored coordinates in
`training-world.tsv`, not recovered retail spawn locations. The shard currently
uses the installed Master Osteth Menhir asset and an authored completion rule;
the original Menhir history/cinematic has not been reconstructed. Scout potion
items are real DAT definitions, but their reflect, regeneration and speed buffs
are not yet executable. XP, deterministic drops and encounter timing are Pyrealium
development policy. The separate three-Idol Training Grounds circuit remains.

Seven additional armor profiles are validated against the installed entity,
qualities and physical records: Human 47001951/1952/1954, Lugian
4700195E/195F/1960, and Tumerok 47001968. They supply torso/legs/feet for Human
and Lugian and a torso for Tumerok. Native race/sex selectors choose appearance
tables; worn pieces contribute their DAT armor values to development damage
mitigation. Once the relevant Shelter quest drop is owned, additional Digger or
Sentinel kills grant missing race-compatible profiles. This deterministic loot
policy is not an original retail loot table. Unequip the existing slot first.

Starter and additional armor use saved full VisualDesc updates for the owner,
nearby players, late observers, selection and re-entry. Character creation traits
remain intact. No debug command grants arbitrary item definitions.

Native InitializeMissile and moving PhysicsDesc snapshots carry target, offset,
velocity and instance. Server health changes remain authoritative.

The server now imports native skill costs, equipment/target requirements, range,
cooldowns, rank curves and core effects from the installed DATs. The executable
subset includes 224 racial profiles: Flame Bolt applies its damage and periodic
burn, while supported healing, vigor, regeneration and combat-rating effects
use the same cast path. Train the skill and equip the required implement first.
Nine shared mastery skills can be trained and raised for the supported races;
their ranks contribute to combat ratings. See `content/README.md` and
`skill-runtime.report.json` for the exact scope and pending profiles.

This does not finish every retail spell effect. Magic damage remains a server
impact with an authored release delay rather than a native simulated missile.
Native ballistic parameters, scripted missile creation, target cloning,
sound/light/streak FX, area/multi-hit timing, combos, summons and specialized
statuses need further work. Human/Lugian starter casts select Magic_Nuke1 and
Tumerok Flash selects Magic_Harm1; normal magic auto-attacks keep Magic_Auto1.
Unsupported required effects withhold the cast rather than consume vigor for
a silently incomplete status effect. Non-lifestone recalls remain unavailable.

`tests/network/test_shelter.py` verifies the complete implemented continuation with two
UDP clients, including three additional armor appearances and server restart.
`tests/network/test_armor.py` covers the starter wardrobe on all six supported bodies.
These checks do not establish full retail quest or spell parity.

- Skill metadata (1,927 resolved definitions; 186 unresolved) and 30 advancement curves were regenerated with the existing strict DAT readers. Unsupported records remain unsupported. Server metadata validation remains active for supported skills, inventory and loot.
- Ordered network counters wrap as native 16-bit stamps; a reordered rollover is covered by a transport regression test. Client re-entry keeps the upstream-compatible incoming sequence.
- `emulator` retains the `pyrealium` library name for existing tests/modules; the executable is **pyrealium.exe**. Original licensing/notices remain intact. `UPSTREAM.md` describes upstream features, not the completion level of this Pyrealium ruleset.
- Upstream native-client launcher assets and launcher-specific startup scripts are omitted. The `deploy/` templates and upstream documentation remain reference material for porting; use the Pyrealium scripts above for this Windows/Legions setup.

Build the server independently with `Build-Pyrealium.ps1 -Test`, or build the server and the native protocol bridge the network tests drive with `../Build-AC2Network.ps1 -Test`. Protocol integration tests are in `../Network`; they use isolated temporary stores. For a manual game-only build, regenerate the source archive with Python `export_source.py`, then run `cargo build --release --locked --manifest-path Cargo.toml --target-dir target/game`. Use the PowerShell build script to also refresh the stopped account-service executable.

Inventory drags reorder the native dense backpack list, including adjacent slots.
Release a backpack item over the world to drop the whole item or stack at your feet;
double-click it within four meters to pick it up. Unequip worn items first. Nearby
players see the same dropped model and may pick it up. A drop or pickup and both
characters it touches are saved in the same transaction and survive server restart. Ground items are
bounded to 512 per server; partial-stack world drops remain unfinished. Corpse
containers now support owned multi-item drops, single pickup and Loot All.

The reconstructed Vault arrival and Advance Scout now use floor positions clear
of the authored Menhir Shard. Logging in at the old obstructed arrival moves the
character to that clear landing. Native DAT collision tests cover
both supported capsule extremes and walking away from the landing/scout.
`tests/network/test_world_drops.py` checks two-player ownership and restart recovery;
`test_shelter.py` covers the tutorial continuation and old-arrival recovery.
