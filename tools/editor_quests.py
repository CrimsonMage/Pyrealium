"""The world's quests: part of its content database (`editor_content`). Kept so older imports work;
the quest names map onto the content database's."""
from editor_content import (  # noqa: F401 - re-exported
    LIMIT, content_path as quests_path, content_db_path as quests_db_path, content_text as quest_text,
    dump_database, ensure_database as build_database, mtime, open_in_tool, read_dump, save_dump, signature)
