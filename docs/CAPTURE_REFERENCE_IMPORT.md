# Packet corpus and historical emulator import — 2026-10-29

> Historical work log. Some checks recorded here ran the former Unreal client (its `Launch-AC2.ps1`,
> `Unreal/Saved` captures and `*_ui.py` tests), which is no longer in this repository.

The supplied MEGA packet collection and Pyreal emulator sources are available
locally under ignored `Saved/PacketCorpus` and `Saved/ServerReference`. Seventeen
selected source/capture archives were downloaded, SHA-256 hashed and verified
against their MEGA download MACs. Download verification is distinct from native
packet checksum verification; captured packet checksums are not used as proof
of authenticity. Historical game clients and installers were not downloaded.

`content/capture-reference-sources.json` records archive provenance.
No downloaded executable, installation script or SQL statement was executed.
`AC2 Users.rar` is password protected and remains unopened. One nested website
backup has unsafe member paths and was withheld. Historical landblock exports
remain separate from the installed Legions DATs.

## Concrete changes

- The combined collection contains 1,188 distinct captures (3,450 copies),
  13,044,788 reassembled messages and 57,658 sanitized world-object observations.
  PCAPNG, multiple interface clocks, out-of-order fragments, retransmissions,
  flow/session isolation and bounded large offline patch records are supported.
- A native offline decoder uses the pinned Rust protocol implementation rather
  than guessing offsets through optional object descriptions. Only world-object
  metadata and script function IDs are emitted. Player avatars, attached/carried
  items, pets, corpse ownership, private inventory, chat and login payloads are
  excluded from the published evidence. Raw captures remain local.
- `capture-spawns-audited.tsv` adds **121 observations in 86 previously uncovered
  cells**. Installed DATs classify 27 as encounter candidates, 12 as vendors,
  two as doors and one as a sign. Eight named entries resolve their labels from
  installed localization records; captured literal item names are not published.
- The imported pack retains exact captured positions, orientations and scale.
  Initial looted/dead/combat states, active movement/behaviors, position changes,
  attached objects and unsupported visual overrides are excluded. A capture may
  show an actor standing somewhere without proving that location is its spawn
  origin; these remain observations, not a reconstructed retail spawn schedule.
- New cells take one admissible capture snapshot each. Earlier cell snapshots
  retain ownership, including when their incompatible rows are withheld. This
  prevents mixing object lifetimes and piling multiple town versions together.
- The audit identified 30 exact historical rows captured in a transient state.
  Preparation withholds 27 of those rows; the other three are already masked by
  earlier cell ownership. Exclusions require the original row's hash, source
  file and IID to match. Edited rows and curated training content are unaffected.
- After compatibility checks and landmark deduplication, the prepared pack has
  **1,392 objects**, up from 1,276. The starter encounter and existing gameplay
  simulation are retained. Newly classified creatures use existing encounter
  profiles; this change does not invent new loot probabilities or spawn timers.

The generated evidence manifest records each admitted row's source capture hash,
message hash, original IID and observation time. The full opcode/behavior/FX
audit is local in `Saved/PacketCorpus/audit-final/`; no raw payload replay is used
to initialize player state.

## Useful historical emulator references

The newest source archive is extracted at
`Saved/ServerReference/extracted/8279c8634a9375c1/`.

- `Source/InternalLibs/Server/World/Interfaces/Entity/StanceTypes.cs` identifies
  native mode IDs from tables `23000026` and `23000048`, including looted
  `41000004`, dead `41000005`, open `41000002` and closed `41000003`. This helped
  identify why captured looted objects must not become fresh active placements.
- `Source/InternalLibs/Server/World/World/Entity/Living/Monster.cs` documents
  combat stance changes, pursuit and corpse creation. It defaults to 30 m
  perception, 60 m homesickness and a two-second attack interval, and creates a
  separate corpse after a 1.2-second timer. Those are emulator implementation
  choices, not verified retail constants. Its later DAT property readers are
  more useful than copying the incomplete decision tree.
