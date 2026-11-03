"""Recover native trophy probabilities, without substituting a captured bundle.

NPC_TrophyDrop1..15 and matching _Rate properties are decoded using the installed
MasterProperty table. Special trophies have separate eligibility rules and are
not enabled by this importer. Every inheritance record is fingerprinted.
"""
import argparse
import json
import math
from pathlib import Path
import struct
import sys

ROOT=Path(__file__).resolve().parents[1]
sys.path[:0]=[str(ROOT/'tools/ac2'),str(ROOT/'tools')]
from content_profile import ContentProfile
from population_probe import Reader,entity

def fnv(data):
    value=0xcbf29ce484222325
    for byte in data:value=((value^byte)*0x100000001b3)&0xffffffffffffffff
    return f'{value:016x}'

def recover(directory):
    profile=ContentProfile(directory); records={}; result=[]; rejected=[]
    try:
        r=Reader(profile.portal.read(0x34000000));r.take(12);keys={}
        for _ in range(r.word()):
            key=r.word();keys[r.string()]=key
        for did,*_ in profile.portal.entries():
            if did>>24!=0x47:continue
            data=profile.portal.read(did)
            try:
                r=Reader(data);row=entity(r,profile.types)
                if r.pos!=len(data):continue
                records[did]=(row,fnv(data))
            except (ValueError,KeyError,AssertionError):continue
        for did in sorted(records):
            props={}; sources={}; current=did
            while current in records and current not in sources:
                row,digest=records[current];sources[current]=digest
                for key,value in row.get('properties',{}).items():props.setdefault(int(key[9:],16),value)
                current=row['data']
            drops=[]
            for slot in range(15):
                key=keys[f'NPC_TrophyDrop{slot+1}']
                if key not in props:continue
                definition=props[key]
                if definition==0:continue
                rate=struct.unpack('<f',struct.pack('<I',props.get(keys[f'NPC_TrophyDrop{slot+1}_Rate'],0xffffffff)))[0]
                visual=profile.visual(definition)
                if not math.isfinite(rate) or not 0<=rate<=1 or definition>>24!=0x47 or not visual:
                    rejected.append(dict(entity=f'{did:08x}',slot=slot+1,reason='missing/invalid item, visual or probability'));continue
                drops.append(dict(definition=definition,visual=visual,probability=rate,slot=slot+1))
            if drops:result.append(dict(entity=did,sources={f'{k:08x}':v for k,v in sources.items()},drops=drops))
    finally:profile.close()
    return dict(schema=1,source='Installed EntityDesc inheritance and MasterProperty NPC_TrophyDrop1..15 / _Rate',
                rules=result,rejected=rejected,limitations='Special trophy eligibility and generic treasure generation are not recovered here.')

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('directory',type=Path);p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    report=recover(a.directory);a.output.write_text(json.dumps(report,indent=2)+'\n')
    print(f"{len(report['rules'])} creature trophy rules; {sum(len(r['drops']) for r in report['rules'])} drops; {len(report['rejected'])} rejected.")
if __name__=='__main__':main()
