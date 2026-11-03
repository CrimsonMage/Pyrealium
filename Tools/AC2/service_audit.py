"""Join the read-only DAT audit with the selected server pack; never invent spawns.

Run population_probe.py first. Template existence, artwork placement and a live
server actor are different kinds of evidence. This report keeps them separate.
"""
import argparse
import collections
import json
from pathlib import Path


def audit(population, content, encounter_entities):
    templates = population['templates']
    names = population['property_names']

    def resolve(did):
        chain, seen = [], set()
        while f'{did:08X}' in templates and did not in seen:
            seen.add(did)
            record = templates[f'{did:08X}']
            chain.append(record)
            did = record['data']
        properties = {}
        for record in reversed(chain):
            properties.update({names.get(str(int(k[9:], 16)), k): v
                               for k, v in record.get('properties', {}).items()})
        return did, properties

    placements = []
    for line in content.splitlines():
        if not line or line.startswith('#') or line.startswith('pyrealium-'):
            continue
        f = line.split('\t')
        if len(f) not in (12,13):
            raise ValueError('Expected a version-3/4 server placement row')
        entity, cell = int(f[1], 16), int(f[3], 16)
        terminal, props = resolve(entity)
        role = ('door' if 3756 <= terminal <= 3764 or terminal == 3922 else
                'chest' if terminal == 3723 else
                'sign' if terminal == 411 or 4000 <= terminal <= 4075 else
                'vendor' if props.get('StoreTemplate') or props.get('StoreGroup') else
                'encounter' if entity in encounter_entities else 'other-scripted-object')
        placements.append(dict(instance=f[0], entity=f[1], visual=f[2], cell=f[3],
            block=f'{cell >> 16:04X}', position_m=list(map(float, f[4:7])),
            role=role, script_class=terminal, ai_table=props.get('AI_NPCTable'),
            store_template=props.get('StoreTemplate'), store_group=props.get('StoreGroup')))
    blocks = {}
    for p in placements:
        counts = blocks.setdefault(p['block'], collections.Counter())
        counts[p['role']] += 1
    return dict(
        scope='Selected compatible capture/authored pack, not a retail spawn database',
        static_dat=dict(outdoor=population['placement_count'],
            indoor=population['interior_placement_count'],
            creature_placements=len(population['known_creature_placements']),
            encounter_descriptor=population['encounter_descriptor'],
            rejected=population['rejected']),
        selected_pack_counts=dict(collections.Counter(p['role'] for p in placements)),
        blocks=blocks, placements=placements,
        limitations=[
            'A class or mesh is not evidence for a spawn location, vendor stock or quest binding.',
            'The selected pack contains both compatible observations and explicit Pyrealium authored content.',
            'Encounter rules, respawn delays, aggression and rewards remain Pyrealium policy.',
            'No static scenery is promoted to a door, chest, sign or NPC by visual similarity.',
        ])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--population', required=True, type=Path)
    parser.add_argument('--content', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    # Match runtime evidence, not the historical list that also includes town NPCs.
    entities = {int(line.split('\t')[0],16) for line in
                (root/'emulator/content/creature-combat.tsv').read_text().splitlines()
                if line and not line.startswith(('#','pyrealium-'))}
    entities.update(int(line.split('\t')[1],16) for line in
                    (root/'emulator/content/development-encounters.tsv').read_text().splitlines()
                    if line.startswith('npc\t'))
    # Same authored dialogue identities as training_dialogue/shelter dispatch.
    entities.difference_update({0x47001699,0x47002448,*range(0x47001573,0x47001582)})
    result = audit(json.loads(args.population.read_text(encoding='utf-8')),
                   args.content.read_text(encoding='utf-8'), entities)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2), encoding='utf-8')
    print(json.dumps({k: result[k] for k in ('scope', 'static_dat', 'selected_pack_counts')}, indent=2))


if __name__ == '__main__':
    main()
