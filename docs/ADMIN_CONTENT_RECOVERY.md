# Admin client content recovery

> Historical work log. Some checks recorded here ran the former Unreal client (its `Launch-AC2.ps1`,
> `Unreal/Saved` captures and `*_ui.py` tests), which is no longer in this repository.

This pass uses the local build-7148 admin decompile, build-7160 client reference,
the installed Legions `portal.dat` / English string tables, and the pinned
AC2_Rusted binary readers. The DATs and source captures remain unchanged.

## Changes enabled in the server

- Native gateway and road-sign script classes now enter the readable-sign service.
  Within four meters, use sends the original localized Description StringInfo.
  Missing descriptions fall back to examination without pretending to run a quest.
  Cragstone's gateway text describes Lake Blessed, the River Prosper and Kingdom
  Shrines. It no longer reaches the generic unimplemented-action response.
- Examination includes available native descriptions, including NPC biographies
  such as Orven's Arwic Factotum description. This does not invent a conversation
  or grant a quest merely because an NPC has descriptive text.
- `native-trophies.json` supplies 482 explicit trophy slots for 333 creature
  templates. Runtime validates the complete EntityDesc inheritance fingerprints,
  item records, visuals and item transport before admitting a rule. Native
  `NPC_TrophyDropN_Rate` probabilities replace guaranteed copies of that same trophy
  in observed bundles. Each slot rolls independently once per corpse; loot-proof
  creatures still drop nothing. Thirty-six of these templates appear in the
  current imported combat catalog.
- `native-loot-limits.json` recovers inherited generic death-loot quantity bounds
  for 312 templates. `NPC_DeathLootMinQuantityOverride` (`410002f5`) and
  `NPC_DeathLootMaxQuantityOverride` (`410002f6`) constrain the generic item count;
  Baron Colier has 3–6 slots. Complete inheritance/type-registry hashes must match
  the installed DAT before a rule is used. `GameplayStatistics_Level`
  (`41000173`) supplies a native level fallback for reconstructed item rolls.
  Quality/variance overrides (`41000275` / `41000276`) are retained as evidence;
  their retail mutation formula is not recovered. Uniform quantity sampling and
  generic item selection remain reconstruction. Native trophy slots are separate,
  loot-proof actors yield nothing, and explicit operator loot tables take precedence.
- `treasure-weapons.json` recovers 69 observed weapon identities, native hand slots,
  implement types and minimum use levels. Runtime fingerprints the entity,
  qualities, physics and visual records. The generic pool now includes these
  weapons, limited by creature level. Overcharged Hive remains excluded because
  its additional `0x2d8` use restriction is not implemented. The archived Pyreal
  `IntStats.cs` identifies `Usage_MinLevel` as `0x2bc` and `Weapon_Damage` as
  `0x258`; its inventory manager confirms the level gate before equipping.
  All 69 weapon defaults omit damage. Pyrealium reconstructs it as
  `(6 + 2 * treasure level)` with ±15% per-item variance. Damage, treasure level
  and native use level appear in inspection; equipped damage feeds attacks and
  supported weapon-based skills, without being added to periodic ticks.
  Distinct `W` journal grants preserve these rolls through pickup/equipment and
  restart. Existing unrolled values remain unchanged. The importer is
  `tools/import_treasure_weapons.py`; the real UDP acceptance test
  is `tests/network/test_weapon_loot.py`. Neither the formula nor the existing
  three-gold equipment sale policy claims retail economy parity.

The recovery pass did not recover random item affixes, every item stat,
special-trophy eligibility, or the full retail treasure generator. Capture
frequency is not a retail probability. The separately authorized inferred
implementation below now supplies variable generic treasure, armor and weapon rolls.

## Inferred gameplay pack v1 (2026-10-30)

The user authorized best-guess quest/spawn/loot implementation using the content
recovery section. `inferred-content-v1.json` separates those authored decisions
from unchanged DATs, captures, SQL and reference catalogs. This is playable
reconstruction, not a claim to have recovered retail server scripts.

### Enabled quests and population

- **88 quest profiles**: 55 counted hunts, 29 boss hunts, three hunt elixirs and
  Gambling Debt. Native death effects identify hunt bestowers and eligible
  targets; native phase text supplies counts. Each profile preserves its source
  package and any matched guide URL/hash. Generic native "Incursion" titles
  are disambiguated with their targets.
- **143 camp groups**: 129 from old guide/SQL coordinates and 14 near compatible
  captured creatures. These cover 61 hunt/boss profiles; the remaining profiles
  can advance wherever their native starter/target templates are already placed,
  but do not imply newly located camps for every quest.
- **702 new accepted placements**: 676 creature candidate slots plus 26 NPCs.
  Native terrain/collision checks reject nine candidate slots. The resulting
  groups allow at most **548 simultaneous creature lifetimes**, including
  lootable corpses. All 702 pass installed entity/visual/cell compatibility.
- Regular camps nominate one starter and four target slots, normally with a
  four-child cap; boss camps nominate a starter and boss, with a two-child cap.
  Caps shrink when grounding rejects candidates. A missing starter rejects the
  camp. The controller owns initial creation, corpse retirement and regeneration;
  these are not static creatures inserted on every player login.
- The coarse guide point is searched within 32 metres for dry, walkable native
  ground with a clear route through the camp and six-metre separation from
  existing server placements. Exact retail camp centres, facing, densities and
  generator regions remain unknown. Child regeneration is inferred at 120 seconds.
  Native perception, wandering controls and homesickness are retained for the
  169 creature templates used by this pack. No foliage placement is changed.
- The additional NPC placements remain noncombat. Their placement alone does not
  enable dialogue trees, vendor stock or arbitrary scripted interactions.

Kills bestow/advance solo hunts and pay completion XP once. Guide reward/level
values are used when matched; otherwise the explicit fallback is at least
100 XP, or 20 XP per target level per required kill, with a minimum level of
target level minus three. Missing timers default to a one-hour objective and
20-hour repeat. A detail page wins a conflicting index timer; this choice is
recorded rather than treated as certain retail behavior. The native trophy
probabilities remain authoritative where available.

