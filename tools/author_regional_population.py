"""Reconstruct sparse outdoor populations from AC2 wiki rosters and Pyreal guide geography.

Region envelopes, density and regeneration are explicit Pyrealium inferences. No
capture, quest policy, DAT placement or existing camp is changed. Grounding is
a separate native-geometry pass; this file alone cannot enable new placements.
"""
import argparse
import hashlib
import json
import math
import re
import struct
from collections import defaultdict
from pathlib import Path

from author_recovery_content import CONTENT, ROOT, coordinate, load, norm

REGIONS = {
    'Arwic Outland': 'Arwic Outlands', 'Esper': 'Esper Reaches', 'Lost Wish': 'Lost Wish',
    'Mad Crone': 'Mad Crone Mountains', 'Cobalt': 'Cobalt Sweeps', 'Drudge Citadel': 'Drudge Citadel',
    'Prosper': 'Prosper Marches', 'Kydi': 'Kydi Delta', 'Ariaki': 'Ariaki River Basin',
    'Lumari': 'Lumari Spill Isles', 'Tou-Tou': 'Tou-Tou Peninsula', 'Naderu': 'Naderu Greenwood',
    'Farali': 'Farali Wasteland', 'Vesayan': 'Vesayen Islands', 'Oriad': 'Rings of Oriad',
    'Stoneshadow': 'Stoneshadow Plains', 'Thusik': 'Thusik Delta', 'Nepeth': 'Nepeth Strata',
    'Winterhollows': 'Winterhollows', 'Malthabbor': 'Mount Malthabbor', 'Gevoth': 'Gevoth',
}
PEACEFUL = {0,14,23,24,25,27,28,29,30,31,32,35,37,38,45,48,55,56,59,60,61,79}

def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

def cross(o,a,b):
    return (a[0]-o[0])*(b[1]-o[1])-(a[1]-o[1])*(b[0]-o[0])

def hull(points):
    points=sorted(set(map(tuple,points)))
    def half(points):
        out=[]
        for p in points:
            while len(out)>1 and cross(out[-2],out[-1],p)<=0:out.pop()
            out.append(p)
        return out
    return half(points)[:-1]+half(reversed(points))[:-1]

def inside(point,polygon):
    return len(polygon)>=3 and all(cross(a,b,point)>=-1e-8 for a,b in zip(polygon,polygon[1:]+polygon[:1]))

def seed(text):
    return int.from_bytes(hashlib.sha256(text.encode()).digest()[:8],'little')

def region_rows(article):
    lines=article['text'].splitlines()
    start=next((i for i,l in enumerate(lines) if l in ('Monsters','Creatures')),None)
    if start is None:return []
    result=[]
    for line in lines[start+1:]:
        if line in ('Maps','Quests','NPCs','Points of Interest'):break
        match=re.fullmatch(r'(.+),\s*(\d+)\s*(group)?',line.strip())
        if match:result.append((match[1],int(match[2]),bool(match[3])))
    return result