- `Documentation/Database` contains 89 lifestones, 160 portals and 1,553 telepad
  records. These are historical references; no historical database was installed
  over the current server or its account store.
- The old `AC2UtilSQL/Data/Npc.dat.sql` has 184 map-label rows, with names and
  approximate north/east coordinates. `Quests.dat.sql` has 162 guide/index rows,
  and `Towns.dat.sql` has 22 labels. These lack full entity IDs, heights, facing,
  spawn schedules and quest execution logic. They are leads for capture research,
  not sufficient evidence to spawn everything at guessed positions.

## Remaining gaps

Many captures use entity/model combinations incompatible with this installation.
The selected inputs still include 1,743 rows missing required DAT records and
14,029 rows whose visuals cannot be verified against installed templates. Raw
source rows remain unchanged and the preparation report explains every exclusion.
Matching names or numeric DIDs across builds is insufficient to restore them.
Do not overlay historical landblocks or portal records onto the installed DATs.

Retail spawn generators, full roaming paths, vendor stock, quest conditions,
respawn schedules, aggression rules and probabilistic loot tables have not been
reconstructed by this import. No Unreal rendering or physics implementation was
changed in the original import pass. The follow-up below verifies two outdoor
observations over the network and one rendered creature; the remaining imported
appearances have not all been visually inspected.

## Reproduce

Build the offline decoder with the repository's Rust environment:

```powershell
cargo build --release --locked --manifest-path crates/ac2-network-bridge/Cargo.toml --bin ac2-capture-decode
python tools/audit_capture_corpus.py --root Saved/PacketCorpus/extracted --root Saved/ServerReference/extracted --decoder target/release/ac2-capture-decode.exe --output Saved/PacketCorpus/audit-final
python tools/import_audited_population.py --audit Saved/PacketCorpus/audit-final --base content/world-observed-expanded.tsv --base content/training-world.tsv --base content/capture-spawns.tsv --base content/capture-spawns-upstream.tsv --game-data "Asheron's Call 2 - Legions" --output content/capture-spawns-audited.tsv --report content/capture-spawns-audited.report.json
python tests/network/prepare_content.py "Asheron's Call 2 - Legions" data/world/legions-world.tsv
```

The normal launcher repeats content preparation. A stopped server will load the
new pack on its next normal start. Account and character stores are not migrated.

Focused verification covers capture framing, conflicting fragments, flow
isolation, native event decoding, private-state exclusion, stationary placement
selection, stale exclusion evidence, DAT compatibility and runtime loading with
service/encounter classification. Raw game captures are not required for the
synthetic parser regressions; the runtime content test uses `AC2_TEST_GAME_DATA`.

All 25 focused tests passed (21 Python, three decoder tests and one DAT-backed
server content test). The production server also loaded all 1,392 objects and
165 portal routes with the normal launcher gameplay flags in a separate temporary
store on loopback ports 19970/19971, then shut down after three seconds. No login
was performed and the production account/character stores were not used.

## Outdoor runtime verification

After the NPC classification correction below, the current pack contains 510
active combat encounters, including 135 outdoors across 64 landblocks. The
earlier 562/183 counts incorrectly included NPCs. The larger placement count includes services and
landmarks. This is sparse capture coverage, not a complete world spawn system.
No supplemental creatures or guessed locations were added.

`tests/network/test_outdoor_population.py --render` uses the prepared pack unchanged
and two disposable characters under a local test account. Both clients receive the authored
positions, health and combat-mode updates for:

- Edge Beetle `4700088C`, IID `81409BEC26CB5AF8`, cell `76B5002B`, local
  position `(101.744698, 52.311005, 138.205872)`; approximately **42.2N, 7.0W**.
- Creature `47000F7E`, IID `8140B739FEA6EC9A`, cell `76B50001`, local
  position `(6.200933, 18.482271, 129.885986)`; approximately **42.1N, 7.4W**.

