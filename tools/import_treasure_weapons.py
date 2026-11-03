"""Recover observed weapon identities and use requirements, never retail rolls.

Native IntStats (archived Pyreal Interfaces/Entity/IntStats.cs): ImplementType
0xfa0, valid/preferred slots 0x113/0x11a, Usage_MinLevel 0x2bc, Weapon_Damage
0x258. Generic item defaults omit damage; their instance rolls are server policy.
"""
import argparse
import hashlib
import json
import struct
from pathlib import Path

from import_audited_population import ContentProfile
from recover_apparel import qualities


def recover(directory):
    profile = ContentProfile(directory)
    weapons, withheld = [], []
    try:
        for line in (Path(__file__).resolve().parents[1] / 'content/loot-items.tsv').read_text(encoding='utf-8').splitlines():
            fields = line.split('\t')
            if len(fields) != 3 or not fields[0].startswith('47'):
                continue
            did, visual = (int(v, 16) for v in fields[:2])
            integers, dids = qualities(profile.portal, did)
            implement, mask = integers.get(0xfa0, 0), integers.get(0x113, 0)
            if implement in (0, 1, 16) or not mask & 0x40:
                continue
            # Additional usage restrictions and unimplemented property tails
            # must not become a weapon that silently bypasses its conditions.
            unsupported = set(integers).intersection(set(range(0x2bd, 0x2cd)) | {0x2d8})
            if unsupported:
                withheld.append(dict(definition=did, name=fields[2], reason='additional usage restrictions', stats=sorted(unsupported)))
                continue
            physical = dids[1]
            if (mask not in (0x40, 0xc0) or integers.get(0x11a) != 0x40
                    or not 1 <= integers.get(0x2bc, 1) <= 150
                    or profile.visual(did) != visual
                    or profile.portal.read(physical) not in (
                        struct.pack('<4I', 11, physical, 0x40000001, visual),
                        struct.pack('<9I', 0x8b, physical, 0x40000001, visual, 1, 0x40000002, 1, 0x40000015, 1))):
                raise ValueError(f'unsupported weapon identity {did:08x}')
            sources = {f'{asset:08x}': hashlib.sha256(profile.portal.read(asset)).hexdigest()
                       for asset in (did, 0x81000000 | (did & 0xffffff), physical, visual)}
            weapons.append(dict(definition=did, visual=visual, name=fields[2], mask=mask,
                                implement=implement, minimum_level=integers.get(0x2bc, 1),
                                damage=integers.get(0x258), sources=sources))
        return dict(schema=1, policy='Observed identities and native use levels. Missing damage uses Pyrealium reconstruction: (6 + 2 * treasure level) with +/-15% item variance. No retail affix weights recovered.',
                    weapons=weapons, withheld=withheld)
    finally:
        profile.close()


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('directory', type=Path)
    p.add_argument('output', type=Path)
    args = p.parse_args()
    report = recover(args.directory)
    args.output.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8', newline='\n')
    print(f"Recovered {len(report['weapons'])} weapon profiles; {len(report['withheld'])} withheld")
