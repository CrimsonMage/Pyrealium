"""Immutable second hunt pack: guide rules and exact native quest death effects.

No retail quest IDs or missing NPC dialogue are claimed recovered. Coordinates
are guide anchors, then native-grounded; group sizes and timers are inferred.
"""
import json
from author_recovery_content import ROOT, CONTENT, load
from author_regional_population import digest


def build():
    tp = ROOT/'Saved/current-native-templates.json'
    wp = ROOT/'Saved/ContentRecovery/wiki-content.json'
    templates = {int(k): v for k, v in load(tp).items()}
    wiki = {a['title']: a for a in load(wp)['articles']}
    # title, native speaker, exact death-effect suffix, level, XP basis points
    # (zero means fixed XP), cap/fixed XP, prerequisite low byte, habitat guidance.
    specs = [
        ('Crush the Skeleton Lancers','Moira Len','_KQ2G_SkeletonLancer',55,250,52892362,0,
         "Follow the Foundry coast east from Jethric's Outpost."),
        ('Destroy the Ruthless Waraths','Urlin Furst','_KQ2E_RuthlessWarath',50,300,31480235,0,
         "Search southeast of Jethric's Outpost."),
        ('Shred the Mu-miyah Warriors','Furakhe','_KQ2C_MumiyahWarrior',50,300,10787851,0,
         "Search southeast of Jethric's Outpost and in Vothardun."),
        ('Dispatch the Anguish Crones','Brother Groli Vontag','LinC1_AnguishCrone',50,300,15455108,0,
         'Search west of the Arramora lifestone.'),
        ('Dispatch the Thornling Plunderers','Brother Groli Vontag','LinC2_ThornlingPlunderer',50,300,15455108,14,
         'Search west of the Arramora lifestone.'),
        ('Dispatch the Shock Reapers','Brother Groli Vontag','LinC3_ShockReaper',50,300,31480235,15,
         'Search west of the Arramora lifestone.'),
        ('Exterminate the Olthoi Worker Nymphs','Gregor Adulen','LinA2_OlthoiNymphWorker',55,250,74815717,4,
         'Search west and southwest of the Northern Arramora Gateway, including the camp at 24.6N 51.9W.'),
        ('Exterminate the Olthoi Drones','Gregor Adulen','LinA3_OlthoiDrone',55,200,82111840,17,
         'Search west and southwest of the Northern Arramora Gateway and near the Burun Idol.'),
        ('Perforate the Spawn','Audalon','LinE_VigilantSpawn',50,300,22085504,0,
         'Search the Arramora central plateau, including the camp at 17.2N 52.2W.'),
        ('Puncture the Lumna Niffis','Eliza Dumach','LinG1_LumnaNiffis',55,400,59676033,0,
         'Search the Arramora desert. These are group creatures.'),
        ('Puncture the Veinu Niffis','Eliza Dumach','LinG2_VeinuNiffis',55,400,141206944,20,
         'Search the Arramora desert. These are group creatures.'),
        ('Unravel the Dark Dread Mu-miyah','Red Bellows','LinH_DarkDreadMumiyah',55,250,52892362,0,
         'Search the Arramora central plateau and the camp at 16.7N 47.7W.'),
        ('Pluck the Stray Darkenfowl','Karin Dirae','IkA_StrayDarkenfowl',17,0,3000,0,
         'Look for Darkenfowl Strays in western Kydi.'),
        ('Perforate the Servitor Spawn','Sumalar','IkB1_ServitorSpawn',18,0,4300,0,
         'Look for Servitor Spawn in western Kydi.'),
        ('Perforate the Puppet Spawn','Sumalar','IkB2_PuppetSpawn',20,0,7500,24,
         'Look for Puppet Spawn throughout Ariaki River Basin.'),
        ('Perforate the Surrogate Spawn','Sumalar','IkB3_SurrogateSpawn',22,0,12000,25,
         'Look for Surrogate Spawn throughout Lumari Spill Isles.'),
        ('Drive out the Kachem Mimbu','Pim Donar','IkC_MimbuKachem',21,0,8000,0,
         'Search southwest of Mayoi and between the West and Northwest Ariaki Outposts.'),
        ('Deflate the Parfal Nefanes','Baliga','IkD1_ParfalNefane',23,0,13000,0,
         'Search between the West and Northwest Ariaki Outposts and in northern Lumari.'),
        ('Deflate the Listris Niffis','Baliga','IkD2_ListrisNiffis',22,0,18000,28,
         'Search Lumari Spill Isles near Shinoko.'),
        ('Eliminate the Ulu Sclavus','Sister Nell Palnos','IkE1_UluSclavus',23,0,22000,0,
         'Search southwest of Zu and northeastern Lumari.'),
        ('Eliminate the Faisi Sclavus','Sister Nell Palnos','IkE2_FaisiSclavus',23,0,30000,30,
         'Faisi Sclavus west of Northwest Tou-Tou and Seeker Faisi Sclavus in southern Tou-Tou both count.'),
        ('Thin the Marsh Armoredillos','Baltor Vultu','IkF_MarshArmoredillo',27,0,44000,0,
         'Search northwest and southeast Naderu and north of the Tou-Tou Vault.'),
        ('Puncture the Opor Niffis','Laurikhe','IkH1_OporNiffis',28,0,45000,0,
         'Search around the Northwest Naderu Outpost.'),
        ('Puncture the Opor Nefanes','Laurikhe','IkH2_OporNefane',28,0,82000,33,
         'Search around the Northeast Naderu and Northeast Tou-Tou Outposts.'),
        ('Cull the Blighted Mucor','Ingara','IkG_BlightedMucor',29,0,55000,0,
         'Search northeast Tou-Tou, southeast Naderu and the Naderu ringway.'),
    ]
    quests = []
    for i, (title, speaker, suffix, level, bps, xp, prerequisite, hint) in enumerate(specs, 11):
        a = wiki[title]; facts = a['facts']
        effect = 'Eff_IncrementQuest'+suffix
        npcs = [d for d,t in templates.items() if t['name']==speaker and t['props'].get('AI_NPCTable')]
        assert len(npcs)==1, (title, npcs)
        # Select identities by the actual quest counter effect, not a fuzzy
        # name. This admits the Seeker variant but excludes Gauntlet clones.
        targets = sorted(d for d,t in templates.items() if t['role']=='monster' and any(
            v==effect for k,v in t['props'].items() if k.startswith('NPC_DeathEffect')))
        assert targets, (title, effect)
        assert '10' in a['text'] and str(level) in facts['Requirements']
        assert (speaker if speaker!='Laurikhe' else 'Lauri') in facts['Quest Starter']
        reward = facts.get('Rewards', facts.get('XP Reward'))
        assert f'{xp:,}' in reward, (title, reward)
        if bps: assert f'{bps/100:g}%' in reward
        limit = {'7 days':604800, 'No time limit':0}[facts['Time Limit']]
        repeat = {'7 days':604800, '6 days, 18 hours':583200, '20 hours':72000}[facts['Repeat Time']]
        quests.append(dict(id=0x4000fc00+i, title=title, npc=npcs[0], npc_name=speaker,
            targets=targets, goal=10, level=level, xp_basis_points=bps, xp_cap=xp,
            prerequisite=0x4000fc00+prerequisite if prerequisite else 0,
            limit_seconds=limit, repeat_seconds=repeat,
            text=f'{title}: defeat 10 qualifying creatures, then return to {speaker}. {hint}',
            evidence=dict(url='https://www.20oz.net/index.php/'+title.replace(' ','_')+'.html',
                html_sha256=a['html_sha256'], facts=facts,
                native_npc_table=templates[npcs[0]]['props']['AI_NPCTable'], native_death_effect=effect)))
    # Furakhe and Ingara lack compatible recovered placements. Town offsets
    # are inferred; other speakers retain their existing observed positions.
    npcs = []
    for name,north,east,source in [('Furakhe',-32.08,52.10,"Jethric's Outpost"),
                                   ('Ingara',10.2,62.4,'Ikeras')]:
        entity = next(q['npc'] for q in quests if q['npc_name']==name)
        npcs.append(dict(id=0x8140d30000000000+entity,entity=entity,
            land=[1020+east*10.2,1020+north*10.2],evidence=f'{source}: inferred town offset, native search <=32m.'))
    camps=[]
    for i,(title,name,north,east) in enumerate([
        ('Perforate the Spawn','Vigilant Spawn',17.2,-52.2),
        ('Exterminate the Olthoi Worker Nymphs','Olthoi Worker Nymph',24.6,-51.9),
        ('Unravel the Dark Dread Mu-miyah','Dread Dark Mu-miyah',16.7,-47.7)],1):
        q=next(q for q in quests if q['title']==title)
        entity=next(d for d in q['targets'] if templates[d]['name']==name)
        camps.append(dict(id=0x8140d30000010000+i*16,land=[1020+east*10.2,1020+north*10.2],
            roster=[entity,entity],capacity=2,evidence=dict(url=q['evidence']['url'],
            coordinates=f'{north}N {abs(east)}W',policy='Guide anchor; inferred two-child group, 120 second refill after death, native search <=32m.')))
    used={q['npc'] for q in quests}|{d for q in quests for d in q['targets']}
    return dict(version=2,policy='Authored qh2/qk2 expansion. V1 stays immutable; fixed XP uses zero basis points, zero deadline means unlimited.',
        quests=quests,npcs=npcs,camps=camps,
        templates={str(d):dict(name=templates[d]['name'],visual=templates[d]['visual']) for d in sorted(used)},
        sources={p.name:digest(p) for p in [tp,wp,CONTENT/'hunting-content-v1.json']})


if __name__=='__main__':
    output=CONTENT/'hunting-content-v2.json'
    data=build(); text=json.dumps(data,indent=2)+'\n'
    if output.exists() and output.read_text(encoding='utf8')!=text:
        raise SystemExit('Released v2 hunt policy differs; version new journal rules instead of rewriting it.')
    output.write_text(text,encoding='utf8')
    print(f'{len(data["quests"])} new hunts, {len(data["npcs"])} NPCs, {len(data["camps"])} guide camps')
