# Build 7230 skill data

These tables are selected by the bootstrap's verified 7230 script fingerprint,
before character-store replay initializes skill catalogs. Legions and converted
7148 profiles retain their existing tables.

The active-effect and travel tables were generated from the unmodified archived
7230 `portal.dat` using `tools/import_skill_effects.py --profile 7230` and this directory's skill
catalog. `skill-effects.report.json` pins every successfully decoded skill and
effect record by SHA-256. The runtime validates those records against the supplied
archive at startup. Unresolved records remain listed in the report; selecting
7230 does not make unsupported effect types executable.

`tests/profile_7230.rs` exercises training and casting Flame Bolt in a separate
process. Set `AC2_TEST_7230_PORTAL` to the native archive path to additionally
validate the eligibility, advancement and effect tables against the DAT.