Native geometry verifies both captured heights within 2 cm of their terrain.
The rendered client loads and selects the Edge Beetle with its DAT model and
server health. The test observer uses a separately verified floor position;
offsetting X alone placed that test character below the adjacent hillside.

The server previously rejected movement and combat presentation for creatures
whose initial PhysicsDesc omitted the tutorial model's mode/slider fields.
Updates now handle all three layouts emitted by the server serializer. A mode
is added when required, preserving the pose, name and scale. Regression tests
cover late-observer snapshots and death/respawn for each layout. The existing
two-client pursuit test still crosses a landblock boundary and returns home.

This verifies these observations and presentation paths, not retail aggression,
roaming, spawn generation or loot probabilities throughout the world.

### Captured creature pursuit follow-up

The encounter controller had a second tutorial-only gate: it enabled pursuit
only for PhysicsDesc `0x8082`. It now asks whether the record supports pose
updates, accepting all three layouts emitted by the server. Creatures still
move only along native collision-checked walking steps. They also turn toward
targets already inside melee range without adding fake steps to their return
route. Captured home coordinates and population selection remain unchanged.

Authoritative position packets now encode neutral motion as `0x7f7f7f`.
The previous zero word decoded as `(-1,-1,-1)` under the native
`Vector::UnitVectorFromUInt24` quantizer. Heading uses native truncation.

The outdoor UDP test now verifies both real observations moving uphill/downhill,
matching updates on two clients, current snapshots for a joining observer,
retaliation, and a stepwise return after the attacker leaves. Native geometry
checks the entire 12 m test path at both locations. The boundary pursuit test
continues to pass. The rendered test equips the starter staff, casts once, and
requires changing native locomotion poses and retaliation from the Edge Beetle;
the verified run recorded 235 changing movement frames and a 10-point hit.
Screenshots are `Unreal/Saved/Screenshots/AC2Population-outdoor.png` and
`AC2Population-pursuit.png`.

Automatic proximity aggression remains unimplemented. The DAT contains
`AI_PerceptionRadius` (`4100017A`), but the examined actors also inherit
`AI_LoveTable` (`4100018A`, values 6 or 22) and AI script names. Their hostility
semantics have not been established. The older emulator attacks any nearby
player; that alone is not evidence that every retail creature should do so.

## Additional archive audit and stationary FPS — 2026-10-30

Two more archives from the supplied MEGA tree were downloaded and verified:

- `6 AC 2 Portal and Landblocks.7z`, SHA-256
  `4139266d0df5d2c94992c4f064325bb0085bc184dc8bb62eb2fed95019c80728`.
  Its 3,422 landblock-info records decode without errors and contain 20,967
  placements of static-object type `40000001`, spanning 1,516 entity definitions.
  None intersects the current combat definitions. Its `14000000` encounter
  record is empty, as in the installed client. The included portal archivever
  supplies no additional exact entity/model matches for the 14,029 withheld
  capture rows. These historical records were not overlaid onto live DATs.
- `3 AC2 Wiki Site.7z`, SHA-256
  `595afb35dbea396a50da74ed9a9ac5ca7ce51f7e4d79001e90a6ecf4dfca51c9`.
  The download passes MEGA authentication, but 7-Zip reports 62,407 member data
  errors. The successfully extracted HTML/PHP files were individually checked
  against archive CRCs: 3,117 pass. Most are empty file-discussion pages; checked
  Arwic/Cragstone pages contain no quest or spawn instructions. Damaged members
  were not used as gameplay evidence. A clean copy could improve reference
  coverage; successful downloading alone does not repair this archive.

The exact output of the landblock audit is retained locally in
`Saved/ServerReference/landblock-placements-audit.json`; archive listings,
extraction logs and `wiki-valid-pages.json` document the extraction limits.
These files supplement the earlier source, SQL and packet review. They do not
establish full retail generators, quest scripts or world population.

The reported constant FPS loss included a CPU bottleneck in interior lighting:
every world material queried every loaded room's floor triangles, including
outdoor grass. The query now rejects rooms by conservative geometry bounds
before performing the unchanged closest-floor test. Bounds include the native
height and barycentric edge tolerances. Missing bounds retain the exact query.
Foliage placement, material quality and draw distances are unchanged.

