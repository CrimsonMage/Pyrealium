"""Recover the mine Outcast's combat and AI fields from the installed DAT.

Room placements and respawn schedules are separate reconstruction policy.
Absent native fields remain absent; this does not infer wandering or attacks.
"""
import argparse
import hashlib
import json
import struct
from pathlib import Path

from import_audited_population import ContentProfile
from population_probe import Reader, entity
from import_creature_ai import export
from import_creature_combat import SKILL_TYPES, SKILL_LEVELS, ATTACK_SKILLS, DEFENSE_SKILLS, interval_ms


def recover(directory, destination):
    profile = ContentProfile(directory)
    templates, properties, hashes = {}, {}, {}
    did = 0x470015ac
    try:
        while did >> 24 == 0x47:
            if f'{did:08X}' in templates:
                raise ValueError('cyclic Outcast inheritance')
            raw = profile.portal.read(did)
            reader = Reader(raw)
            row = entity(reader, profile.types)
            if reader.pos != len(raw):
                raise ValueError('unparsed Outcast properties')
            templates[f'{did:08X}'] = row
            hashes[f'{did:08x}'] = hashlib.sha256(raw).hexdigest()
            for key, value in row.get('properties', {}).items():
                properties.setdefault(int(key[9:], 16), value)
            did = row['data']
        hashes['34000000'] = hashlib.sha256(profile.portal.read(0x34000000)).hexdigest()
        number = lambda key: properties.get(key, 0)
        floating = lambda key: struct.unpack('<f', struct.pack('<I', number(key)))[0]
        rating = lambda kinds: max((number(level) for skill, level in zip(SKILL_TYPES, SKILL_LEVELS)
                                   if number(skill) in kinds), default=0)
        natural_time = floating(0x41000187) if profile.types[0x41000187] == 0x40000003 else number(0x41000187)
        xp = int.from_bytes(bytes.fromhex(properties[0x41000172]), 'little')
        fields = [number(0x4100016e), number(0x41000184), interval_ms(natural_time) if natural_time else 0,
                  number(0x410000de), xp, number(0x41000173), round(floating(0x41000185), 4),
                  rating(ATTACK_SKILLS), rating(DEFENSE_SKILLS), floating(0x4100017a),
                  floating(0x41000178), number(0x410002f9), number(0x41000170)]
        combat = ('pyrealium-creature-combat-v3\n'
                  '# Native inherited Outcast fields. Absent weapon time stays 0 (runtime default).\n'
                  '# entity health damage interval_ms armor xp level variance attack defense perception homesick damage_type max_vigor\n'
                  '470015ac\t' + '\t'.join(map(str, fields)) + '\n')
        ai = export({'templates': templates}, {0x470015ac})
        outputs = {'arwic-mines-creature-combat-v1.tsv': combat, 'arwic-mines-creature-ai-v1.tsv': ai}
        for filename, text in outputs.items():
            (destination / filename).write_text(text, encoding='utf-8', newline='\n')
        audit = {'entity': '470015ac', 'source': 'installed portal.dat', 'source_sha256': hashes,
                 'properties': {f'{key:08x}': value for key, value in sorted(properties.items())},
                 'output_sha256': {name: hashlib.sha256(text.encode()).hexdigest() for name, text in outputs.items()},
                 'policy': 'Native fields only. Existing Pyrealium hostility, attack timing and collision scheduling apply; no absent wandering is invented.'}
        (destination / 'arwic-mines-creatures-v1.audit.json').write_text(json.dumps(audit, indent=2) + '\n', encoding='utf-8')
        print(combat, ai)
    finally:
        profile.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('destination', type=Path)
    args = parser.parse_args()
    recover(args.directory, args.destination)
