"""Author versioned, explicitly inferred gameplay from the recovery catalog.

Never edits captures or DATs. Native death-effect bindings determine quest
identity; guide coordinates seed camps, NOT exact retail generator positions.
Run population_authoring afterwards to resolve ground and reject bad positions.
"""
import argparse
import hashlib
import json
import math
import re
import struct
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CONTENT = ROOT / 'emulator/content'

def load(path):
    return json.loads(Path(path).read_text(encoding='utf-8'))

def norm(value):
    return re.sub('[^a-z0-9]', '', value.lower())

def number(value, default):
    match = re.search(r'\d[\d,]*', value or '')
    return int(match[0].replace(',', '')) if match else default

def duration(value, default):
    match = re.search(r'(\d+)\s*(min|hour|day)', value or '', re.I)
    return int(match[1]) * {'min': 60, 'hour': 3600, 'day': 86400}[match[2].lower()] if match else default

def coordinate(north, east):
    n = re.fullmatch(r'(\d+(?:\.\d+)?)([NS])', str(north).strip(), re.I)
    e = re.fullmatch(r'(\d+(?:\.\d+)?)([EW])', str(east).strip(), re.I)
    if not n or not e or float(n[1]) == float(e[1]) == 0:
        return None
    return [1020 + float(e[1]) * (10.2 if e[2].upper() == 'E' else -10.2),
            1020 + float(n[1]) * (10.2 if n[2].upper() == 'N' else -10.2)]