In the fixed online 7EBA scene at 1600x900, 8 seconds of warmup followed by
15 seconds of stationary sampling measured **10.449 ms/frame before** and
**5.079 ms/frame after** (about 96 versus 197 FPS). Interior lookup fell from
5.389 to 0.155 ms/frame. Both runs retained 4,170 grass batches and approximately
802 draw calls. This is an offscreen rendered test on this machine, not a claim
that the user's full slowdown was reproduced or every location is fixed.
The final report is
`Saved/AC2NetworkTests/performance-39d021987c4942c79f1251096531b6d6/performance.txt`.

Run `python Network/profile_client.py` after building the client and server to
repeat the scene with a disposable character/store, audio and native networking.
The opt-in `-AC2Performance` client mode records frame, game/render thread, GPU,
streaming, audio, network and lighting times under `Saved/AC2NetworkTests`.

Game builds now install to `target/release/pyrealium.exe`. The normal launcher
prefers this copy, preventing an older executable locked by standalone accounts
from silently remaining the game server. Accounts keep their existing process;
the build reports when refreshing that executable must wait. The launcher test
verifies account tickets across the separate copies, repeated startup, ownership
checks and clean shutdown, using temporary stores only.

`@spawns` is a local-testing chat command, gated by loopback map travel. It lists
the nearest five live runtime creatures with map coordinates and distance,
excludes dungeon/dead/despawned actors, and creates no population. The existing
two-peer outdoor test covers the command plus placement, damage, pursuit,
late-observer snapshots, retaliation and return to the captured home. The current
pack contains 135 outdoor encounters across 64 blocks after the NPC correction
below; there are no admitted
creatures within four blocks of the reported 7EBA area. Use the verified 76B5
locations above to test population. World coverage remains incomplete.

## NPC classification and item tooltips — 2026-10-30

`creature-entities.txt` is a historical candidate catalog, not an attackability
list. The encounter loader incorrectly gave every entry without recovered combat
data a generic 60-health profile, damage, XP and potion loot. This made town NPCs
such as Orven killable and included them in `@spawns`. That fallback is removed:
encounters require an explicit authored rule or a recovered combat profile.
Native services and configured Shelter/training dialogue roles take precedence
over either source. In particular, Pectar's template has inherited monster
statistics but is used as an authored training mentor and must remain protected.
The service audit now uses the same positive evidence and dialogue exclusions.

The prepared pack still contains 1,392 placed objects. It now admits 510 combat
actors, of which 135 are outdoors across 64 blocks. No spawn position changed.
At Orven's captured position there are zero admitted creatures within 300 m;
the verified Edge Beetle at **42.2N, 7.0W** is about 562 m away. The survey uses
captured names or catalog labels for presentation only; a label never makes an
object attackable. Full retail spawn generators and hostility scripts remain
unrecovered. The archive limitations documented above still apply.

The DAT-backed content regression keeps exact source rows for Orven, Pectar,
Shelter guides and two real monsters, checking their combat eligibility. The
two-peer outdoor test additionally rejects attacks on Orven and the mentor and
checks that they do not appear in the survey, while retaining its real-monster
combat, movement, retaliation and observer checks.

Item hints previously used `NameTextTemplate`, a fixed-height world label. They
now use DAT tooltip field `F7C721AA` and text child `D5A32D4D`, size to the actual
DAT font's wrapped lines, and stay within the viewport. The rendered corpse-loot
test hovers an item and checks hint content, height and clipping. Its screenshot
`Unreal/Saved/Screenshots/AC2Network-corpse-loot.png` was visually checked: both
lines are visible on the native bordered background above the loot window.

## Population recovered through the matching 2017 client — 2026-10-30