Beetle Slayer, Wasp Bane and Armoredillo Doom consume their owned native hunt
elixir and create a journal objective atomically. They are quest starters, not
health potions. Gambling Debt uses the guide's level 11 requirement, Kanten's
Bag, delivery to Orven in Arwic, two-hour limit/repeat and 1,000 XP reward.
Double-click Orven or target him while using the bag. A successful hand-in consumes
the bag and pays only once. Expired jobs can be renewed with Kanten, reusing the
existing bag. The source describes a random Kanten quest set; only this option
is implemented. Because the missing Kanten1/Kanten2 definitions do not establish
which ID is Gambling Debt, it uses reconstruction ID `4000fe01`.

### Treasure policy and persistence

Creatures with a supported native combat level now use an explicitly inferred
variable treasure policy instead of repeating an entire observed bundle:
one sampled observed item at 35%, plus independent 70% and 25% rolls from the
supported apparel/weapon/restorative pool. Duplicate item definitions are suppressed.
Unsupported quest/special items are not inserted into the generic pool. Native
trophy slots are then applied at their recovered rates, and loot-proof creatures
remain empty. These percentages are Pyrealium tuning, not extracted retail weights.

Supported armor gains persistent rating, treasure level and native palette tint.
Rating varies from 75–125% of a known base, or an inferred base of twice creature
level plus two where no base exists. Inspection shows the roll, equipped defense
uses it, and the native appearance receives its tint. Original equipment models
and race restrictions remain in force. This does not implement the full retail
weapon/jewelry affix system. Existing unrolled items keep their historical values.

`q6` journal events bind hunt credit, timestamps and rewards; `q7` binds elixir
consumption and acceptance; `q8` binds delivery inventory/reward/state exchanges.
Rolled armor grants use `G` and weapon grants use `W`, distinct from historical `g` grants. Old journal
events retain their old meaning. Corpse loot rolls once, reopening never rerolls,
and claimed rolls survive pickup, equipment changes and restart. Keep v1 rules
immutable after release; changed rewards/quest semantics need a new journal
policy version and migration rather than editing historical replay behavior.

### Regeneration and verification

From the repository root, regenerate the separate pack without editing references:

```powershell
python tools/author_recovery_content.py
python tools/import_creature_combat.py "Asheron's Call 2 - Legions/portal.dat" content/inferred-creature-entities.txt content/inferred-creature-combat-v1.tsv
cargo run --release --manifest-path Cargo.toml --example population_authoring -- "Asheron's Call 2 - Legions" content/inferred-content-v1.json content/inferred-world-v1.tsv content/inferred-world-v1.report.json data/world/legions-world.tsv
python tests/network/prepare_content.py "Asheron's Call 2 - Legions" data/world/legions-world.tsv
./Build-Pyrealium.ps1
```

The grounding report records the source/manifest hashes, existing outdoor-world
hash, rejected slots and exact accepted group roster. Preparation rejects stale
or mismatched inferred manifests. Tests cover all quest completions, timers,
levels, replay-safe rewards/consumption, rolled armor persistence/defense/tint,
varied Vermin treasure, native ground and noncombat roles, group capacities and
starter availability. `tests/network/test_inferred_content.py` exercises real UDP
delivery/targeted hand-in, elixir use, a five-kill incursion across two camps,
native journals and a full server restart using a disposable test account store.

## Ringway exploration pack v1 (2026-11-01)

The existing Arwic, Lost Wish and Esper Explorers now accept and advance their
regional Obelisk Search quests (`40000016`–`40000018`). Use the ruby, sapphire
and emerald obelisks in order, then return to the same region's Explorer for
80 XP and the Find the Arwic Mines Overseer referral (`400000c0`). Speaking to
the Overseer completes that referral without an additional XP reward. These
interactions send native NPC speech, MessageDialog and quest-journal updates.

`exploration-content-v1.json` keeps the recovered native entity/quest IDs and
all four journal phases, including the ringway directions. The cached wiki
articles supply the 24-hour search deadline and 20-hour repeat cooldown, and
the referral's 48-hour deadline and 24-hour repeat. Cooldowns start at completion;
expired searches restart through their Explorer. This is an explicit server
reconstruction, not recovered executable NPC-table bytecode. Completion dialogue
is authored; source URLs, article hashes and native text-record hashes remain in
the policy. Native obelisk visuals are used.

Nine stationary obelisks are placed near the outgoing ringways named by the
next journal step. Their coarse anchors are Arwic: 46.9N 0E, 42.1N 8.5E,
34.3N 2W; Lost Wish: 53.1N 9.9E, 53.5N 19E, 47.3N 21.4E; Esper:
41.2N 25.9W, 29.8N 24.8W, 37.8N 19.8W. The authoring pass offsets the anchor
eight metres east and searches up to 32 metres for dry, walkable native ground
with separation from existing objects. **These are inferred placements, not
exact retail coordinates.** All nine were accepted; no new Explorer or Overseer
placements are added. Stationary quest objects do not use creature spawners.

`q9` durably records each canonical conversation transition and its XP together.
Wrong-order or repeated touches do not advance a quest, and repeated turn-ins
cannot grant another reward during the cooldown. An existing active Overseer
referral is preserved when another region's search is completed. Ownership,
life, peace stance, visibility and four-metre distance are checked before use;
the distance check now permits nearby interaction across outdoor cell and
landblock boundaries. Preserve this policy version after release so historical
journal replay retains its original meaning.

**Remaining gaps:** the Aunmauri/Obelisk Ward temporary buff rewards are not
implemented. The subsequent Arwic Mines chain is described below.
The journal describes the deadline; a live native countdown/expiry notification
is not yet provided. This pass does not activate other unsupported NPC scripts.

## Arwic Mines lesson v1 (2026-11-01)

The Overseer now starts native quest `400000c1` after completing an active
referral, or directly when approached without one. Its four native journal
phases cover retrieving the Crude Flat Hammer (`470015a9`), returning it,
crafting the Arwic Mines Short Sword (`470015aa`) and returning that sword.
Recipe `6f001f0e` requires 20 Iron (`470010f3`), no forge/rank/cost, and always
produces the sword. Its entitlement is learned at the hammer exchange, saved
separately from general profession grants and checked on every craft request.

