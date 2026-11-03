"""Versioned, inferred outdoor spawners from archived wiki rosters and maps.

Existing observations are immutable. Membership, levels and habitat descriptions
are evidence; envelopes, density and respawn counts are reconstruction policy.
Run regional_population afterwards: ungrounded candidates never enter the world.
"""
import json
import math
import re
import struct
from collections import Counter, defaultdict
from pathlib import Path

from author_recovery_content import ROOT, CONTENT, load, norm
from author_regional_population import digest, hull, inside, seed, PEACEFUL

PREFIX = 'wilderness'
NAMESPACE = 0x8140C10000000000
MAP = ROOT / 'Saved/ServerReference/extracted/595afb35dbea396a/AC2/ac2wiki.info/images/0/06/MAPKnorrPortals.png'
ARR_MAP = MAP.parents[2] / '2/21/MAPArramora.png'
ALIASES = {'Silver Colossi': 'Silver Colossus', 'Mu-miyah Chieftain': 'Mu-miyah Chieftan',
           'Skeleton Cataphrac': 'Skeleton Cataphract', 'Clairvoyant Tizagn': 'Clairvoyant Tizegn',
           'Kresu Takeru': 'Cresu Takeru', 'Swarm Enslaved Soldier': 'Enslaved Soldier'}
# These named/rare/structure encounters need their own placement and lifecycle.
# A quest completion death effect alone does NOT make a common monster unique.
SPECIAL = {'Shattered Golem', 'Pearly Lumna Niffis', 'Staring Floating Spore',
           'Traumatized Rabid Mosswart', 'Diabolical Thornling Punisher',
           'Seething Olthoi Soldier Grub', 'Seasoned Dune Shark', 'Miser Chelak',
           'Stoneback', 'Irontooth', 'Bendit Stelzor', 'Skyport Attendant', 'Skyport Controller',
           'Olthoi Soldier Grub'}  # mound-driven static grub script; native wander probability is 5.0
# The first terrain audit rejected every Keerik candidate. Reserve more search
# attempts inside its documented peninsula, without relaxing ground/separation.
COVERAGE_ATTEMPTS = {'Burun Keerik': 12}


def compass(north, east):
    return [1020 + east * 10.2, 1020 + north * 10.2]


def map_point(x, y):
    # Calibrated against all four labelled portal coordinates on the map.
    # A (275,101)=(31.0S,34.9E); B (471,111)=(32S,52E);
    # C (397,189)=(38.7S,45.4E); D (455,322)=(50.3S,50.5E).
    return compass(-31.0 - (y - 101) / 11.45, 34.9 + (x - 275) / 11.45)


def roster(article):
    """Support prose lists, split link/level lines, and Foundry stats tables."""
    lines = article['text'].splitlines()
    start = next((i for i, l in enumerate(lines) if l in ('Monsters', 'Creatures')), None)
    if start is None:
        return []
    rows = []
    previous = ''
    for line in lines[start + 1:]:
        if line in ('Maps', 'Quests', 'NPCs', 'Points of Interest', 'Unique Creatures'):
            break
        match = re.fullmatch(r'(.+),\s*(\d+)\s*(group)?\s*(rare)?', line.strip())
        if match:
            rows.append((match[1], int(match[2]), bool(match[3]), bool(match[4])))
        else:
            match = re.fullmatch(r'-?\s*Lv\s+(\d+)(?:\s*-\s*(solo|group))?', line.strip())
            if match and previous:
                rows.append((previous, int(match[1]), match[2] == 'group', False))
        previous = line.strip()
    return list(dict.fromkeys(rows))


def habitat(region, creature, p):
    """Conservative subregions from the guide prose, not recovered generators."""
    north, east = (p[1] - 1020) / 10.2, (p[0] - 1020) / 10.2
    name, elite = creature['name'], creature['elite']
    if region == 'Knorr Wilds':
        if 'Banderling' in name:
            return east > 35 and north < -29
        if 'Monouga' in name:
            return 36 < east < 42 and -33 < north < -27
        return (east < 31 or east > 44) if elite else 30 < east < 44
    if region == 'Knorr Foundry':
        if 'Mu-miyah' in name and elite:
            return east > 62
        if 'Golem' in name or 'Skeleton Lancer' in name or 'Skeleton Dragoon' in name or 'Cataphrac' in name:
            return east > (56 if elite else 60)
        if elite:
            return east > 56 or north > -29
        return east < 57 or (57 < east < 60 and -35 < north < -31)
    if region == 'Knorr Gardens':
        if 'Scraven' in name:
            return north > -47 and (east > 56 if elite else east < 56)
        if 'Moarsman' in name or 'Takeru' in name:
            return north < -55 and (east < 52 if elite else True)
        if any(s in name for s in ('Divinator', 'Exemplar', 'Lector', 'Clairvoyant', 'Spirit of Vengeance')):
            return north < -51 and east > 54
        return east > 55 if elite else east < 56
    if region == 'Knorr Desolation':
        return north < -61 if creature['level'] >= 80 else north > -66
    if region == 'Arramora':
        if name == 'Olthoi Harvester':
            return north < 14 and east < -49  # hunting quest's more specific habitat
        if name == 'Deranged Lasher':
            return east > -46
        if name == 'Frost Wisp':
            return 16 < north < 23 and -52 < east < -45
        if 'Thornling' in name or 'Crone' in name or 'Reaper' in name:
            return north < 14
        if 'Golem' in name or 'Dark Mu-miyah' in name or 'Vigilant' in name:
            return 16 < north < 23 and -52 < east < -45
        if 'Mosswart' in name:
            return east < -52 or north > 23
        if 'Niffis' in name:
            return north > 19 and east > -47
        if 'Olthoi' in name:
            return north > 22
        if any(s in name for s in ('Lurker', 'Reaver', 'Shark', 'Armoredillo')):
            return east > -46
        if 'Fanatic' in name:
            return False  # kingdom encounters, not the island wilderness
        return north > 17 if creature['level'] >= 57 else north < 20
    return True