This pass supersedes the 135-outdoor-creature coverage figure above. The main
problem was cross-build identity, not a lack of captured monsters: most captures
use the 2017 client, while the installed client has reassigned entity and visual
IDs. For example, captured `4700265F` describes a Gurog in the 2017 DATs but the
same number describes equipment in the installed DATs. Rejecting mismatched
visuals was necessary; treating those numeric IDs as permanent identities was
insufficient to recover the population.

The supplied MEGA tree contains
`Pyreal/3 AC 2 - Latest Client From 2017/1 AC 2 Legion 2017 Client - NOT WORKING.7z`.
Its 1,699,287,581-byte download passed MEGA MAC verification and has SHA-256
`f581da6f41bb8230fc390892170f87049f83fd50ca2e674097264bb5d00de0eb`.
Only `portal.dat`, `local_English.dat` and `cell_1.dat` were extracted, to
`Saved/ServerReference/client2017`. No executable was run and the installed
client archives were not changed. Archive provenance is in
`content/capture-reference-sources.json`.

`import_versioned_population.py` translates an actor only when its source
visual matches the reference client and its native localized name, AI table,
and role identify exactly one installed template. It preserves the capture's
coordinates, orientation and instance scale. Outdoor positions must pass
native floor-contact validation at the unchanged height; interior room records
must additionally match byte-for-byte between builds. The ground probe accepts
only walkable contact within 0.3 m, including static floors/bridges. This is not
proof of complete pathfinding or every actor's capsule clearance.

The importer selects one capture per outdoor block or interior room and one
initial five-second observation cohort per cell. It does not stack repeated
journeys or later respawns. Healthy initial monsters may subsequently move in
the capture; that no longer disqualifies their initial observation. Services
retain the stricter movement exclusion. Curated/training cells take precedence,
and preparation prevents old capture snapshots from overlapping recovered cells.
The final pack and every source observation have SHA-256 provenance. Preparation
rejects a modified recovered manifest until it is reverified.

The recovered manifest contains **4,217 placements**:

- **3,931 monsters**, including **3,742 outdoors**.
- **261 scripted actors**, including town criers, quest characters, escorts and
  quest objects. This count is not a count of completed quests.
- **25 vendors**, spanning 12 native vendor templates.

The merged installed pack has **5,606 objects**, **4,428 combat actors**,
**3,872 outdoor creatures in 1,642 landblocks**, and **78 native vendors**.
The previous pack had 135 outdoor creatures in 64 blocks. The active encounter
limit now matches the content limit of 16,384 instead of silently stopping at
4,096. Visibility still sends nearby content only.

Combat profiles come from the installed native entity inheritance chain. The
generated catalog has 426 profiles. Native services, existing dialogue roles,
and the recovered scripted-actor catalog override combat eligibility. The
installed relationship labels `AIFriendTable_PlayerFriendly` and
`AIFriendTable_SkyportServitor` protect friendly escorts, mechanisms and Skyport
service actors that inherit damage/health. Their conditional quest combat is
not invented. Pectar and Orven remain protected.

Examples now placed at observed locations include Cutter Beetles near Cragstone
(`78B8002E`, approximately **44.9N, 5.4W**), the Human Weapon Vendor
(`77B70036`), and Katriya the Town Crier (`57720015`). Use `@spawns` on the local
test server to list nearby living creatures and their map positions. It still
creates no monsters. `recovered-population.report.json` records names, cells,
native AI bindings, translated IDs and capture hashes for quest/vendor work.

Reproduction, from the repository root with the sanitized audit and three
reference DATs available:

```powershell
python tools/import_versioned_population.py --audit Saved/PacketCorpus/audit-final --reference Saved/ServerReference/client2017 --installed "Asheron's Call 2 - Legions" --protected content/world-observed-expanded.tsv --protected content/training-world.tsv --output Saved/population-candidates.tsv --report Saved/population-candidates.report.json
cargo run --release --locked --manifest-path Cargo.toml --example population_ground -- "Asheron's Call 2 - Legions" Saved/population-candidates.tsv Saved/population-ground.json
python tools/import_versioned_population.py --audit Saved/PacketCorpus/audit-final --reference Saved/ServerReference/client2017 --installed "Asheron's Call 2 - Legions" --protected content/world-observed-expanded.tsv --protected content/training-world.tsv --ground Saved/population-ground.json --output content/recovered-population.tsv --report content/recovered-population.report.json --protected-entities-output content/population-protected.txt
python tools/import_creature_combat.py "Asheron's Call 2 - Legions/portal.dat" content/creature-entities.txt content/creature-combat.tsv --population-report content/recovered-population.report.json
python tests/network/prepare_content.py "Asheron's Call 2 - Legions" data/world/legions-world.tsv
```

The 5,014 proposed placements yielded 4,217 native ground contacts. The other
797 remain withheld rather than being snapped to guessed terrain. Unsupported
templates, changed interiors, active initial poses and ambiguous identities also
remain excluded. This is observed population coverage, not the retail random
generator schedule. Uncaptured regions remain incomplete.

Validation includes importer ambiguity/cohort tests, content merge/hash tests,
native DAT checks over the complete recovered catalog, and two-client UDP
gameplay with a disposable store. `tests/network/test_recovered_population.py --render`
kills the real recovered Cutter Beetle, verifies the other client sees death,
tests corpse ownership, loot collection/replay rejection and respawn, buys and
sells through the placed vendor, and rejects attacks on the vendor and town
crier. The optional rendered test selects the native creature model after
relogin. Tests do not modify operator accounts or inject test spawns.

Remaining limits: native AI script names identify quest bindings but do not
supply the complete quest conditions/rewards; the supplied SQL quest/entity
tables are empty. Newly placed NPCs support examination where no script exists.
Vendor transactions use the existing limited Pyrealium stock catalog. Creature gold,
corpse drops, respawn timing and combat controller behavior retain explicit
Pyrealium rules; retail loot probabilities, automatic aggression and full roaming
generators have not been reconstructed. Those limits are not resolved merely by
restoring the actor locations.

Final verification artifacts are under
`Saved/AC2NetworkTests/recovered-population-3f64a921affa420ea25fa434884ea973`:
both peers' decoded events, server/client logs, a PASS result and the rendered
selection screenshot. Dense native foliage partly obscures the beetle in that
view; the screenshot is not a claim of complete foliage/rendering parity.
All 3,931 recovered combat placements passed the native catalog eligibility
test; scripted actors and vendors passed the protection checks. The Rust test
run passed 93 library and 4 integration tests (one unrelated library test is
ignored), and 14 focused Python tests passed.

With the larger pack, the existing offscreen 7EBA stationary profile at 1600x900
measured 10.316 ms/frame (about 97 FPS), 2.954 ms/network tick, and 807 average
draw calls over 1,455 sampled frames. This is slower than the earlier sparse
5.079 ms scene; it is not a performance improvement claim or a measurement of
every populated location. The report is
`Saved/AC2NetworkTests/performance-48e8d874c48149b3b6eaa1defb219e5f/performance.txt`.

## Server-managed spawners (2026-10-30)

The 4,428 admitted combat placements now define bounded point spawners, rather
than leaving initial creation and regeneration in each combat actor. Startup
withdraws the imported creature lifetimes before clients can see them. A
deadline queue creates each child, controls corpse removal and refills its slot.
Each point owns at most one lifetime; no replacement can overlap a still-present
corpse. World creation/removal must be acknowledged before ownership changes.
Moving out of a cell or leaving a client's visibility does not refill a slot.
Fixed vendors and protected scripted actors stay outside the controller.

Initial delay and regeneration can be configured with explicit `spawner` rows
in the encounter profile. The default retains Pyrealium's authored 30-second period,
and lootable corpses retain their 60-second presentation. Those numbers are not
claims about retail generator timings. Regeneration restores exact captured
coordinates, quaternion and scale and advances the network instance stamp.
The previous respawn path reduced the captured quaternion to movement heading;
that loss is corrected. `@spawns` now reports managed/live slot counts as well
as nearby living outdoor monsters.

