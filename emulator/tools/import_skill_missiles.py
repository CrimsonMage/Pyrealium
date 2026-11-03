"""List the attack hooks of each active skill that fire a missile.

A skill record's root field 65 lists its attack hooks in order (the story's
`StoryHookData` attack hook number is the index). A hook whose seventh value is
an EntityDesc DID (0x47......) names the missile it fires: the client's player
script returns that entity from `gmCWInterface::GetMissileInformationFromWSL`
for the same skill and hook number (Flame Bolt 0x59 hook 1:
fx_proj_energybolt_base_fire 0x47000d04; the human bow attack 0xc7 hook 0: the
arrow 0x470000be), and `StoryManager::FireMissile` makes it on the client when a
story hook of type 3 names that hook. Starter magic attacks (0x60, 0x111,
0x2a0) have no such hook.

Usage (from emulator): tools/import_skill_missiles.py --game-data <DAT-dir>
--catalog content/skills.tsv --output content/skill-missiles.tsv (and the
content/7230/ pair with the 7230 DATs).
"""
import argparse
import hashlib
from pathlib import Path

from ac2_dat import Dat
from import_skill_effects import NODE, ROOT, objects, struct_fields, list_items

ANIMATION_HOOKS = 65
ENTITY = 6
ENTITY_TYPE = 0x47


def missile_hooks(data):
    """(hook index, entity DID) for each attack hook of the record that names a missile."""
    values, graph = objects(data)
    if not graph or graph[0][0] != ROOT:
        raise ValueError("no root object")
    fields = struct_fields(values, graph[0][1])
    if len(fields) <= ANIMATION_HOOKS:
        return []
    hooks = fields[ANIMATION_HOOKS]
    if hooks >= len(graph) or graph[hooks][0] != NODE:
        return []
    found = []
    for index, item in enumerate(list_items(values, graph[hooks][1])):
        if item >= len(graph) or graph[item][0] != NODE:
            continue
        hook = list_items(values, graph[item][1])
        if len(hook) > ENTITY and hook[ENTITY] >> 24 == ENTITY_TYPE:
            found.append((index, hook[ENTITY]))
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--game-data", type=Path, required=True)
    parser.add_argument("--catalog", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    rows = [line.split("\t") for line in args.catalog.read_text(encoding="utf-8").splitlines()
            if line and not line.startswith(("#", "pyrealium"))]
    out = [
        "pyrealium-skill-missiles-v1",
        "# Attack hooks that fire a missile, read from portal.dat skill records (root field 65).",
        "# skill record hook entity",
    ]
    with Dat(args.game_data / "portal.dat") as portal:
        portal.index()
        for row in rows:
            did = int(row[1], 16)
            data = portal.read(did)
            if hashlib.sha256(data).hexdigest() != row[2]:
                raise ValueError(f"skill record {did:08x} differs from the catalog")
            for index, entity in missile_hooks(data):
                out.append(f"{int(row[0], 16):x}\t{did:08x}\t{index}\t{entity:08x}")
    args.output.write_text("\n".join(out) + "\n", encoding="utf-8", newline="\n")
    print(f"skills {len(rows)} missile hooks {len(out) - 3}")


if __name__ == "__main__":
    main()