def author(templates_path,wiki_path,output):
    templates={int(k):v for k,v in load(templates_path).items()}
    articles={a['title']:a for a in load(wiki_path)['articles']}
    reference=load(CONTENT/'reference-tables.json')
    protected={int(l,16) for l in (CONTENT/'population-protected.txt').read_text().splitlines() if re.fullmatch('[0-9a-fA-F]{8}',l)}
    quests=load(CONTENT/'inferred-content-v1.json')['quests']
    # Single-kill quest targets belong to their authored encounter, not the
    # regional wilderness pool. The quest catalog stores goals, not a kind.
    boss_targets={d for q in quests if q['goal']==1 for d in q['targets']}
    names=defaultdict(list)
    for did,t in templates.items():names[norm(t['name'].split('[')[0])].append(did)
    regions=[]; excluded=[]; towns=[]
    for source in reference['sql_sources']:
        for row in source['tables'].get('towns',[]):
            p=coordinate(row['locN'],row['locE'])
            if p:towns.append({'name':row['name'],'land':p,'radius_m':220})
        current=None
        for row in source['tables'].get('ringways',[]):
            if row['Name'].endswith(' Ringway'):
                key=row['Name'].removesuffix(' Ringway')
                title=REGIONS.get(key)
                if not title:raise ValueError('Unknown ringway '+key)
                article=articles[title]
                current={'name':title,'anchors':[],'center':coordinate(row['locN'],row['locE']),
                         'source':{'wiki_url':'https://20oz.net/index.php/'+title.replace(' ','_')+'.html',
                                   'archived_url':article['url'],'html_sha256':article['html_sha256'],
                                   'sql_sha256':source['sha256']},'creatures':[]}
                for name,level,elite in region_rows(article):
                    candidates=[]
                    for did in names[norm(name)]:
                        p=templates[did]['props']
                        if (did not in protected|boss_targets and p.get('GameplayStatistics_Level')==level
                            and p.get('GameplayStatistics_MaxHealth',0)>0 and p.get('AICombat_NaturalDamage',0)>0
                            and p.get('AI_LoveTable',0) not in PEACEFUL and not p.get('AI_IdleOnly',0)
                            and not p.get('NPC_IsLootProof',0) and ',' not in name
                            and not any('CompleteQuest' in str(v) or 'Gauntlet' in str(v) for k,v in p.items() if 'DeathEffect' in k)):
                            candidates.append(did)
                    if not candidates:
                        excluded.append({'region':title,'name':name,'level':level,'reason':'No unambiguous eligible ordinary combat profile at the guide level'})
                        continue
                    # Equivalent named variants are retained as evidence; prefer
                    # the earliest compatible base rather than scripted variants.
                    did=min(candidates)
                    current['creatures'].append({'entity':did,'name':templates[did]['name'],
                        'level':level,'elite':elite,'variants':sorted(candidates)})
                regions.append(current)
            p=coordinate(row['locN'],row['locE'])
            if current and p:current['anchors'].append({'name':row['Name'],'land':p})
    if len(regions)!=21:raise ValueError('Expected unique guide geography for 21 regions')
    for region in regions:
        region['polygon']=hull([a['land'] for a in region['anchors']])
        if len(region['polygon'])<3 or not region['creatures']:raise ValueError('Incomplete region '+region['name'])
    camps=[]
    # One jittered candidate per landblock inside a guide-anchor convex hull.
    # Overlapping hulls use the closest documented ringway centre. This is an
    # intentionally bounded approximation, not the missing retail region map.
    for bx in range(255):
        for by in range(255):
            h=seed(f'regional-v1:{bx}:{by}')
            p=[bx*8+2+(h&65535)/65535*4,by*8+2+((h>>16)&65535)/65535*4]
            choices=[r for r in regions if inside(p,r['polygon'])]
            if not choices:continue
            r=min(choices,key=lambda r:math.dist(p,r['center']))
            if any(math.dist(p,t['land'])*20<t['radius_m']+40 for t in towns):continue
            # Two ordinary creatures, or one stronger elite: 'group' on the
            # wiki means fellowship difficulty, never a recovered child count.
            c=r['creatures'][(h>>32)%len(r['creatures'])]
            count=1 if c['elite'] else 2
            iid=0x8140c00000000000|(bx<<16)|(by<<8)
            camps.append({'id':iid,'land':p,'region':r['name'],'roster':[c['entity']]*count,
                          'capacity':count,'regeneration_ms':180000,'polygon':r['polygon']})
    entities=sorted({d for c in camps for d in c['roster']})
    result={'version':1,'policy':'Inferred regional wilderness, not recovered retail generators',
        'regions':regions,'town_exclusions':towns,'camps':camps,'npcs':[],
        'templates':{str(d):{'visual':templates[d]['visual'],'name':templates[d]['name']} for d in entities},
        'excluded':excluded,'sources':{Path(p).name:digest(p) for p in [templates_path,wiki_path,CONTENT/'reference-tables.json']}}
    output.write_text(json.dumps(result,indent=2)+'\n',encoding='utf8')
    (output.parent/'regional-creature-entities.txt').write_text(''.join(f'{d:08x}\n' for d in entities))
    fields=[('AI_PerceptionRadius',True),('AI_HomesickRadius',True),('AI_Wandering',False),
            ('AI_WanderingRange',True),('AI_WanderingProb',True),('AI_LoveTable',False),
            ('AI_FreeAttacking',False),('AI_IdleOnly',False),('NPC_DeathLootProfile',False),('NPC_IsLootProof',False)]
    ai=['pyrealium-creature-ai-v1']
    for did in entities:
        values=[]
        for key,floating in fields:
            v=templates[did]['props'].get(key)
            if v is not None and floating:v=struct.unpack('<f',struct.pack('<I',v))[0]
            values.append('-' if v is None else str(v))
        ai.append(f'{did:08x}\t'+'\t'.join(values))
    (output.parent/'regional-creature-ai-v1.tsv').write_text('\n'.join(ai)+'\n')
    print(f'{len(regions)} regions, {len(camps)} candidate camps, {sum(len(c["roster"]) for c in camps)} slots, {len(entities)} native creatures; {len(excluded)} unmatched/unsafe names withheld')

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--templates',type=Path,default=ROOT/'Saved/current-native-templates.json')
    p.add_argument('--wiki',type=Path,default=ROOT/'Saved/ContentRecovery/wiki-content.json')
    p.add_argument('--output',type=Path,default=CONTENT/'regional-content-v1.json')
    a=p.parse_args();author(a.templates,a.wiki,a.output)
