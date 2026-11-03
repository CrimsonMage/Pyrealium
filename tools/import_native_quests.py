"""Export native quest text/phase references and exact NPC/death-effect links.

Requires the retail probe (`ac2-retail-probe`, a tool of the AC2_Rusted client project, on PATH or
--probe). This is an authoring catalog,
not permission to execute quests without their eligibility and reward tables.
"""
import argparse
import shutil
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT=Path(__file__).resolve().parents[1]
sys.path[:0]=[str(ROOT/'tools/ac2'),str(ROOT/'tools')]
from content_profile import ContentProfile
from population_probe import Reader,entity

def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('directory',type=Path)
    p.add_argument('--probe',type=Path,default=Path(shutil.which('ac2-retail-probe') or 'ac2-retail-probe'))
    p.add_argument('--output',type=Path,required=True);a=p.parse_args()
    data=json.loads(subprocess.check_output([str(a.probe),str(a.directory),'--content'],encoding='utf-8'))
    profile=ContentProfile(a.directory);templates={};names={}
    try:
        r=Reader(profile.portal.read(0x34000000));r.take(12)
        for _ in range(r.word()):key=r.word();names[key]=r.string()
        for did,*_ in profile.portal.entries():
            if did>>24!=0x47:continue
            try:
                r=Reader(profile.portal.read(did));row=entity(r,profile.types)
                if r.pos==len(r.data):templates[did]=row
            except (KeyError,ValueError,AssertionError):continue
        bindings=[]
        quest_effects={e['package']:e for e in data['quest_effects']}
        quests={q['id']:q for q in data['quests']}
        for did in sorted(templates):
            props={};seen=set();current=did
            while current in templates and current not in seen:
                seen.add(current);row=templates[current]
                for key,value in row.get('properties',{}).items():props.setdefault(names.get(int(key[9:],16),key),value)
                current=row['data']
            effects=[dict(slot=i,effect=props[f'NPC_DeathEffect{i}'],stat=props.get(f'NPC_DeathEffect{i}Stat')) for i in range(1,9) if props.get(f'NPC_DeathEffect{i}')]
            for effect in effects:
                resolved=quest_effects.get(effect['effect'])
                if resolved:
                    effect.update(effect_state=resolved['state'],quest=resolved['quest'],quest_status=resolved['quest_status'],
                                  quest_package=quests.get(resolved['quest'],{}).get('package'))
            if effects or props.get('AI_NPCTable'):
                bindings.append(dict(entity=did,npc_table=props.get('AI_NPCTable'),death_effects=effects))
        data['entity_bindings']=bindings
        data['record_sha256']={f'{did:08x}':hashlib.sha256(profile.portal.read(did)).hexdigest() for did in [0x34000000,0x56000005]}
        scripts=data.pop('scripts')
        data['named_export_count']=sum(len(s['functions']) for s in scripts)
        # Stripped symbol metadata does NOT mean absent bytecode. Keep each
        # package's own virtual entries and distinguish real bodies from stubs.
        data['script_methods']=[dict(package=s['name'],pid=s['pid'],bodies=s['own_bodies']) for s in scripts]
    finally:profile.close()
    a.output.write_text(json.dumps(data,indent=2,ensure_ascii=False)+'\n',encoding='utf-8')
    qs=data['quests'];print(f"{len(qs)} quest definitions; {sum(bool(q['name']['text']) for q in qs)} readable names; {sum(bool(p['journal']['text']) for q in qs for p in q['phases'])} readable phase entries; {len(data['quest_effects'])} quest effects; {len(data['entity_bindings'])} entity links; {sum('quest' in e for b in bindings for e in b['death_effects'])} resolved death-effect quest links.")
if __name__=='__main__':main()