def author(templates_path, wiki_path, output):
    templates = {int(k): v for k, v in load(templates_path).items()}
    native = load(CONTENT / 'native-quests.json')
    reference = load(CONTENT / 'reference-tables.json')
    articles = load(wiki_path)['articles']
    by_name = defaultdict(list)
    for did, t in templates.items():
        by_name[norm(t['name'].split('[')[0])].append(did)
    protected = {int(l, 16) for l in (CONTENT / 'population-protected.txt').read_text().splitlines() if re.fullmatch('[0-9a-fA-F]{8}', l)}
    def combat(did):
        p = templates.get(did, {}).get('props', {})
        return did not in protected and p.get('GameplayStatistics_MaxHealth', 0) > 0 and p.get('AICombat_NaturalDamage', 0) > 0 and p.get('AI_LoveTable', 0) not in (0,24,79)
    guide = []
    npcs = []
    for s in reference['sql_sources']:
        for row in s['tables'].get('incursions', []):
            guide.append((row, s['sha256']))
        for row in s['tables'].get('npc', []):
            npcs.append((row, s['sha256']))
    bindings = defaultdict(lambda: defaultdict(set))
    for e in native['entity_bindings']:
        for effect in e['death_effects']:
            if effect.get('quest') and combat(e['entity']):
                for action in ('Bestow', 'Increment', 'Complete'):
                    if action in effect['effect']:
                        bindings[effect['quest']][action].add(e['entity'])
    article_by_title = {norm(a['title']): a for a in articles}
    index_by_starter = {norm(r['values']['Quest Starter']): r for r in reference['incursion_reference_rows']}
    profiles, excluded = [], []
    for q in native['quests']:
        journals = [p['journal']['text'] for p in q['phases'] if p['journal'].get('text')]
        match = next((m for text in journals if (m := re.search(r'(?:Kill|Slay|Defeat)\s+(\d+)\s+([^\n.]+)', text, re.I))), None)
        boss = not match and bool(bindings[q['id']]['Bestow']) and bool(bindings[q['id']]['Complete'])
        targets = sorted(bindings[q['id']]['Complete' if boss else 'Increment'])
        if not targets or not journals or (not match and not boss):
            continue
        goal = 1 if boss else int(match[1])
        target_text = templates[targets[0]]['name'] if boss else match[2]
        if not 1 <= goal <= 100:
            continue
        starters = sorted(bindings[q['id']]['Bestow'])
        name = q['name'].get('text') or target_text
        # "Incursion" is the shared native title, not a unique guide article.
        article = article_by_title.get(norm(name)) if norm(name) != 'incursion' else None
        index = None
        for did in starters:
            index = index_by_starter.get(norm(templates[did]['name']))
            if index:
                article = article_by_title.get(norm(index['values']['Quest Name']), article)
                break
        facts = article['facts'] if article else {}
        use_starters = []
        if not starters:
            starter_name = facts.get('Quest Starter', '').split(',')[0].replace(' drop', '').strip()
            use_starters = by_name.get(norm(starter_name), []) if starter_name else []
        # Native hunt elixirs have no named item-use effect in the DAT defaults.
        elixirs = {0x40000009:0x47000f42, 0x4000000a:0x47000f43, 0x4000000b:0x47000f40}
        if q['id'] in elixirs:
            use_starters = [elixirs[q['id']]]
        if not starters and not use_starters:
            excluded.append({'quest':q['id'], 'reason':'No bestow binding or identifiable guide starter'})
            continue
        target_level = min(templates[d]['props'].get('GameplayStatistics_Level', 1) for d in targets)
        level = number(facts.get('Requirements', index['values'].get('Req', '') if index else ''), max(1, target_level-3))
        xp = number(facts.get('XP Reward', facts.get('Rewards', index['values'].get('XP', '') if index else '')), max(100, goal * target_level * 20))
        title = article['title'] if article else (index['values']['Quest Name'] if index else
                 ('Incursion: '+target_text if norm(name)=='incursion' else name))
        profiles.append(dict(id=q['id'], title=title, text=journals[0], goal=goal, targets=targets,
                             kill_starters=starters, use_starters=use_starters, level=min(150,level), xp=min(100000000,xp),
                             repeat_seconds=duration(facts.get('Repeat Time'), 72000),
                             limit_seconds=duration(facts.get('Time Limit'), 3600),
                             evidence={'native_package':q['package'], 'article':article['url'] if article else None,
                                       'article_hash':article['html_sha256'] if article else None},
                             inference='Native kill/count bindings; guide reward/level when available, otherwise 20 XP per target level per kill. Detail-page timer wins conflicts. Solo credit; automatic payout.'))
    # Native Kanten1/2 effect IDs exist, but their missing quest definitions do
    # not establish which is Gambling Debt. Use a separate reconstruction ID.
    debt=article_by_title[norm('Gambling Debt')]
    profiles.append(dict(id=0x4000fe01,title='Gambling Debt',delivery=True,
        text="Kanten needs a payoff delivered to Orven in Arwic (44.9N, 7.1W). Carry Kanten's Bag to Orven within two hours.",
        goal=1,targets=[],kill_starters=[],use_starters=[],level=11,xp=1000,
        repeat_seconds=7200,limit_seconds=7200,
        evidence=dict(article=debt['url'],article_hash=debt['html_sha256']),
        inference="Guide flow/reward/timers. Dedicated reconstruction quest ID; this is the only enabled option of Kanten's random quest set."))
    camps, fixed, used = [], [], set()
    def camp(key, land, roster, evidence):
        if not land or key in used: return
        used.add(key)
        # Camp density is independent of quest kill count. Five candidate slots,
        # four active; one starter plus a rotating selection of regular targets.
        camp_id = 0x8140A00000000000 | int(hashlib.sha256(key.encode()).hexdigest()[:10],16)
        camps.append(dict(id=camp_id, land=land, roster=roster, capacity=min(4,len(roster)),
                          radius=22, regeneration_ms=120000, evidence=evidence))
    # The SQL npc table also lists named quest-starter monsters.
    guide += [(dict(StarterMob=row['name'],loc1N=row.get('locN'),loc1E=row.get('locE')),sha) for row,sha in npcs]
    for row, sha in guide:
        candidates = by_name.get(norm(row['StarterMob']), [])
        # Old guide sometimes reverses adjective order.
        if not candidates:
            words = sorted(row['StarterMob'].lower().split())
            candidates = [d for d,t in templates.items() if sorted(t['name'].lower().split()) == words]
        q = next((q for q in profiles if any(d in q['kill_starters'] for d in candidates)), None)
        if not q: continue
        starter = next(d for d in q['kill_starters'] if d in candidates)
        target = q['targets']
        for i in range(1,6):
            land = coordinate(row.get(f'loc{i}N'), row.get(f'loc{i}E'))
            roster = [starter] + [target[j % len(target)] for j in range(1 if q['goal']==1 else 4)]
            camp(f"guide:{starter}:{land}",land,roster,dict(sql_sha256=sha,row=row,quest=q['id']))
    # Supplement unrepresented quest species around existing compatible capture
    # anchors only. Preserve captures; avoid unbounded uniform world scattering.
    existing = []
    for line in (ROOT/'data/world/legions-world.tsv').read_text().splitlines():
        if not line.startswith('8140') or line.startswith(('8140a0','8140b0')): continue
        f=line.split('\t');d=int(f[1],16);cell=int(f[3],16)
        if 1 <= (cell & 65535) <=64:
            existing.append((d,[(cell>>24)*8+float(f[4])/20,((cell>>16)&255)*8+float(f[5])/20]))
    for q in profiles:
        if any(c['evidence'].get('quest')==q['id'] for c in camps):continue
        anchor=next(((d,p) for d,p in existing if d in q['targets']+q['kill_starters']),None)
        if anchor and q['kill_starters']:
            d,p=anchor;camp(f"capture-near:{q['id']}",[p[0]+2,p[1]],
                [q['kill_starters'][0]]+[q['targets'][j%len(q['targets'])] for j in range(1 if q['goal']==1 else 4)],
                dict(quest=q['id'],anchor_entity=d,origin='near compatible captured creature; 40m east search origin'))
    # Guide NPCs stay noncombat services. Prefer existing exact template anywhere
    # over an ambiguous second town coordinate (e.g. the conflicting Orven rows).
    all_existing={int(l.split('\t')[1],16) for l in (ROOT/'data/world/legions-world.tsv').read_text().splitlines() if l.startswith('8140') and not l.startswith(('8140a0','8140b0'))}
    for row,sha in npcs:
        choices=[d for d in by_name.get(norm(row['name']),[]) if not combat(d)]
        if len(choices)!=1 or choices[0] in all_existing:continue
        d=choices[0];land=coordinate(row.get('locN'),row.get('locE'))
        if land:
            key=f'npc:{d}';iid=0x8140B00000000000|int(hashlib.sha256(key.encode()).hexdigest()[:10],16)
            fixed.append(dict(id=iid,entity=d,land=land,evidence=dict(sql_sha256=sha,row=row)))
            all_existing.add(d)
    entities=sorted({d for c in camps for d in c['roster']})
    result=dict(version=1,policy='User-authorized inferred gameplay; not recovered retail rules',quests=profiles,camps=camps,npcs=fixed,
                templates={str(d):{'visual':templates[d]['visual'],'name':templates[d]['name']} for d in entities+[n['entity'] for n in fixed]},
                excluded_quests=excluded,sources={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in [CONTENT/'reference-tables.json',CONTENT/'native-quests.json',Path(templates_path)]})
    output.write_text(json.dumps(result,indent=2),encoding='utf8')
    (CONTENT/'inferred-creature-entities.txt').write_text(''.join(f'{d:08x}\n' for d in entities))
    fields=[('AI_PerceptionRadius',True),('AI_HomesickRadius',True),('AI_Wandering',False),
            ('AI_WanderingRange',True),('AI_WanderingProb',True),('AI_LoveTable',False),
            ('AI_FreeAttacking',False),('AI_IdleOnly',False),('NPC_DeathLootProfile',False),('NPC_IsLootProof',False)]
    ai=['pyrealium-creature-ai-v1']
    for did in entities:
        values=[]
        for key,is_float in fields:
            value=templates[did]['props'].get(key)
            if value is not None and is_float:
                value=struct.unpack('<f',struct.pack('<I',value))[0]
            values.append('-' if value is None else str(value))
        ai.append(f'{did:08x}\t'+'\t'.join(values))
    (CONTENT/'inferred-creature-ai-v1.tsv').write_text('\n'.join(ai)+'\n')
    print(f'Authored {len(profiles)} quest profiles, {len(camps)} camp candidates, {len(fixed)} NPC candidates, {len(entities)} creature templates')

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--templates',type=Path,default=ROOT/'Saved/current-native-templates.json')
    p.add_argument('--wiki',type=Path,default=ROOT/'Saved/ContentRecovery/wiki-content.json')
    p.add_argument('--output',type=Path,default=CONTENT/'inferred-content-v1.json')
    a=p.parse_args();author(a.templates,a.wiki,a.output)