def build():
    templates_path = ROOT / 'Saved/current-native-templates.json'
    wiki_path = ROOT / 'Saved/ContentRecovery/wiki-content.json'
    templates = {int(k): v for k, v in load(templates_path).items()}
    wiki = {a['title']: a for a in load(wiki_path)['articles']}
    original = load(CONTENT / 'regional-content-v1.json')
    protected = {int(l, 16) for l in (CONTENT / 'population-protected.txt').read_text().splitlines()
                 if re.fullmatch('[0-9a-fA-F]{8}', l)}
    existing = ROOT / 'data/world/legions-world.tsv'
    counts = Counter()
    for line in existing.read_text(encoding='utf8').splitlines():
        f = line.split('\t')
        if len(f) == 13 and not f[0].startswith('8140c10000'):
            counts[int(f[1], 16)] += 1
    names = defaultdict(list)
    for did, t in templates.items():
        names[norm(t['name'].split('[')[0])].append(did)
    regions = [dict(name=r['name'], polygon=r['polygon'], center=r['center'],
                    source=r['source'], budget=55) for r in original['regions']]
    shapes = {
        'Knorr Wilds': [(148, 26), (435, 26), (438, 163), (199, 169), (145, 100)],
        'Knorr Foundry': [(450, 30), (680, 30), (680, 170), (448, 170)],
        'Knorr Gardens': [(432, 178), (615, 178), (620, 520), (395, 550), (405, 400)],
        'Knorr Ramparts': [(120, 172), (420, 172), (412, 350), (195, 350)],
        'Knorr Desolation': [(195, 365), (395, 365), (350, 488), (210, 593), (75, 595), (80, 442)],
    }
    for name, pixels in shapes.items():
        polygon = hull([map_point(*p) for p in pixels])
        regions.append(dict(name=name, polygon=polygon,
            center=[sum(p[i] for p in polygon) / len(polygon) for i in range(2)], budget=300,
            source=dict(map=MAP.relative_to(ROOT).as_posix(), map_sha256=digest(MAP),
                        pixel_envelope=pixels, basis='Inferred envelope inside the archived labelled Knorr portal map; native terrain must validate every point.')))
    polygon = hull([compass(*p) for p in [(10, -52), (11.6, -52.5), (16, -55),
        (24.6, -51.9), (23, -44.5), (20.1, -42.7), (14, -43)]])
    regions.append(dict(name='Arramora', polygon=polygon, center=compass(18, -49), budget=350,
        source=dict(map=ARR_MAP.relative_to(ROOT).as_posix(), map_sha256=digest(ARR_MAP),
                    basis='Convex envelope of wiki mainland camps, excluding kingdom islands; family habitat restrictions follow camp coordinates.')))
    towns = original['town_exclusions'] + [dict(name=n, land=compass(a, b), radius_m=220)
        for n, a, b in [('Skyport approach', -31, 34.9), ('Jethric Outpost', -32, 52),
                       ('Greenstone', -49.7, 50.5), ('Aerie', -38.7, 45.4),
                       ('Fort Strathelar', -51.7, 39.1), ('Arramora landing', 11.9, -49.1)]]
    excluded, camps, covered = [], [], set()
    for index, region in enumerate(regions):
        title = region['name']; article = wiki[title]
        region['source'].update(wiki_url='https://20oz.net/index.php/' + title.replace(' ', '_') + '.html',
                                html_sha256=article['html_sha256'])
        creatures = []
        for name, level, elite, rare in roster(article):
            candidates = []
            reasons = set()
            for did in names[norm(ALIASES.get(name, name))]:
                t = templates[did]; p = t['props']
                if (did in protected or t['role'] != 'monster' or not t['visual']
                    or p.get('AI_LoveTable', 0) in PEACEFUL or p.get('AI_IdleOnly', 0)
                    or p.get('NPC_IsLootProof', 0) or p.get('GameplayStatistics_MaxHealth', 0) <= 0
                    or p.get('AICombat_NaturalDamage', 0) <= 0):
                    reasons.add('Not an ordinary hostile, lootable native monster'); continue
                if p.get('GameplayStatistics_Level') != level:
                    reasons.add('Guide/native level disagreement'); continue
                if rare or name in SPECIAL or ',' in name or 'Fanatic' in name or any('Gauntlet' in str(v) for k,v in p.items() if 'DeathEffect' in k):
                    reasons.add('Named, rare, structure or scripted encounter needs separate placement'); continue
                candidates.append(did)
            if not candidates:
                excluded.append(dict(region=title, name=name, level=level,
                                     reasons=sorted(reasons) or ['No matching installed native template']))
                continue
            did = min(candidates)
            creatures.append(dict(entity=did, name=templates[did]['name'].split('[')[0], level=level,
                                  elite=elite or bool(templates[did]['props'].get('NPC_IsGroupMonster', 0)),
                                  variants=sorted(candidates), existing_placements=counts[did]))
        region['creatures'] = creatures
        points = []
        # Stable grid offset from v1; budget is regional, never density near a player.
        for bx in range(255):
            for by in range(255):
                h = seed(f'wilderness-v1:{bx}:{by}')
                p = [bx*8 + .75 + (h & 65535)/65535*6.5,
                     by*8 + .75 + ((h >> 16) & 65535)/65535*6.5]
                if not inside(p, region['polygon']): continue
                owners = [r for r in regions if inside(p, r['polygon'])]
                if min(owners, key=lambda r: math.dist(p, r['center'])) is not region: continue
                if any(math.dist(p, t['land'])*20 < t['radius_m'] + 35 for t in towns): continue
                points.append((h, bx, by, p))
        points.sort()
        used = Counter()
        for h, bx, by, p in points:
            if sum(used.values()) >= region['budget']: break
            choices = [c for c in creatures if habitat(title, c, p)]
            if not choices: continue
            # First give every eligible type several attempts, then balance the
            # roster. Prefer types with no existing population on the first lap.
            c = min(choices, key=lambda c: (used[c['entity']] / (4 if not c['existing_placements'] else 1),
                bool(c['existing_placements']), seed(f"{h}:{c['entity']}")))
            did = c['entity']; used[did] += 1; covered.add(did)
            count = 1 if c['elite'] else 2
            camps.append(dict(id=NAMESPACE | (bx << 16) | (by << 8), land=p,
                region=title, roster=[did]*count, capacity=count, regeneration_ms=180000,
                polygon=region['polygon']))
        occupied = {c['id'] for c in camps}
        for c in creatures:
            minimum = COVERAGE_ATTEMPTS.get(c['name'], 0)
            if c['existing_placements'] or used[c['entity']] >= minimum: continue
            for h, bx, by, p in points:
                identity = NAMESPACE | (bx << 16) | (by << 8)
                if identity in occupied or not habitat(title,c,p): continue
                camps.append(dict(id=identity,land=p,region=title,roster=[c['entity']]*2,
                    capacity=2,regeneration_ms=180000,polygon=region['polygon']))
                occupied.add(identity);covered.add(c['entity']);used[c['entity']]+=1
                if used[c['entity']] >= minimum: break
    entities = sorted(covered)
    result = dict(version=1, namespace=NAMESPACE, policy='Inferred wiki wilderness expansion; not exact retail generators.',
        regions=regions, town_exclusions=towns, camps=camps, npcs=[], excluded=excluded,
        templates={str(d): dict(name=templates[d]['name'], visual=templates[d]['visual']) for d in entities},
        sources={p.name: digest(p) for p in [templates_path, wiki_path, CONTENT/'regional-content-v1.json']})
    (CONTENT/'wilderness-content-v1.json').write_text(json.dumps(result, indent=2)+'\n', encoding='utf8')
    (CONTENT/'wilderness-creature-entities.txt').write_text(''.join(f'{d:08x}\n' for d in entities))
    fields = [('AI_PerceptionRadius', True), ('AI_HomesickRadius', True), ('AI_Wandering', False),
              ('AI_WanderingRange', True), ('AI_WanderingProb', True), ('AI_LoveTable', False),
              ('AI_FreeAttacking', False), ('AI_IdleOnly', False), ('NPC_DeathLootProfile', False), ('NPC_IsLootProof', False)]
    ai = ['pyrealium-creature-ai-v1']
    for did in entities:
        values = []
        for key, floating in fields:
            value = templates[did]['props'].get(key)
            if value is not None and floating: value = struct.unpack('<f', struct.pack('<I', value))[0]
            values.append('-' if value is None else str(value))
        ai.append(f'{did:08x}\t'+'\t'.join(values))
    (CONTENT/'wilderness-creature-ai-v1.tsv').write_text('\n'.join(ai)+'\n')
    print(f'{len(regions)} regions, {len(camps)} candidates, {sum(c["capacity"] for c in camps)} slots, {len(entities)} native monster types, {len(excluded)} withheld roster entries')
    for r in regions: print(r['name'], len(r['creatures']), sum(c['region']==r['name'] for c in camps))


if __name__ == '__main__':
    build()