The [recovered quest guide](https://www.20oz.net/index.php/Arwic_Mines_and_Crafting_Quest.html)
supplies the 150 XP reward, 20-hour repeat, retained hammer and two reward items:
Arwic Outlands Glyph (`4700109f`) and The Shaper's Guide (`47002917`). Native
recipe bytes, quest text and item quality fingerprints are pinned in
`arwic-mines-v1.json`. All 15 book pages come from installed localization table
`25007a50`; each double-click sends the next page through native MessageDialog,
now decoded into readable bridge chat. Completing this lesson does not implement
the separate Arwic Vault unlocking and shard quests.

Four Outcasts (`470015ac`) and two Edge Beetles (`4700088c`) are added to native
mine rooms in block `7cb9`, with bounded spawner groups. Existing Overseer and
Taskmaster placements are reused. Older captured Outcast DIDs now resolve to
unrelated items, so the new pack explicitly maps recorded room anchors to the
installed creature templates. Native collision validation accepts six anchors
and rejects two; no rejected position is silently relocated. These assignments
and respawn timing are reconstruction policy, not an exact recovered retail
generator table. Outcast combat/AI properties are read from native inherited
templates: level 6, 164 health/vigor, 28 natural damage, armor 4, 19 death XP,
30m perception, 60m home radius and loot profile 948. Absent wandering fields
remain absent. The native-fields authoring tool records every source hash.

Eligible accepted-quest kills in this mine receive one guaranteed hammer in
addition to ordinary inferred treasure; the original drop probability is unknown.
Corpse ownership/range, alive/peace interaction, backpack capacity, learned
recipe and exact ingredient IDs are validated before changes. Durable `m1`
transactions commit the full exchange and replay the frozen v1 policy. The UDP
acceptance test covers combat-to-loot, both targeted item hand-ins, readable book
pages, rejected requests and three server restarts; unit tests cover split stacks,
full bags, duplicate rewards, compaction and the unmodified production spawners.

`author_exploration_quests.py` rebuilds the initial policy from the locally
recovered text, native templates and archived wiki cache; it refuses to overwrite
an existing version. Grounding uses:

```powershell
cargo run --release --manifest-path Cargo.toml --example population_authoring -- "Asheron's Call 2 - Legions" content/exploration-content-v1.json content/exploration-world-v1.tsv content/exploration-world-v1.report.json data/world/legions-world.tsv
python tests/network/prepare_content.py "Asheron's Call 2 - Legions" data/world/legions-world.tsv
python tests/network/test_exploration_quests.py
```

Preparation checks policy/manifest hashes, grounding evidence and installed
entity/visual/cell compatibility. Unit tests cover ordering, expiry, cooldowns,
cross-region rejection and exactly-once XP replay. The real UDP acceptance test
uses the nine prepared placements and existing NPCs with a disposable store;
it completes all four journals, tests range/combat rejection and use across a
landblock boundary, and verifies partial and completed progress after restarts.
It does not establish visual parity in the retail client.

## Regional wilderness pack v1 (2026-10-31)

The previous world followed captured travel routes and individual quest camps;
it did not contain the missing regional wilderness generators. This pass joins
the locally archived [AC2 wiki locations](https://20oz.net/index.php/Locations.html)
and regional rosters (for example [Arwic Outlands](https://20oz.net/index.php/Arwic_Outlands.html),
[Esper Reaches](https://20oz.net/index.php/Esper_Reaches.html),
[Kydi Delta](https://20oz.net/index.php/Kydi_Delta.html) and
[Rings of Oriad](https://20oz.net/index.php/Rings_of_Oriad.html)) to the recovered
Pyreal AC2UtilSQL `ringways` and `towns` rows in `reference-tables.json`.
Those SQL rows contain geography; they are not a recovered spawn database.
Article URLs, HTML hashes, SQL source hashes, native template-cache hash and
the source catalog hash are retained in `regional-content-v1.json` and its
grounding report. This pass adds no guessed NPC dialogue or quest state machines.

The new separate pack admits **3,495 native-grounded creature slots**, using
**326 native creature templates**, in **2,071 spawner groups**. The source roster
matched 329 eligible templates; three had no accepted placement. Sixty-five
regional creature names with no eligible ordinary native profile at the guide's
level were withheld. Protected NPCs, loot-proof/script-only actors and known
single-kill quest targets are excluded from this generic wilderness pool.

Coverage spans:

- Osteth: Arwic Outlands, Esper Reaches, Lost Wish, Mad Crone Mountains, Cobalt
  Sweeps, Drudge Citadel and Prosper Marches.
- Omishan: Kydi Delta, Ariaki River Basin, Lumari Spill Isles, Tou-Tou Peninsula,
  Naderu Greenwood, Farali Wasteland and Vesayen Islands.
- Linvak: Rings of Oriad, Stoneshadow Plains, Thusik Delta, Nepeth Strata,
  Winterhollows, Mount Malthabbor and Gevoth.

Prepared world placements increase from **6,306 to 9,801**. Outdoor landblocks
containing server content increase from **1,991 to 3,833**, including **1,842
newly populated blocks**. The complete world has 8,612 managed creature points,
8,056 outdoors in 3,623 blocks, and 2,214 capacity-limited camp groups. These
counts include the earlier packs; they do not imply complete retail coverage.

### Explicit inference and validation policy

The convex hull of a region's documented ringway anchors bounds its candidate
area; it is not the missing retail region polygon. Overlapping hulls select the
closest documented ringway centre. A stable hash chooses one jittered candidate
camp per 160-metre landblock and one eligible creature from that region's roster.
The source's `group` marker means elite/fellowship difficulty, so the authored
capacity is one elite or two ordinary creatures, not a supposed retail count.
Grounding can shrink a two-child group: 1,424 accepted groups have two children
and 647 have one.

Candidates must stay inside their inferred region, at least 220 metres from
documented town centres, 80 metres from existing outdoor server objects and
other new camp centres, and 12 metres from their own group member. A bounded
32-metre search requires native dry ground, collision contact, a walkable patch
and a clear route between members. The report records 899 rejected attempted
slots; rejecting a camp's first slot withholds its remaining candidates too.
Existing captures, services, quest camps and all DAT scenery/foliage stay intact.
No expansion is inferred for dungeons, Arramora, Knorr or missing regional edges.

The normal server uses native health, level, vigor, natural damage, armor,
perception, wandering and homesickness for these actors. Existing generic
treasure rolls and native trophy probabilities apply, so they can be killed and
looted through the usual ownership/persistence path. This does not recover full
retail loot affixes, creature spell AI or exact spawn weights. Child regeneration
is an explicit **180-second** policy; the controller owns creation, corpse
retirement and replacement rather than inserting permanent static mobs on login.
Idle distant regions perform no collision or wandering work until players approach.

### Regeneration and acceptance

From the repository root (using the configured local Python/Rust tools):

```powershell
python tools/author_regional_population.py
python tools/import_creature_combat.py "Asheron's Call 2 - Legions/portal.dat" content/regional-creature-entities.txt content/regional-creature-combat-v1.tsv
cargo run --release --manifest-path Cargo.toml --example regional_population -- "Asheron's Call 2 - Legions" content/regional-content-v1.json content/regional-world-v1.tsv content/regional-world-v1.report.json data/world/legions-world.tsv
python tests/network/prepare_content.py "Asheron's Call 2 - Legions" data/world/legions-world.tsv
./Build-Pyrealium.ps1
$env:AC2_TEST_GAME_DATA = "${PWD}/Asheron's Call 2 - Legions"
cargo test --release --manifest-path Cargo.toml --test regional_population
python tests/network/test_regional_population.py
```

The grounder ignores only this pack's exact reserved identity range when run
against an already expanded world. Preparation refuses stale catalog/placement
proof or mismatched native visuals. Native acceptance checks all 3,495 positions,
regional rosters, town/content separation, combat health and varied loot for
every placed template, plus group capacity, one-time initial creation and delayed
replacement. Two real UDP clients tested visibility, kills and loot pickup in
Esper (Drudge Outcast, 38.6N 22.6W), Kydi (Akeha Mimbu, 10.3N 40.6E) and Oriad
(Undead Stony Armoredillo, 15.0S 4.7W) with the complete 9,801-placement world.
Only those three test templates had reduced health/damage for the bounded UDP
test; separate native acceptance verifies the normal stats. These are protocol
tests, not a rendered-client visual inspection of all new locations.

## Wilderness and NPC hunting expansion (2026-11-01)

This follow-up uses the archived [Knorr](https://www.20oz.net/index.php/Knorr.html)
portal map, [Arramora](https://www.20oz.net/index.php/Arramora.html) map and habitat
notes, and the original 21 regional wiki lists. The Foundry's split-name/level
tables needed a different roster parser. The old Pyreal Arramora spawn class
was also checked: it supplies a few named services/landmarks, not the missing
general monster generator database. Archive paths and SHA-256 values are retained
in `wilderness-content-v1.json`; source SQL, captures and DATs are unchanged.

The new pack contributes **2,553 creatures, 1,555 managed groups and 1,167 newly
populated outdoor blocks**. It uses 443 native template identities, 180 absent
from the prior world. There are 266 new creatures in Knorr Wilds, 217 in Foundry,
308 in Gardens, 433 in Ramparts, 349 in Desolation and 296 in Arramora; the
remaining 684 fill original-continent gaps. The prepared world totals 12,379
objects, including the two new hunt speakers. Every one of the 563 eligible
native types matched from these 27 wiki rosters has a placement somewhere in
the combined world. Existing captured and authored encounters retain priority.

The `wilderness-coverage-v1.json` audit checks all **2,024** native records with
positive health and damage: 800 eligible hostile templates have placements,
172 are alternative templates of a placed name, 192 are protected/non-hostile,
11 need special encounter lifecycles, and 849 still need habitat or encounter
evidence. These are template counts, not distinct species counts. The audit also
retains withheld regional rows and a wiki quest-starter inventory for subsequent
implementation; a placed NPC does not imply that all its quests work.

### Placement and loot policy

Knorr envelopes are calibrated against four labelled portal coordinates in the
archived map. Arramora uses its documented mainland camps, excluding the kingdom
islands. Family-specific habitat constraints follow guide prose; specific hunt
descriptions override broader family guidance. Exact region polygons, density,
group counts and three-minute regeneration remain reconstruction choices.
The grounder enforces native dry ground, contact, walkability, a clear path
between group members, town exclusion and 80-metre separation from existing
content/new camp centres. Burun Keeriks required additional candidate attempts;
their eventual placements passed the same checks without lowering tolerances.

A quest death effect alone no longer excludes an ordinary monster: Drudge
Robbers and other common hunt targets can inhabit the wilderness. Named/rare
encounters, service NPCs, loot-proof actors and special structures remain
withheld. The Olthoi Soldier Grub's static mound AI has a native wandering value
outside the supported probability range; its missing lifecycle is documented,
not silently converted into ordinary roaming AI.

New actors use native health, vigor, damage, level, armor, motion and AI catalogs.
Loot uses the existing observed pools, recovered generic quantity limits, native
trophy probabilities and variable supported equipment rolls. Equipment stats
use the creature level and survive pickup. This does **not** recover exact
retail loot weights, rare affixes, spell AI or all dungeon populations. Every
admitted template is tested for supported, variable loot across multiple rolls,
including level-zero native records whose generated item level is clamped to one.

### Ten NPC hunting quests

- Vreema: Fracture the Bone Golems → Fracture the Rime Golems → Fracture the
  Uncharneled Golems. Level 55, ten targets per stage, 4% level XP capped at
  59,676,035 / 84,627,779 / 119,705,147 respectively.
- Gregor Adulen: Exterminate the Olthoi Harvesters, level 50, 3% capped at
  10,787,851. Nola Vontag: Rout the Deranged Lashers, level 50, 3% capped at
  22,085,504. Tankaru: Vaporize the Frost Wisps, level 55, 2.5% capped at 88,254,343.
- Brelar Marlage: Crush the Cursed Bones, level 48, 4% capped at 5,829,950.
  Illiana Bain: Crush the Skeleton Lieutenants, level 50, 4% capped at 10,014,692.
  Shurakhe: Destroy the Ravenous Lashers, level 50, 3% capped at 15,455,108.
  Endaren Brightstar (native spelling **Endaraen**): Smash the Coral Golems,
  level 55, 3% capped at 44,757,025.

All require ten qualifying kills within seven days and returning to the correct
NPC. Arramora repeats after six days eighteen hours; the four Foundry hunts
repeat after seven days. Percentage rewards use Pyreal's inclusive level XP
bands, capped by the wiki reward. Level 150's two-XP terminal sentinel is not a
real advancement band: these already-capped hunts retain their capped payout.
Kills before acceptance grant no hunt progress; duplicate conversations cannot
pay twice. Expired objectives must restart. Prerequisites, partial progress,
completion and awarded XP persist atomically across restarts.

Six speakers already have placements. Shurakhe and Endaraen are inferred near
Jethric's Outpost (32.0S, 52.0E), on validated native ground. Their original NPC
tables and target death-effect names corroborate the quest bindings. The later
NPC quest IDs are missing from the recovered admin export; similarly named
auto-bestowed raids are separate quests. Authored IDs `4000fc01`–`4000fc0a` and
summarized dialogue make that distinction explicit. The versioned `qh1` and
`qk1` store events preserve old `q6` replay behavior. Released hunt policy must
not be edited in place without a new version and migration.

### Regeneration and verification

From the repository root, after recovering the wiki archive and native cache:

```powershell
python tools/author_wilderness_expansion.py
python tools/import_creature_combat.py "Asheron's Call 2 - Legions/portal.dat" content/wilderness-creature-entities.txt content/wilderness-creature-combat-v1.tsv
cargo run --release --manifest-path Cargo.toml --example regional_population -- "Asheron's Call 2 - Legions" content/wilderness-content-v1.json content/wilderness-world-v1.tsv content/wilderness-world-v1.report.json data/world/legions-world.tsv
python tools/author_hunting_quests.py
cargo run --release --manifest-path Cargo.toml --example population_authoring -- "Asheron's Call 2 - Legions" content/hunting-content-v1.json content/hunting-world-v1.tsv content/hunting-world-v1.report.json data/world/legions-world.tsv
python tests/network/prepare_content.py "Asheron's Call 2 - Legions" data/world/legions-world.tsv
python tools/audit_wilderness_coverage.py
$env:AC2_TEST_GAME_DATA = "${PWD}/Asheron's Call 2 - Legions"
./Build-AC2Network.ps1 -Test
python tools/test_wilderness_authoring.py
python tests/network/test_wilderness_population.py
python tests/network/test_hunting_quests.py
```

The native-DAT tests check every accepted position, actual combat/AI/loot data,
all hunt speakers and targets, capacity-controlled creation and delayed refill.
Two real UDP clients verify visibility, death replication and loot collection
in seven sample regions, including all five Knorr areas and Arramora. The hunt
UDP test uses production NPC positions and ten weakened copies of each target
at native ground positions, completing all ten quests and restarting during a
partial hunt and after completion. It checks level refusal, capped return XP
and duplicate-turn-in protection. Test stores are disposable. These checks
verify gameplay/protocol behavior, not rendered retail visual parity.

## Hunting quest expansion v2 (2026-11-01)

The second immutable hunt catalog adds 25 executable quest flows, bringing the
combined catalogs to 35 hunts at 23 native NPC identities. Existing capture
placements are retained. This fills NPC interaction gaps rather than replacing
ordinary monsters with quest-only fixtures.

- **Ikeras (13):** Karin's Stray Darkenfowl hunt; Sumalar's Servitor, Puppet and
  Surrogate Spawn chain; Pim's Kachem Mimbu hunt; Baliga's Parfal Nefane and
  Listris Niffis chain; Sister Nell's Ulu and Faisi Sclavus chain; Baltor's Marsh
  Armoredillos; Laurikhe's Opor Niffis and Nefane chain; Ingara's Blighted Mucor.
  Minimum levels range from 17 to 29; fixed rewards range from 3,000 to 82,000 XP.
  Repeat time is 20 hours. The last eight quests have no deadline, as documented;
  the first five retain seven-day deadlines. A completed prerequisite is still
  required even when the next quest's listed level is lower.
- **Linvak Tukal / Arramora (9):** Groli's Anguish Crone, Thornling Plunderer and
  Shock Reaper chain; Gregor's Worker Nymph and Drone follow-ups; Audalon's
  Vigilant Spawn; Eliza's Lumna and Veinu Niffis; Red Bellows' Dark Dread Mu-miyah.
  These retain seven-day deadlines, six-day/eighteen-hour repeat timers and
  guide-listed percentage XP/caps. Gregor's new stages require completing his
  existing Harvester hunt.
- **Jethric's Outpost (3):** Moira's Skeleton Lancers (level 55, 2.5%, cap
  52,892,362); Urlin's Ruthless Waraths (level 50, 3%, cap 31,480,235); Furakhe's
  Mu-miyah Warriors (level 50, 3%, cap 10,787,851). Deadline and repeat are seven
  days, with ten kills and an NPC return required.

Every profile preserves the archived wiki article hash, facts and source URL,
the native speaker's NPC-table name, and the **specific** counter death effect
used to select qualifying templates. This includes Seeker Faisi Sclavus and
other native qualifying variants while excluding unrelated Gauntlet clones.
Native `Dread Dark Mu-miyah` corresponds to the guide's reversed-wording title;
Lauriche in one guide page is the native Laurikhe NPC. Full reward/identity
details are in `hunting-content-v2.json`; dialogue is an authored summary.

Examples of the guide rules used: [Servitor Spawn](https://www.20oz.net/index.php/Perforate_the_Servitor_Spawn.html),
[Skeleton Lancers](https://www.20oz.net/index.php/Crush_the_Skeleton_Lancers.html),
and [Faisi Sclavus](https://www.20oz.net/index.php/Eliminate_the_Faisi_Sclavus.html).

Two absent speakers are now placed on validated ground: Furakhe near
32.08S 52.10E and Ingara near 10.20N 62.40E. Their offsets within the documented
towns are inferred. Three Arramora camp anchors come from the quest guides:
Vigilant Spawn at 17.2N 52.2W, Olthoi Worker Nymphs at 24.6N 51.9W, and Dread
Dark Mu-miyahs at 16.7N 47.7W. Ground search is bounded to 32m and rejects
water, collision, steep terrain and positions within 6m of existing actors.
Each camp owns two creatures with 120-second regeneration. These sizes/timers
are conservative reconstruction policy, not recovered retail generator values.
NPCs are excluded from combat; the six creatures use native stats, AI and the
existing varied equipment/trophy loot pipeline. No global loot weights were
invented in this pass. Prepared world size is now **12,387 objects**.

IDs `4000fc0b`–`4000fc23` are authored and stable. New durable events `qh2` and
`qk2` make the expanded catalog available. `qh1` and `qk1` replay against only
the original ten quests; otherwise an old Gregor repeat turn-in could silently
start Worker Nymphs and change saved progress. Fixed XP uses zero basis points;
zero time limit means unlimited, never immediately expired. Catalogs must not
be changed after deployment without a new replay policy.

The expanded UDP run also exposed reliable-queue exhaustion: every kill and
NPC interaction resent unchanged completed quests. Journal updates now contain
only entries whose state changed, across hunting, exploration, recovered and
starter quests. Login still sends the full snapshot. A 35-entry regression test
checks that a single kill emits one quest update rather than 35; the extended
network run exercises the completed-history case.

Verification completed with 271 Rust tests passing (four optional tests ignored),
15 content-preparation tests, and all three population tests explicitly run
against the installed DATs. The real UDP run completed all 35 hunts and retained
them after restart; separate exploration, Arwic Mines and starter-gameplay runs
also passed. Exploration verifies that its existing referral stays in the
journal without being retransmitted on subsequent quest returns.

Regenerate and verify:

```powershell
python tools/author_hunting_expansion.py
cargo run --release --manifest-path Cargo.toml --example population_authoring -- "Asheron's Call 2 - Legions" content/hunting-content-v2.json content/hunting-world-v2.tsv content/hunting-world-v2.report.json data/world/legions-world.tsv
python tests/network/prepare_content.py "Asheron's Call 2 - Legions" data/world/legions-world.tsv
python tools/audit_wilderness_coverage.py
.\Build-AC2Network.ps1 -Test
$env:AC2_TEST_GAME_DATA = (Resolve-Path "Asheron's Call 2 - Legions").Path
cargo test --release --manifest-path Cargo.toml --test wilderness_population
python -m unittest discover -s Network -p test_prepare_content.py
python tests/network/test_hunting_quests.py
```

The native acceptance test checks dry reachable ground, existing-actor
separation, NPC protection, loot generation, two-child group capacity and
delayed refill. Quest tests cover exact reward/timer boundaries, no-deadline
quests, qualifying variants, prerequisites and mixed old/new journal replay
through compaction. UDP acceptance completes all 35 quests using production
NPC positions and disposable weakened target copies, with restart checks for
partial and completed progress. Rendered UI parity and fellowship-shared kill
credit remain outside this verification.

## Quests and flow evidence

`content/native-quests.json` contains 582 unique quest definitions,
449 readable titles, 1,650 readable phase-journal entries, 2,575 quest-effect
definitions and 2,806 entity records with NPC-table or death-effect links. Of
those death-effect references, 581 resolve to a native quest and status word. Native
IDs, localization keys, phase flags, related-quest lists, effect status words and
unresolved references are retained. It is an authoring catalog, not a runtime
switch that enables all quests.

Examples recovered directly from the data:

- `QuestKillGreenWaspStriker` (`40000097`, **Incursion**) has ordered journal
  entries for five through one remaining Green Phyntos Wasps. Creature templates
  `47000f9c` and `47000f9d` name its increment effect in `NPC_DeathEffect1`.
  `47001623` names its bestow effect with a different native stat value. Those
  values are preserved, not silently interpreted as identical kill-credit rules.
- `QuestArwicBoss` (`40000032`) says to slay the Drudge Keeper in the Arwic Vault;
  `QuestArwicShard` (`40000033`) says to use the Arwic Shard. Retained setup
  bytecode describes both as hidden quests controlling the Outland quest flow.
- `QuestBrothOfTanacha` retains a phase list of five related quest IDs. These are
  references to other quests, not callable script functions.
- Orven resolves to `NPCTable_Orven`. This is a table name, not its dialogue,
  prerequisite and reward rows. Those rows were not recovered in this pass.
- Old event packages such as `QuestMucorMushrooms` retain setup bytecode but have
  empty localized text entries. Missing text is reported rather than replaced
  with a package name or new prose.

Function-name exports and import symbols are stripped. Virtual tables still
point to real bytecode, including setup constants and text references. Other
methods are empty frame/return stubs or trap instructions. The exporter records
these distinctions; an empty exports list must never be reported as proof that
the entire script is missing. Conversely, a retained constructor does not prove
that its server-side eligibility, advancement or reward implementation exists.

To activate a recovered quest, still verify its bestow trigger and prerequisites,
phase transitions, fellowship/kill-credit policy, completion/repeat rules and
rewards. Bind those to authenticated server actions, persist the state changes
and test replay/duplicate events before exposing it as a playable quest. Native
text and effect names alone are insufficient. Existing Pyrealium-authored tutorial
quests remain distinct from recovered retail quests.

## Population evidence

The installed-DAT generator audit found 639 templates with generator properties
and 2,111 profile labels. Across 20,784 outdoor and 151,868 interior placements,
none referenced a generator profile; the EncounterDesc table was empty. Five
unsupported template parses remain recorded in the audit. The `Generator`
bytecode exposes property/editor setup but has not yielded the missing child
rosters, region records or placed generator database.

Accordingly this pass adds no speculative spawn coordinates or camp sizes.
Existing compatible captured monsters continue through server-owned point
spawners; verified services remain fixed objects. Matching profile names is not
enough to reconstruct retail placement. The supplied SQL's entity and quest
tables were also empty, as documented in `NATIVE_GAMEPLAY_PASS.md`.

## Reproduction and validation

From the repository root, after building the reference probe:

```powershell
# The retail probe is a tool of the AC2_Rusted client project; install it onto PATH:
#   cargo install --path tools/ac2-retail-probe   (in an AC2_Rusted checkout)
python tools/import_native_quests.py "Asheron's Call 2 - Legions" --output content/native-quests.json
python tools/import_native_trophies.py "Asheron's Call 2 - Legions" --output content/native-trophies.json
```

The quest catalog fingerprints WLib and MasterProperty; each trophy rule
fingerprints the actual inherited EntityDesc records used. These are separate
from generated `Saved/` research listings.

Server/bridge and Unreal Editor/game builds passed. Installed-DAT tests validate
sign classification, native text resolution, trophy compatibility, a 5% trophy
distribution, loot-proof suppression and stable corpse rolls. Real UDP tests
validate sign reading and range limits, self/NPC/item examinations, doors,
vendors, corpse naming, single pickup/Loot All, ownership, replay and inventory
persistence. These checks do not establish complete retail quest or loot parity.

All 52 Unreal data tests passed. A rendered client/server journal test also
passed skill training, advancement, untraining/cancellation, quest tabs and
window dragging. That test exposed two input defects fixed in this pass: saved
off-screen window offsets delayed visible dragging, and underlying health-bar
controls intercepted clicks on an overlapping floating panel's title bar.

## Second corpus pass: Pyreal SQL, cached articles, and missing private strings

The second pass audited 70 SQL files, deduplicated into 26 distinct byte payloads,
across the extracted server projects, packet-corpus source snapshots and supplied
2017 SQL. A fresh copy of **5 AC 2 Pyreal SQL Database.7z** from the supplied MEGA
folder contains the same `pyreal.sql`: SHA-256
`e70658d4856784ae8a032c81cac2664c014caf009c1cadbbe6387ba5c2e2dd0d`.
Its `entity`, `quest`, `questphase`, `skill` and `submission` tables remain empty.
The version 18–22 database drafts contain schemas, not the missing population.
The file `ikeras-shops.sql` contains ten **portal** rows, not vendor inventories.

There are useful tables elsewhere in Pyreal. Three byte-identical copies of the
old **AC2UtilSQL** guide database contain:

- `npc`: 184 NPC names and coarse map coordinates.
- `incursions`: 67 starter-monster rows with up to five guide locations each.
- `quests`: 162 quest names, guide levels and detail/guide identifiers.
- `overview`: 282 summary rows, including suggested/minimum levels and rewards.
- `bestiary`: 791 creature reference rows with level, health and XP fields.
- `dungeons`: 65 entries; also outposts, vaults, portals, resources and towns.

These previously escaped the narrow SQL audit because it only admitted emulator
table names. The guide DDL also uses unquoted table names, single-line schemas,
and column identifiers with leading spaces. The reader now supports those
formats through an explicit public-content allowlist. It never executes SQL or
exports account/character rows. The corpus catalog retains source hashes,
duplicate paths and original row values. All 70 SQL files parsed without errors.
Zero-coordinate placeholders are excluded from location hints, not spawned at
the world origin. Repeated tables in separate dumps are not independent evidence.

### Previously hidden article content

The archived wiki's ordinary extracted files are incomplete. Its internal
`AC2/hts-cache/new.zip` contains the actual article responses under URL-shaped
member names. Reading that ZIP directly recovered **2,075 main articles and 902
HTML tables**; 397 articles include a Quest Starter field. No HTML is executed
and URL-shaped members are never extracted as filesystem paths. File/talk/user
namespaces, revision URLs and missing-page responses are excluded.

`reference-tables.json` records 597 guide-row/article candidate links, 968 guide
rows with usable coarse location hints, and 83 native quests with title-matched
article candidates. Names are search aids, not proof that two versions have the
same template identity. The full article text, links and tables remain in the
generated `Saved/ContentRecovery/wiki-content.json`; each article has its raw
HTML SHA-256 and original URL. The compact catalog retains selected loot and
dungeon tables plus all 46 incursion-index rows.

Useful examples from the cache:

- **Gambling Debt** explains Orven's role: Kanten randomly assigns a quest and
  gives the player Kanten's Bag; the player hands it to Orven to finish. This is
  an item hand-in with quest state, not a universal click-to-reward action.
  The page lists level 11+, a two-hour limit/repeat and 1,000 XP. These are guide
  claims requiring build validation, not newly installed server rules.
- **Broth of Tanacha** identifies five monster/ingredient pairs and the order:
  gather the ingredients, place them in the cauldron, then use it. That agrees
  structurally with the native five related quests and final use-cauldron phase.
- **Incursion** has 46 starter/target/reward/region rows. Its target amounts mean
  kills required by the quest, not generator capacity. The Green Phyntos Wasp
  article separately describes a five-wasp location south of Arwic; this is a
  guide observation, not a recovered generator roster or terrain transform.
- **Monster Statistics** explicitly describes its Drops column as how many items
  a creature is *likely* to drop, and labels its source Hero 1.0. It provides
  useful comparisons and unique-trophy lists, not item-selection probabilities.
- **Loot Jewelry** has effect ranges. **Loot Weapons** describes random modifiers,
  exceptional quality and cosmetic variation, while explicitly saying several
  prefix effects are untested. No general affix/stat-roll formula was recovered.
- **Tyrant Armor** includes Tyrant's Loft room/corridor occupants, companion
  counts, boss levels and scale quantities. Its trophy percentage table is
  explicitly a sample of 30 or 50 kills per creature, not native probabilities.
  The room roster can guide a future dungeon pass once matched to actual cells.

Version disagreements are retained. Eight incursion index/detail pairs disagree
on repeat time; the tool reports both values and hashes. Green Phyntos Wasp is
five minutes on its detail page but twenty hours in the index. Broth of Tanacha
has 400 XP in the old SQL overview and 850 on the cached page. Two Miller of
Kehan pages disagree on XP, repeat time and the Miller's location. Orven's own
article gives 16.2N 28.4E, while Gambling Debt places the hand-in near 44.9N 7.1W
and the old SQL says 44.9N 7.0W. None of these conflicts is resolved by silently
preferring the last imported source.

### Native text recovery

Pyreal's `StringTables/English/LocalEnglish.rar` contains loose native localization
records, including private StringTables absent from the installed DAT. The new
`recover_quest_text.py` uses the pinned packed decoder to follow the same key
through private-table redirects. Recovery requires an identical installed parent
record; any installed intermediate record must also match byte for byte. Missing
keys, cycles, variables requiring formatting, empty text and version mismatches
remain unresolved. Existing readable fields are never replaced.

This recovered **75 names, 18 summaries and 198 journal phases**, across **98
quests**. Examples include Broth of Tanacha, Arwic/Esper/Lost Wish obelisks, Rescue
the Shelter Hold, Arwic Mines and the Miller introduction. The 203 remaining
MissingPrivateTable fields comprise 178 absent archived records and 25 empty
texts. Other error categories, such as uninitialized fields, are not rewritten.
The generated `recovered-quest-text.json` preserves raw text/grammar markers,
quest/field identity and all record hashes. A matching parent does not prove the
missing private record came from exactly the same build, so this remains
authoring evidence rather than an automatic localization overlay.

### Sources that do not fill the gap

The old `spawn list.xls` is a lookup of entity/name/visual and related IDs, not
placed generator coordinates. The newly checked **1 Utilities.zip** contains
AC1 tools; its CreatureMapper `spawn.xml` and Dereth Explorer `quests.xml` are
for AC1 and are excluded. The password-protected `AC2 Users.rar` remains unread.
The source tree's test quest-advance packets and `IQuest` interfaces do not
supply NPC dialogue/eligibility/reward rows.

No complete executable NPC table, exact placed generator database, or general
loot-weight/stat-generation table was found in this pass. The new sources do
provide substantially more quest text, flow references, coarse locations,
dungeon rosters and loot comparisons than the admin client alone. Activation
still needs source/build reconciliation, exact entity/cell bindings, durable
quest progress, authenticated item hand-ins and replay-safe rewards.

### Native potion crafting connection

The existing health/vigor recipes `6f001f17` and `6f001f18` now reach the Unreal
Craft tab through native registry, ingredient-validation and execution messages.
The ordinary Unreal server profile grants this bounded pair; it no longer
requires the development training kit. The training mentor also continues into
the existing authored Field Supplies and Potion Practice lessons, including
atomic ingredient consumption, quest credit and replay-safe rewards.

The installed Legions recipe records are both 1,448 bytes. Their FNV-1a values
are `4bf38afdcbdee5dd` and `2737e155650100af`; the retained older records are
`fe785ca8fcbe9b88` and `93c3bd24e230ac1a`. Each differs only at word `0x12c`
(interpreted package `01d7`, field 36: zero rather than one). Both complete
records are pinned. The skill DID, output DID, ingredient identities and
quantities are independently checked: one Mucor Mushroom and 100 Tallow.
The meaning of field 36 is not inferred from this byte difference.

Recipe grants, stock prices and cooldowns remain Pyrealium policy. The subsequent
profession implementation adds 26 saved skill records, 357 supported DAT-backed recipes,
advancement, refining, dyeing, surveying and gathering. Its native sources,
reconstructed formulas, workshop evidence and remaining spellbinding/discovery
gaps are documented in [profession coverage](../content/README.md#profession-crafting-and-gathering).
The Arwic Mines sword recipe `6f001f0e` now has a separate learned entitlement
and durable quest exchange; it is not part of the default recipe grant.

### Reproducing this pass

Run from the repository root; the HTML tool requires `lxml`. Extract only the
`0x25` folder from the historical English RAR to the indicated scratch folder
before the first command. Archive extraction does not modify the retail DATs.

```powershell
python tools/recover_quest_text.py "Asheron's Call 2 - Legions" --loose-strings Saved/ContentRecovery/old-local/0x25 --source-archive Saved/ServerReference/extracted/199ae71fc5ca72c6/PyrealSourceCode/StringTables/English/LocalEnglish.rar --output content/recovered-quest-text.json
python tools/recover_reference_tables.py --sql-root Saved/ServerReference/extracted --sql-root Saved/PacketCorpus/extracted --sql-root Saved/PyrealSQL-2017 --wiki-cache Saved/ServerReference/extracted/595afb35dbea396a/AC2/hts-cache/new.zip --wiki-output Saved/ContentRecovery/wiki-content.json --native-quests content/native-quests.json --recovered-quest-text content/recovered-quest-text.json --output content/reference-tables.json
```

Validation covers SQL formats/deduplication, exclusion of private tables,
coordinate placeholders, inert HTML/cache reading, conflicting timers, generic
quest-title ambiguity, exact-parent text recovery, missing keys, variable text,
cycles and preservation of existing translations. Installed-DAT quest vectors
also pass. This pass changes recovery tooling and authoring catalogs only; it
does not change live NPC behavior, spawn placement or loot probabilities.

### Wiki NPC implementation pass — 2026-11-01

The subsequent implementation cross-checks the [live NPC category](https://20oz.net/index.php/Category%253ANPC.html)
with the retained 2,075-article wiki cache, town NPC sections, quest starters and
native actor identities. The category has 89 entries; 74 resolve to installed
native actors. Including town/quest listings yields 225 names, 210 resolved
names and 217 native variants. Fifteen unresolved names remain explicit:
Cedar; Darvain, Karu, Kiraly and Tyke Geddon; Jellio; Jorwea; Jrent; Keranua;
Orulaan; Polauri; Shi Lezawa; Subchieftain Gruu; Trader Stix; and Wiggles.
They are not given unrelated substitute models.

The server now provides 326 authored pages covering 229 quest topics through
native named speech and MessageDialog. Replies explain sourced requirements,
starting areas, timers and listed rewards, with selected paraphrased walkthrough
summaries. Actual retail quotations have not been recovered for this catalog.
Information is explicitly labeled as such and cannot accept or advance a quest,
consume an item, teleport a player or pay a reward. Working quest/shop handlers
keep priority on ordinary use; `@talk` provides informational browsing on those
same actors. This fills the generic no-action response for supported NPCs, not
every missing retail quest flow.

Twenty-seven missing actors now have inferred outdoor placements, including
Old Herrigar, Sachem Mimbu, Dark Agent, Brisin, Maronak and Greenstone quest NPCs.
Each passes native dry-ground, collision, walking-patch and six-metre separation
checks within 32 metres of its coarse wiki coordinate. Existing native identities
are retained globally when proposing new positions. The hostile variants of
Zroth and Rashan are excluded from friendly dialogue. The wiki supports direct
visits to Feral Pet Lasher, Rashan's friendly spectre and both Gurog Civil War
contacts; these are not assumed to require an event spawn. The High Shaman's
coordinate fails native ground validation, so only the other three are added.
Ambiguous native variants remain unplaced. Both Civil War contacts receive the
quest topic, and the Lasher receives its documented Nature's Ally information.

Another source conflict was found: Lataua's page lists the same Cavendo coordinate
as Kanten, despite both the Zu town list and [The Path into Light](https://20oz.net/index.php/The_Path_into_Light.html)
placing her in Zu. Her proposed coordinate is withheld rather than silently
moving her to Cavendo. Discontinued S.H.R.E.T.H. information is labeled separately
from its Base Ultra replacement, and identical mirror aliases are deduplicated.

Evidence and remaining gaps are in
[`npc-dialogue-v1.audit.json`](../content/npc-dialogue-v1.audit.json).
The [content documentation](../content/README.md#wiki-npc-conversations-and-placements)
describes regeneration and validation. Full quest flows, service-specific effects,
missing native identities and unresolved interior/event placements remain work
to be implemented; conversation coverage alone does not establish retail parity.
