"""Extract the skills portal.dat entity records give creatures (NPC_StartingSkill1..20 with their
levels), beside the attack and defense ratings `import_creature_combat.py` reads.

The server uses the ones whose skill record (`skill-effects.tsv`) puts a harmful timed effect on
a player. Evidence: the same entity chunks `import_creature_combat.py` parses
(Entity.LoadFromPortalDat / ParseEntityChunk, parents merged).

    python tools/import_creature_skills.py PORTAL_DAT OUT.tsv ENTITY_LIST [ENTITY_LIST...]
"""
from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from ac2_dat import Dat
from import_creature_combat import load_index, read_entity

# SkillList: magic/melee/missile attack and defense, which the combat table keeps.
RATINGS = {0x1, 0x2, 0x7, 0x8, 0xC, 0xD}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("portal", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("lists", type=Path, nargs="+")
    args = parser.parse_args()
    wanted = set()
    for path in args.lists:
        for line in path.read_text(encoding="utf-8").splitlines():
            line = line.strip()
            if line and not line.startswith("#") and not line.startswith("pyrealium-"):
                wanted.add(int(line, 16))
    rows = []
    with Dat(args.portal) as archive:
        archive.index()
        types = load_index(archive)
        for entity in sorted(wanted):
            try:
                values = read_entity(archive, types, entity)
            except Exception:
                continue
            skills = values.get("skills", {})
            levels = values.get("skill_levels", {})
            own = sorted(
                {(skill, levels.get(slot, 0)) for slot, skill in skills.items() if skill not in RATINGS}
            )
            if own:
                rows.append((entity, own))
    lines = [
        "pyrealium-creature-skills-v1",
        "# Skills (other than the attack and defense ratings) of creature entities from portal.dat",
        "# entity records (NPC_StartingSkill1..20 and their levels), parents merged.",
        "# entity<TAB>skill:level,skill:level,... (hex skill, decimal level)",
    ]
    for entity, own in rows:
        lines.append(f"{entity:08x}\t" + ",".join(f"{skill:x}:{level}" for skill, level in own))
    args.output.write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(f"creatures {len(wanted)} rows {len(rows)}")


if __name__ == "__main__":
    main()
