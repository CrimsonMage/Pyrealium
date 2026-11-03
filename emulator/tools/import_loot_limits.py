"""Recover inherited NPC death-loot quantity controls from the installed DAT.

Quantity distribution and the treasure mutation formula are not in the client.
Quality/variance overrides are retained as evidence, not interpreted as levels.
"""
import argparse
import hashlib
import json
from pathlib import Path

from import_audited_population import ContentProfile
from population_probe import Reader, entity

MINIMUM, MAXIMUM = 0x410002F5, 0x410002F6
QUALITY, VARIANCE = 0x41000275, 0x41000276


def recover(directory):
    profile = ContentProfile(directory)
    rows, hashes, rejected = {}, {}, []
    try:
        for did, *_ in profile.portal.entries():
            if did >> 24 != 0x47:
                continue
            raw = profile.portal.read(did)
            try:
                reader = Reader(raw)
                row = entity(reader, profile.types)
                if reader.pos != len(raw):
                    raise ValueError('trailing entity properties')
                rows[did] = row
                hashes[did] = hashlib.sha256(raw).hexdigest()
            except (ValueError, KeyError, AssertionError):
                rejected.append(f'{did:08x}')
        rules = []
        schema_hash = hashlib.sha256(profile.portal.read(0x34000000)).hexdigest()
        for did in sorted(rows):
            current, props, sources, seen = did, {}, {}, set()
            while current >> 24 == 0x47:
                if current not in rows or current in seen:
                    break
                seen.add(current)
                row = rows[current]
                sources[f'{current:08x}'] = hashes[current]
                for key, value in row.get('properties', {}).items():
                    props.setdefault(int(key[9:], 16), value)
                current = row['data']
            else:
                if MINIMUM not in props or MAXIMUM not in props:
                    continue
                low, high = props[MINIMUM], props[MAXIMUM]
                if not (isinstance(low, int) and isinstance(high, int) and 0 <= low <= high <= 32):
                    raise ValueError(f'invalid native loot limits for {did:08x}: {low}, {high}')
                sources['34000000'] = schema_hash
                rules.append(dict(entity=did, minimum=low, maximum=high, level=props.get(0x41000173),
                                  quality=props.get(QUALITY), variance=props.get(VARIANCE), sources=sources))
        return dict(schema=1, source='installed portal.dat inherited NPC properties',
                    policy='Native min/max bound generic treasure slots. Uniform quantity sampling and item weights are reconstruction; native trophy slots remain separate. Quality/variance are evidence only.',
                    rejected_entities=rejected, rules=rules)
    finally:
        profile.close()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    report = recover(args.directory)
    args.output.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8', newline='\n')
    print(f"Recovered {len(report['rules'])} inherited quantity overrides; {len(report['rejected_entities'])} entity layouts withheld")