The installed DATs contain 639 templates with inherited generator properties,
and label table `23000106` names 2,111 generator profiles. Native properties
include `GeneratorProfile` (`4100001E`), regeneration period (`4100001F`), initial
delay (`41000020`), and min/max quantity overrides (`41000021`/`41000022`). These
are not complete generator definitions. Many names describe camps and treasure
hoards, so names alone cannot establish a creature group or area density.

The complete parsed placement audit covered 20,784 outdoor and 151,868 interior
objects, with no placed `GeneratorProfile`. Encounter descriptor `14000000` has
zero map/row entries. Five template records remain unsupported and are listed
in the report; the audit does not claim those records were decoded. The 2017
reference additionally has 586 direct templates carrying generator properties,
but no `23000106` label record and the same empty encounter descriptor. The
historical server source classifies generator entity classes as unspawnable;
the supplied SQL tables do not provide generator instances or child lists.

The checked-in `content/native-generators.report.json` records
decoded installed template properties, labels, audit limits and DAT hashes.
Reproduce it without executing any historical binaries:

```powershell
python tools/ac2/population_probe.py "Asheron's Call 2 - Legions" --output Saved/GeneratorAudit/installed-population.json
python tools/audit_generators.py "Asheron's Call 2 - Legions" --population-audit Saved/GeneratorAudit/installed-population.json --output content/native-generators.report.json
```

Regional spawning, weighted child selection, native camp capacities, quest
conditional generators, and original regeneration schedules remain unverified.
They are not inferred from captured mobile actors. The point-spawner fallback
keeps the verified coverage playable while preserving the evidence needed to
replace it when server generator definitions/placements are recovered.

Verification: 99 library tests and four native DAT integration tests passed;
one unrelated optional terrain-corpus test remains ignored. The two-client UDP
run in `Saved/AC2NetworkTests/recovered-population-3bc98c4e49ba4fd3a64fac2cca7bfe58`
passed kill/loot/ownership, vendor buy/sell, protected NPC checks, and exactly one
replacement with a new instance stamp and identical captured placement and
orientation. One observer remained at the spawner while the hunter travelled
to services. The run used a disposable store and stopped its own test server.

## Corpse loot, idle AI and group capacity (2026-10-30)

The separate `ac2-capture-decode --loot` stream reads only world identities and
items contained by world identities. It excludes private player bags, literal
names, killer/claimant IDs and non-string substitution variables. The local
audit scanned 770 unique captures and decoded 16,672 contained-item observations.
Corpse name references and source visuals establish the defeated creature;
localized item identity plus native equipment/type/race qualities map base items
between the reference and installed DATs. Numeric DIDs are not assumed stable
between versions. Each destination supplies its own native visual.

The importer requires every item in an observed bundle to map unambiguously.
Unsupported quantities and incompatible items withhold the whole observation;
copied corpse/message hashes do not increase its frequency. The resulting
catalog contains 698 bundles for 146 monster templates and 222 item definitions.
Runtime metadata validation admits 144 templates, 123 native death-loot profile
bindings and 218 items, withholding four items and 21 affected bundles. A Cutter
Beetle yields the captured Wood Dagger; observed Red Soldier Wasp bundles include
multiple weapon/apparel items. There is no generic potion replacement.

The observation counts are used as Pyrealium sampling weights. They are not retail
probabilities. Captured randomized affixes, enchantments, quality rolls, original
gold generation, empty-drop probability and unobserved loot tables are still
unrecovered. The checked-in report records source/corpse hashes and translated
items without shipping raw packet captures or player inventory data.

Reproduction uses the same local sources as the population audit:

```powershell
python tools/audit_loot_captures.py --manifest Saved/PacketCorpus/capture-files.json --root Saved/PacketCorpus --decoder target/release/ac2-capture-decode.exe --output Saved/PacketCorpus/loot-audit
python tools/import_loot_tables.py --audit Saved/PacketCorpus/loot-audit --reference Saved/ServerReference/client2017 --installed "Asheron's Call 2 - Legions" --output content
python tools/import_creature_ai.py Saved/GeneratorAudit/installed-population.json content/creature-combat.tsv content/creature-ai.tsv
```

