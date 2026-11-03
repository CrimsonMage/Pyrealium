"""Pin ten wiki NPC hunting quests to native creature/NPC identities.

The recovered admin quest export lacks these later NPC quest IDs. Use explicit
Pyrealium IDs, never substitute the similarly named auto-bestowed raid quests. V1 is
immutable once journals use it. Rewards follow the guide percentage/cap and
Pyreal level XP table; dialogue is a summary, not claimed recovered NPC speech.
"""
import json
from collections import defaultdict
from author_recovery_content import ROOT, CONTENT, load
from author_regional_population import digest


def build():
    tp = ROOT/'Saved/current-native-templates.json'
    wp = ROOT/'Saved/ContentRecovery/wiki-content.json'
    templates = {int(k):v for k,v in load(tp).items()}
    wiki = {a['title']:a for a in load(wp)['articles']}
    names = defaultdict(list)
    for did,t in templates.items(): names[t['name'].split('[')[0]].append(did)
    # title, speaker, target, level, XP basis points, XP cap, prerequisite index
    specs = [
        ('Fracture the Bone Golems','Vreema','Bone Golem',55,400,59676035,0),
        ('Fracture the Rime Golems','Vreema','Rime Golem',55,400,84627779,1),
        ('Fracture the Uncharneled Golems','Vreema','Uncharneled Golem',55,400,119705147,2),
        ('Exterminate the Olthoi Harvesters','Gregor Adulen','Olthoi Harvester',50,300,10787851,0),
        ('Rout the Deranged Lashers','Nola Vontag','Deranged Lasher',50,300,22085504,0),
        ('Vaporize the Frost Wisps','Tankaru','Frost Wisp',55,250,88254343,0),
        ('Crush the Cursed Bones','Brelar Marlage','Cursed Bones',48,400,5829950,0),
        ('Crush the Skeleton Lieutenants','Illiana Bain','Skeleton Lieutenant',50,400,10014692,0),
        ('Destroy the Ravenous Lashers','Shurakhe','Ravenous Lasher',50,300,15455108,0),
        ('Smash the Coral Golems','Endaren Brightstar','Coral Golem',55,300,44757025,0),
    ]
    quests=[]; used=set()
    for i,(title,speaker,target,level,bps,cap,prerequisite) in enumerate(specs,1):
        article=wiki[title]
        assert speaker in article['text'] and '10' in article['text'] and '7 days' in article['text']
        native_name={'Endaren Brightstar':'Endaraen Brightstar'}.get(speaker,speaker)
        npc=[d for d in names[native_name] if templates[d]['props'].get('AI_NPCTable')]
        assert len(npc)==1,(speaker,npc)
        targets=[d for d in names[target] if templates[d]['role']=='monster' and any(
            'IncrementQuest' in str(v) for k,v in templates[d]['props'].items() if 'DeathEffect' in k)]
        assert targets,(title,target)
        region='Arramora' if i<=6 else 'Knorr Foundry'
        text=f'Defeat 10 {target} creatures in {region}, then return to {speaker} for your reward.'
        if i==4:text+=' Look west and south of the Arramora lifestone.'
        if i==5:text+=' Look in eastern Arramora.'
        if i in (1,2,3,6):text+=' Search the central plateau.'
        quests.append(dict(id=0x4000fc00+i,title=title,npc=npc[0],npc_name=speaker,
            targets=sorted(targets),goal=10,level=level,xp_basis_points=bps,xp_cap=cap,
            prerequisite=0x4000fc00+prerequisite if prerequisite else 0,
            limit_seconds=604800,repeat_seconds=583200 if i<=6 else 604800,text=text,
            evidence=dict(url='https://20oz.net/index.php/'+title.replace(' ','_')+'.html',
                html_sha256=article['html_sha256'],native_npc_table=templates[npc[0]]['props']['AI_NPCTable'],
                native_death_effects={str(d):{k:v for k,v in templates[d]['props'].items() if 'DeathEffect' in k} for d in targets})))
        used.add(npc[0])
    # Other speakers already have recovered world placements. These two are
    # inferred near their documented Jethric's Outpost hub, then native-grounded.
    npcs=[]
    for title,north,east in [('Shurakhe',-32.05,52.05),('Endaren Brightstar',-32.12,52.02)]:
        entity=next(q['npc'] for q in quests if q['npc_name']==title)
        npcs.append(dict(id=0x8140d20000000000+entity,entity=entity,
            land=[1020+east*10.2,1020+north*10.2],
            evidence='Wiki Jethric Outpost 32.0S 52.0E; inferred town offset, native search <=32m.'))
    return dict(version=1,policy='Authored NPC hunt reconstruction; immutable qh1/qk1 journal rules, not recovered retail scripts or quest IDs.',
        quests=quests,npcs=npcs,camps=[],templates={str(d):dict(name=templates[d]['name'],visual=templates[d]['visual']) for d in used},
        sources={p.name:digest(p) for p in [tp,wp,CONTENT/'native-quests.json']})


if __name__=='__main__':
    output=CONTENT/'hunting-content-v1.json'
    text=json.dumps(build(),indent=2)+'\n'
    if output.exists() and output.read_text(encoding='utf8')!=text:
        raise SystemExit('Released hunting policy differs: create a new version instead of changing journal semantics.')
    output.write_text(text,encoding='utf8')
    print('10 hunting quests; 8 NPC speakers; 2 inferred NPC placements')
