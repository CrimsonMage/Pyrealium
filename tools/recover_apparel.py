"""Recover unambiguous wearable families from native pile/worn appearance pairs.

Never copy a quest item's armor rating to a generated loot item. A shared pile
appearance establishes the worn meshes, race and slot, not randomized statistics.
"""
import argparse
import collections
import json
import struct
from pathlib import Path
from import_loot_tables import ROOT, templates, Reader


def qualities(portal, did):
    r = Reader(portal.read(0x81000000 | (did & 0xffffff)))
    mask = r.word()
    assert r.word() == (0x81000000 | (did & 0xffffff)) and mask & 0x21 == 0x21
    desc = r.word()
    for bit in (1, 0x2000000, 2):
        if desc & bit: r.word()
    for bit in (4, 8):
        if desc & bit:
            r.take(8)
            assert r.word() == 0
    for bit in (0x10,0x20,0x40,0x100,0x200,0x20000000,0x40000000,0x4000000,0x80,0x800,0x1000,0x2000,0x200000,0x400000,0x8000,0x10000,0x20000,0x40000,0x80000,0x100000,0x8000000,0x10000000):
        if desc & bit: r.take(8 if bit in (0x20,0x40,0x100,0x200,0x20000000,0x40000000,0x4000000) else 4)
    tables = {}
    for bit, width in ((2,2),(4,2),(8,2),(16,3),(32,2)):
        if mask & bit:
            n = r.word() & 0xffffff
            assert n <= 512
            table = tables[bit] = {}
            for _ in range(n):
                k, v = r.word(), r.word()
                assert k not in table
                table[k] = v
                r.take((width-2)*4)
    return tables.get(2, {}), tables[32]


def fingerprint(data):
    result = 0xcbf29ce484222325
    for byte in data: result = ((result ^ byte) * 0x100000001b3) & ((1 << 64)-1)
    return result


def recover(directory):
    profile, _, records = templates(directory)
    families = collections.defaultdict(lambda: collections.defaultdict(list))
    for did, row in records.items():
        p = row['props']
        keys = (0x4100024d,0x41000245,0x4100008a,0x41000247,0x4100024a)
        if all(p.get(k) for k in keys) and p.get(0x41000248) == p[0x41000245]:
            families[p[keys[0]]][tuple(p[k] for k in keys[1:])].append(did)
    rows = []
    source_lines=(ROOT/'content/loot-items.tsv').read_text().splitlines()[1:]
    tradable={int(line.split('\t')[0],16) for line in source_lines if line and not line.startswith('#')}
    craft_file=ROOT/'content/professions-v1.json'
    if craft_file.exists():
        catalog=json.loads(craft_file.read_text(encoding='utf-8'))
        tradable.update(o['item'] for r in catalog['recipes'] if r['kind']=='produce' for o in r['outputs'])
        source_lines += [f'{i["id"]:08x}\t{i["visual"]:08x}\t{i["name"]}' for i in catalog['items']]
    seen=set()
    for line in source_lines:
        if not line or line.startswith('#'): continue
        definition, visual, name = line.split('\t')
        did, vis = int(definition,16), int(visual,16)
        if did in seen: continue
        seen.add(did)
        try:
            ints, dids = qualities(profile.portal, did)
            choices = families.get(dids.get(0x103), {})
            if len(choices) != 1: continue
            (race, slot, male, female), proof = next(iter(choices.items()))
            if race not in (1,2,4,8,16) or slot not in (1,2,4,8,16,32): continue
            if ints.get(0x113) != slot or ints.get(0x11a) != slot: continue
            # This bounded set has no level gate beyond level 1. Do not silently
            # admit new restricted profiles when extending the source corpus.
            if ints.get(0x2bc, 1) != 1: continue
            if records[did]['visual'] != vis: continue
            # Restrict recovery to the proved simple physics descriptor. More
            # complicated quest effects need their own server implementation.
            if profile.portal.read(dids[1]) != struct.pack('<4I',11,dids[1],0x40000001,vis): continue
            for asset in (male, female, dids[1], vis): profile.portal.read(asset)
            p = records[did]['props']
            armor = p.get(0x410000a7, ints.get(0x262))
            rows.append(dict(definition=did, visual=vis, physical=dids[1], appearance=dids[0x103],
                race=race, slot=slot, male=male, female=female, armor=armor,
                hash=fingerprint(profile.portal.read(0x81000000 | (did & 0xffffff))),
                entity_hash=fingerprint(profile.portal.read(did)), evidence=proof, name=name, tradable=did in tradable))
        except (KeyError, AssertionError, ValueError): continue
    return rows


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--dat', type=Path, default=ROOT/"Asheron's Call 2 - Legions")
    args = parser.parse_args()
    rows = recover(args.dat)
    fields = ('definition','visual','physical','appearance','race','slot','male','female','armor','hash','entity_hash')
    def value(k,v):
        if k == 'armor': return 'None' if v is None else f'Some({v})'
        return str(v) if k in ('race','slot') else f'0x{v:x}'
    (ROOT/'emulator/src/recovered_apparel_data.rs').write_text(
        '// Generated by tools/recover_apparel.py; statistics are never inferred from appearance.\n'
        'const RECOVERED_PROFILES:&[Profile]=&[\n' + ''.join('    Profile{'+','.join(f'{k}:{value(k,r[k])}' for k in fields)+'},\n' for r in rows) + '];\n'
        '// Only ordinary loot/manufactured outputs; dye input eligibility is not sale permission.\n'
        'const RECOVERED_TRADABLE:&[u32]=&['+','.join(f'0x{r["definition"]:08x}' for r in rows if r['tradable'])+'];\n')
    (ROOT/'content/recovered-apparel-evidence.json').write_text(json.dumps(rows, indent=2)+'\n')
    print(f'Recovered {len(rows)} wearable profiles; {sum(r["armor"] is not None for r in rows)} have native armor ratings.')