Native AI fields are inherited for 426 combat templates. Perception and
homesickness constrain acquisition; native geometry checks visibility and every
movement step. Explicit wandering disable/zero-probability flags are respected.
Defined wandering ranges produce nearby patrols without requiring a hit. The
missing-probability fallback (0.2), one-second opportunities, uniform disk
destinations, movement speed and generic hostility exclusions are documented
Pyrealium behavior until the original relationship/state scripts are recovered.
The native Cutter Beetle has 25 m perception but no defined wandering range;
the Dank Vermin Scamp has 15 m perception and a 10 m wandering range.

The spawner controller now accepts an explicit multi-child roster with a shared
capacity. Pending creation, live actors and corpses occupy capacity until actual
removal. Unit tests exercise three candidate anchors with capacity two, and a
live two-client fixture exercises two simultaneous children at captured anchors.
This is runtime support, not evidence that those two captures belonged to one
retail generator. No production multi-child membership/caps were fabricated:
the generator audit still has no recovered placed child rosters or min/max
quantity override values. Nearby captured actors remain independently owned
points until such evidence is recovered.

Targets and stance survive individual attacks, rejected actions and creature
death. Explicit peace/stop actions still stop combat; selecting a different
noncombat object stops the old attack queue. Left mouse selection commits on a
click release, so an orbit/steering gesture does not clear the target on press.

Verification includes native DAT admission, spawner lifecycle/capacity,
multi-item ownership/partial pickup/reopen/Loot All, persisted claimed inventory,
two-client roaming and autonomous aggro, and rendered combat target retention.
`tests/network/test_loot_ai.py`, `tests/network/test_corpse_loot.py` and
`tests/network/test_recovered_population.py --combat-render` use disposable stores.
The latter also checks the captured vendor and protected quest-actor paths.

Final verification artifacts:

- `Saved/AC2NetworkTests/loot-ai-72747826d1e24038b087b888dd1dff92`: two-client idle
  roaming, shared movement, automatic aggro, retained target/stance, explicit
  two-child group, multi-item loot, partial pickup and replay/ownership checks.
- `Saved/AC2NetworkTests/corpse-loot-e4adc1e138b04938bca903fa3eb9352f`: single
  pickup/Loot All, range/owner checks and claimed inventory across restart.
- `Saved/AC2NetworkTests/combat-ui-7d20400c6a0b4d7aa79f45b3d74b01e7`: rendered
  native combat buttons plus all ten mouse-control steps, including target
  retention through camera drags, steering and release.
- `Saved/AC2NetworkTests/recovered-population-2a1ce4c359a042128edcb8dea179b7db`:
  shared kill/respawn, exact captured anchor/orientation, vendor buy/sell and
  protected NPCs; rendered ranged hit, target retention, animated pursuit,
  retaliation, native loot tooltip/icon and item double-click pickup. Foliage
  partly obscures the creature; this is not proof of complete visual parity.
- `Saved/loot-ai-icon-test`: the native visual icon test includes the recovered
  Wood Dagger's four layers. The rendered loot check rejects unknown-item
  placeholders and verifies the actual painted layers. Loot now reads the
  dropped object's visual/aspects, using the same layering and tint rules as
  inventory; it no longer relies solely on the entity's generic descriptor icon.
- `Saved/AC2NetworkTests/gameplay-307afc28d692407dab16d875beb85c53`: starter
  circuit combat/armor, portal exit, recall, death/resurrection and restart.

Both Unreal targets and the game server built successfully. The server suite
passed 103 library tests and 16 integration tests with the installed DATs;
four optional tests remain explicitly ignored. Five capture-decoder tests and
four loot-import admission tests passed. All live tests used disposable stores
and shut down their own servers; the existing account service was untouched.
