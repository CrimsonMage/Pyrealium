-- Pyrealium schema 2. Databases created before schema 1 was rewritten kept their tables in three
-- schemas (auth, shard, world); move them into the default schema beside every other table and
-- drop the empty schemas. A database created from the current schema 1 has nothing to move:
-- every statement here skips a table or schema that does not exist.
ALTER TABLE IF EXISTS auth.accounts       SET SCHEMA public;
ALTER TABLE IF EXISTS auth.login_tickets  SET SCHEMA public;
ALTER TABLE IF EXISTS shard.characters    SET SCHEMA public;
ALTER TABLE IF EXISTS shard.items         SET SCHEMA public;
ALTER TABLE IF EXISTS shard.ground_items  SET SCHEMA public;
ALTER TABLE IF EXISTS shard.world_state   SET SCHEMA public;
ALTER TABLE IF EXISTS shard.gun_users     SET SCHEMA public;
ALTER TABLE IF EXISTS world.prep_meta     SET SCHEMA public;
ALTER TABLE IF EXISTS world.files         SET SCHEMA public;
ALTER TABLE IF EXISTS public.prep_meta    RENAME TO content_meta;
ALTER TABLE IF EXISTS public.files        RENAME TO content_files;
ALTER INDEX IF EXISTS public.prep_meta_pkey RENAME TO content_meta_pkey;
ALTER INDEX IF EXISTS public.files_pkey     RENAME TO content_files_pkey;
DROP SCHEMA IF EXISTS auth;
DROP SCHEMA IF EXISTS shard;
DROP SCHEMA IF EXISTS world;
