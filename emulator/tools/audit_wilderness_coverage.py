"""Account for every native combat template without inventing its habitat.

The report distinguishes source identities from displayed species names and
native records from executable encounter/quest policies. No content is enabled
by this audit. Regenerate after prepare_content.py updates the combined world.
"""
import json
from collections import Counter, defaultdict
from author_recovery_content import ROOT, CONTENT, load, norm
from author_regional_population import digest, PEACEFUL
from author_wilderness_expansion import NAMESPACE, SPECIAL


def build():
    template_path = ROOT/'Saved/current-native-templates.json'
    wiki_path = ROOT/'Saved/ContentRecovery/wiki-content.json'
    world_path = ROOT/'data/world/legions-world.tsv'
    templates = {int(k):v for k,v in load(template_path).items()}
    wiki = load(wiki_path)['articles']
    source = load(CONTENT/'wilderness-content-v1.json')
    proof = load(CONTENT/'wilderness-world-v1.report.json')
    rows = [s.split('\t') for s in world_path.read_text(encoding='utf8').splitlines() if len(s.split('\t'))==13]
    counts = Counter(int(r[1],16) for r in rows)
    added = Counter(int(r[1],16) for r in rows if int(r[0],16) & 0xffffffffff000000 == NAMESPACE)
    # Report the original wilderness expansion against its earlier baseline,
    # not against later guide camps which reuse three of its newly added types.
    later_ids = {s.split('\t')[0] for s in (CONTENT/'hunting-world-v2.tsv').read_text().splitlines()
                 if len(s.split('\t'))==13}
    later = Counter(int(r[1],16) for r in rows if r[0] in later_ids)
    baseline = counts-added-later
    protected = {int(s,16) for s in (CONTENT/'population-protected.txt').read_text().splitlines()
                 if len(s)==8 and all(c in '0123456789abcdefABCDEF' for c in s)}
    by_name = defaultdict(list)
    for did,t in templates.items(): by_name[norm(t['name'].split('[')[0])].append(did)
    habitats = defaultdict(set)
    for r in source['regions']:
        for c in r['creatures']:
            for did in c['variants']: habitats[did].add(r['name'])
    combat = {}
    for did,t in templates.items():
        p=t['props']; name=t['name'].split('[')[0]
        if p.get('GameplayStatistics_MaxHealth',0)<=0 or p.get('AICombat_NaturalDamage',0)<=0: continue
        # Health/damage fields alone are not proof that an NPC is an enemy.
        if did in protected or t['role']!='monster' or p.get('AI_LoveTable',0) in PEACEFUL:
            status='protected_or_non_hostile'
        elif counts[did]: status='placed'
        elif any(counts[d] for d in by_name[norm(name)]): status='alternate_template_of_placed_name'
        elif habitats[did]: status='eligible_habitat_without_accepted_ground'
        elif name in SPECIAL or p.get('AI_IdleOnly',0) or p.get('NPC_IsLootProof',0):
            status='special_encounter_needs_lifecycle'
        else: status='needs_habitat_or_encounter_evidence'
        combat[f'{did:08x}']=dict(name=t['name'],status=status,role=t['role'],
            placements=counts[did],new_placements=added[did],wiki_regions=sorted(habitats[did]))
    quest_records=[]
    for q in [q for version in (1,2) for q in load(CONTENT/f'hunting-content-v{version}.json')['quests']]:
        quest_records.append(dict(title=q['title'],id=f'{q["id"]:08x}',npc=q['npc_name'],
            npc_entity=f'{q["npc"]:08x}',npc_placements=counts[q['npc']],
            target_placements={f'{d:08x}':counts[d] for d in q['targets']},
            source=q['evidence']['url'],flow='accept, ten qualifying kills, return, capped reward, repeat timer'))
    # Quest starter inventory is a follow-up queue, NOT an assertion that a
    # placed NPC's quest script is implemented. Some starters are items/areas.
    starters=[]
    for article in wiki:
        starter=article['facts'].get('Quest Starter')
        if not starter: continue
        key=norm(starter.split(',')[0].strip())
        candidates=by_name.get(key,[])
        starters.append(dict(quest=article['title'],starter=starter,
            native_entities=[f'{d:08x}' for d in candidates],
            placements=sum(counts[d] for d in candidates),
            implemented_in_this_hunting_pack=any(q['title']==article['title'] for q in quest_records),
            url=article['url'],html_sha256=article['html_sha256']))
    memberships={i:g['region'] for g in proof['groups'] for i in g['members']}
    regional=[]
    for r in source['regions']:
        members=[row for row in rows if memberships.get(int(row[0],16))==r['name']]
        regional.append(dict(name=r['name'],added_creatures=len(members),
            added_groups=sum(g['region']==r['name'] for g in proof['groups']),
            eligible_types=len(r['creatures']),
            roster_types_present_anywhere=sum(any(counts[d] for d in c['variants']) for c in r['creatures']),
            unplaced_types=[c['name'] for c in r['creatures'] if not any(counts[d] for d in c['variants'])],
            source=r['source']))
    return dict(version=1,limitations=[
        'Map envelopes, camp positions, capacities and regeneration are inferred; not recovered retail generators.',
        'A placed template/name is not a claim that every variant, level, dungeon, or habitat is covered.',
        'Quest starter matching is exact normalized native-name matching; unmatched starters may be items or areas.',
        'Native loot quantities/trophies and observed item pools are used; exact retail item weights and affix algorithms remain unrecovered.'],
        summary=dict(world_placements=len(rows),new_creatures=sum(added.values()),new_groups=len(proof['groups']),
            hunting_expansion_placements=sum(later.values()),implemented_npc_hunts=len(quest_records),
            new_outdoor_blocks=proof['newly_populated_outdoor_blocks'],new_native_types=sum(not baseline[d] for d in added),
            native_combat_templates=len(combat),template_status_counts=dict(Counter(v['status'] for v in combat.values())),
            eligible_wiki_types=len({c['entity'] for r in source['regions'] for c in r['creatures']}),
            eligible_types_without_any_placement=sorted({n for r in regional for n in r['unplaced_types']})),
        regions=regional,hunting_quests=quest_records,withheld_roster_entries=source['excluded'],
        native_combat_templates=combat,wiki_quest_starters=starters,
        sources={p.name:digest(p) for p in [template_path,wiki_path,world_path,
            CONTENT/'wilderness-content-v1.json',CONTENT/'wilderness-world-v1.report.json',CONTENT/'hunting-content-v1.json',
            CONTENT/'hunting-content-v2.json',CONTENT/'hunting-world-v2.report.json']})


if __name__=='__main__':
    report=build()
    (CONTENT/'wilderness-coverage-v1.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf8')
    print(json.dumps(report['summary'],indent=2))
