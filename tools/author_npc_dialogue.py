"""Build NPC replies and an exhaustive wiki identity/placement audit.

Replies are authored summaries, not recovered dialogue or executable wiki text.
Existing quest/shop handlers retain priority. Unimplemented tasks never grant
quests, items, XP, teleports or completion flags through this catalog.
"""
import collections
import hashlib
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CONTENT = ROOT / 'content'
CATEGORY_URL = 'https://20oz.net/index.php/Category%253ANPC.html'
# The live category has 89 entries. Seven archived pages lack the usual header.
CATEGORY_EXTRAS = ['Aun Aulatah', 'Brisin', 'Foreman', 'Justiciar Maronak',
                   'Naokim', 'Old Herrigar', 'Theoter']
# Explicitly reviewed spelling/title differences; never fuzzy-bind actors.
ALIASES = {'Sontar Risoya': 'Sontar Trisoya', 'Lauriche': 'Laurikhe',
           'Anaro, Warder Pioneer': 'Adaro, Warder Pioneer',
           'Anaro': 'Adaro, Warder Pioneer', 'Hand of Asheron': 'The Hand of Asheron',
           'Mistress of Shadows': 'The Mistress Of Shadows',
           'Servant of the Empire': 'The Servant Of The Empire',
           'Elder Scholar': 'An Elder Scholar', 'Sia Jorlon': 'Sia Jorlorn'}
COORD = re.compile(r'(\d+(?:\.\d+)?)\s*([NS])\s*,?\s*(\d+(?:\.\d+)?)\s*([EW])', re.I)
INTERIOR = re.compile(r'\b(inside|dungeon|vault|cave|burrow|antechamber|crypt|tomb|lair|gauntlet)\b', re.I)
OBJECTS = {'Postings', 'Company Postings', 'Cerulean Throne', 'Foundry Gates', "Warder's Hideout", 'Chaos Portal'}
# Reviewed quest participants omitted from the category and starter fields.
# These links provide information only; they do not implement item exchanges.
PARTICIPANTS = {'Feral Pet Lasher': ["Nature's Ally"]}
# The NPC page repeats Kanten's Cavendo coordinate while its town and quest
# walkthrough both place Lataua in Zu. No exact replacement point is known.
BAD_COORDINATES = {'Lataua': 'NPC coordinate is Cavendo; town and quest walkthrough specify Zu'}
QUEST_ALIASES = {'SHRETH': 'S.H.R.E.T.H.', 'SHRETH Base Ultra': 'S.H.R.E.T.H. Base Ultra'}
# Authored paraphrases of these quest pages, never recovered retail dialogue.
SUMMARIES = {
    'Old Cragstone Excavation': 'Drudges have displaced the Grutts at the excavation. The Warder wants the library blueprints. Three kinds of seal fragment are needed to enter the library.',
    "Nature's Ally": 'The natural spirits require several exchanges before you can reach the Chaos Dungeon and bring its crystal to the Great Deru Shrine.',
    'Anthracite Gem': 'This exchange follows the flowers, wasp eggs and truffle along a chain of natural spirits.',
    'Blood Feud': 'Bar Nobla has occupied a temple near Old Herrigar. Defeating him and reporting back ends this stage of the family feud.',
    'Mimbu Negotiations': 'Akkilea wants a peaceful visit to Sachem Mimbu and a report afterward. Attacking Mimbu breaks the agreement.',
    'The Path into Darkness': 'The Dark Agent seeks artifacts from the Order, Dominion and Shadow. Chaos Corruption is required before he will entrust the task to you.',
    'The Path into Light': 'Lataua offers a route away from Chaos Corruption. The journey involves defending three kingdoms against renegades.',
    'Gambling Debt': 'Kanten needs a bag delivered to Orven in Arwic to settle a debt.',
    'Journey to Greenstone': 'After the Foundry Expeditionary Forces assignments, Dell sends a knapsack to Captain Trammel in Greenstone.',
    'Journey to Fort Strathelar': 'After the Aerie assignments, Otonku needs a report carried to Legionary Brigadier Maekarn at Fort Strathelar.',
    'S.H.R.E.T.H. Base Ultra': 'Maronak is investigating S.H.R.E.T.H. activity. The trail starts with a scrap in Frozen Ravine and continues to the secret island.',
}


def norm(s):
    return re.sub(r'[^a-z0-9]', '', s.lower())


def words(s):
    return ' '.join(s.split())


def section(article, heading):
    lines = article['text'].splitlines()
    if heading not in lines:
        return []
    start = lines.index(heading) + 1
    end = next((i for i in range(start, len(lines)) if lines[i] in article['headings']), len(lines))
    return lines[start:end]


def source(article):
    return dict(title=article['title'], url='https://20oz.net/index.php/' +
                article['url'].split('/index.php/')[-1] + '.html',
                html_sha256=article['html_sha256'])


def coordinate(text):
    found = COORD.findall(text)
    if len(found) != 1:
        return None
    n, ns, e, ew = found[0]
    return [float(n) * (-1 if ns.upper() == 'S' else 1),
            float(e) * (-1 if ew.upper() == 'W' else 1)]


def starter_names(starter, candidates):
    """Keep separate starters while excluding shorter names inside a full name."""
    # A person in a place name (e.g. Dell in Jethric's Outpost) is not another
    # starter. Keep the original field for coordinate evidence elsewhere.
    starter = re.split(r'\s+(?:in|at|near|outside|inside)\s+', starter, maxsplit=1, flags=re.I)[0]
    matches = [(m.start(), m.end(), name) for name in candidates
               for m in re.finditer(r'(?<!\w)' + re.escape(name) + r'(?!\w)', starter, re.I)]
    selected = []
    for match in sorted(matches, key=lambda m: (m[0]-m[1], m[0], m[2])):
        if not any(match[0] < end and start < match[1] for start, end, _ in selected):
            selected.append(match)
    return sorted({name for _, _, name in selected})


def quest_topics(article):
    """Paginate complete facts; never truncate prerequisites or invent speech."""
    title, facts = article['title'], article['facts']
    if 'discontinued' in ' '.join(section(article, 'Quest Overview')).lower():
        return [dict(title=title, text=f'{title}: This is a discontinued quest. No task is offered.',
                     kind='discontinued', source=source(article))]
    parts = []
    if title in SUMMARIES:
        parts.append(SUMMARIES[title])
    for key, label in [('Quest Starter','Begins with'), ('Start Location','Starting area'),
                       ('Requirements','Requirements'), ('Intended Level','Suggested level'),
                       ('Time Limit','Time allowed'), ('Repeat Time','Repeat interval'),
                       ('XP Reward','Listed experience reward'), ('Other Rewards','Listed rewards')]:
        value = words(facts.get(key, '')).strip(' .:-')
        if value:
            parts.append(f'{label}: {value}.')
    prefix = f'{title}. '
    suffix = ' Information only; no task has been started.'
    pages, text = [], prefix
    for part in parts:
        if len((text+part+suffix).encode('utf-16-le'))//2 > 440:
            if text != prefix:
                pages.append(text.rstrip()+suffix); text=prefix
            # A large compound fact is retained in the audit/source, never
            # presented with a silently chopped off prerequisite or reward.
            if len((text+part+suffix).encode('utf-16-le'))//2 > 440:
                continue
        text += part+' '
    if text != prefix or not pages:
        pages.append(text.rstrip()+suffix)
    return [dict(title=title, text=text, kind='information_only', source=source(article)) for text in pages]


def build():
    wp = ROOT / 'Saved/ContentRecovery/wiki-content.json'
    tp = ROOT / 'Saved/current-native-templates.json'
    articles = json.loads(wp.read_text(encoding='utf8'))['articles']
    wiki = {a['title']: a for a in articles}
    # Mirror redirects were archived as separate articles. Keep one discussion
    # for identical quest bodies, with explicit authored summaries preferred.
    identical = collections.defaultdict(list)
    for a in articles:
        if a['facts'].get('Quest Starter'):
            identical[words(a['text'])].append(a['title'])
    quest_aliases = dict(QUEST_ALIASES)
    for titles in identical.values():
        canonical=min(titles,key=lambda t:(t not in SUMMARIES,t not in QUEST_ALIASES.values(),len(t),t))
        for title in titles:
            quest_aliases[title]=canonical
    native = {int(k): v for k, v in json.loads(tp.read_text(encoding='utf8')).items()}
    by_name = collections.defaultdict(list)
    for entity, template in native.items():
        by_name[norm(template['name'])].append(entity)
    category = sorted({a['title'] for a in articles if a['text'].startswith('Back to\nNPCs')} | set(CATEGORY_EXTRAS))
    assert len(category) == 89
    known = collections.defaultdict(dict)
    anchors = collections.defaultdict(list)
    related = collections.defaultdict(set)

    def register(name, article):
        name = words(name)
        if name in OBJECTS or len(name) < 3 or name.startswith((',', '(', ')', '-')) or not name[0].isalpha():
            return
        known[name][article['title']] = source(article)

    for name in category:
        known[name]['category'] = dict(title='Category:NPC', url=CATEGORY_URL)
        if name in wiki:
            a = wiki[name]; register(name, a)
            # Related-quest names are facts; never execute the page's prose.
            for title in section(a, 'Related Quests'):
                if title in wiki and wiki[title]['facts']:
                    related[name].add(title)
            loc = a['text'].split('Location', 1)[-1].split('Related Quests', 1)[0].split('Shop Inventory', 1)[0]
            if 'Location' in a['text'] and not INTERIOR.search(loc):
                if coord := coordinate(loc):
                    anchors[name].append(dict(coordinates=coord, source=source(a), basis='NPC article'))

    # The category omits many town NPCs, including entire hunting chains.
    for a in articles:
        if 'NPCs' not in a['headings']:
            continue
        previous = None
        for line in section(a, 'NPCs'):
            if line.startswith(','):
                if previous and not INTERIOR.search(line) and (coord := coordinate(line)):
                    anchors[previous].append(dict(coordinates=coord, source=source(a), basis='Town NPC listing'))
                continue
            # Reject layout fragments and dungeon names masquerading as entries.
            if line and not INTERIOR.search(line) and (norm(ALIASES.get(line, line)) in by_name or line in wiki):
                register(line, a); previous = line
            else:
                previous = None

    # Include named quest starters absent from the index/town lists. Select the
    # longest exact boundary match so 'Ancestor' cannot absorb a named ancestor.
    candidates = (set(known) | {v['name'] for v in native.values() if v['role'] in ('scripted', 'vendor') and v.get('ai')}) - OBJECTS
    for a in articles:
        starter = words(a['facts'].get('Quest Starter', ''))
        if not starter:
            continue
        matches = starter_names(starter, candidates)
        if not matches:
            continue
        for name in matches:
            register(name, a); related[name].add(a['title'])
            # Multiple starters cannot safely share an unlabeled coordinate.
            if len(matches) == 1 and not INTERIOR.search(starter) and (coord := coordinate(starter)):
                anchors[name].append(dict(coordinates=coord, source=source(a), basis='Quest starter coordinate'))
    for name, titles in PARTICIPANTS.items():
        for title in titles:
            register(name, wiki[title]); related[name].add(title)

    existing = collections.defaultdict(list)
    world = ROOT / 'data/world/legions-world.tsv'
    for line in world.read_text(encoding='utf8').splitlines():
        f = line.split('\t')
        if len(f) == 13 and f[0].startswith('8140') and not f[0].startswith('8140d400'):
            existing[int(f[1], 16)].append(f[0])
    existing_names = {norm(native[d]['name']) for d in existing if d in native}
    profiles, audit, placements = {}, [], []
    for name in sorted(known):
        identity = ALIASES.get(name, name)
        matches = by_name.get(norm(identity), [])
        safe = [d for d in matches if native[d]['role'] in ('scripted', 'vendor')]
        row = dict(name=name, category=name in category, native_name=identity, entities=safe,
                   excluded_combat_entities=[d for d in matches if d not in safe],
                   sources=list(known[name].values()), quests=sorted(related[name]),
                   existing_objects=[iid for d in safe for iid in existing[d]])
        row['status'] = 'resolved' if safe else ('combat-only identity' if matches else 'no native identity')
        audit.append(row)
        for d in safe:
            if d not in profiles:
                profiles[d] = dict(entity=d, name=native[d]['name'], visual=native[d]['visual'], native_table=native[d].get('ai'),
                                   native_base=native[d]['base'], role=native[d]['role'], topics=[], sources=[])
            p = profiles[d]; p['sources'].extend(s for s in row['sources'] if s not in p['sources'])
            for title in sorted({quest_aliases.get(t,t) for t in related[name]}):
                if any(topic['title'] == title for topic in p['topics']):
                    continue
                p['topics'].extend(quest_topics(wiki[title]))
        if not safe:
            continue
        points = anchors[name]
        row['coordinate_evidence'] = points
        if row['existing_objects'] or norm(identity) in existing_names:
            row['placement'] = 'existing identity retained'
        elif identity in BAD_COORDINATES:
            row['placement'] = BAD_COORDINATES[identity]+'; withheld'
        elif len(safe) != 1:
            row['placement'] = 'multiple native variants; placement unresolved'
        elif not points:
            row['placement'] = 'no unambiguous outdoor coordinate'
        elif any(abs(p['coordinates'][0]-points[0]['coordinates'][0]) > .3 or
                 abs(p['coordinates'][1]-points[0]['coordinates'][1]) > .3 for p in points):
            row['placement'] = 'conflicting wiki coordinates; withheld'
        else:
            d = safe[0]; north, east = points[0]['coordinates']
            # One identity globally, never one copy per wiki mention.
            if not any(n['entity'] == d for n in placements):
                placements.append(dict(id=0x8140d40000000000+d, entity=d,
                                       land=[1020+east*10.2, 1020+north*10.2], evidence=points))
            row['placement'] = 'candidate for native ground validation'

    for p in profiles.values():
        if not p['topics']:
            p['topics'] = [dict(title='Greeting', text='Greetings, traveler. I have no task to offer you at present.',
                               kind='authored_greeting')]
        for topic in p['topics']:
            assert len(topic['text'].encode('utf-16-le')) // 2 <= 440
    used = {n['entity'] for n in placements}
    result = dict(version=1, policy='Authored replies from wiki identities and quest links; information only. Existing quest, vendor and service handlers take priority. No recovered dialogue claim.',
                  profiles=sorted(profiles.values(), key=lambda p:p['entity']), npcs=placements, camps=[],
                  templates={str(d):dict(name=native[d]['name'], visual=native[d]['visual']) for d in sorted(used)},
                  sources={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in [wp,tp]})
    report = dict(version=1, category_url=CATEGORY_URL, reviewed_date='2026-11-01',
                  summary=dict(category_entries=len(category), known_names=len(audit),
                               resolved_names=sum(bool(a['entities']) for a in audit),
                               category_resolved=sum(bool(a['entities']) for a in audit if a['category']),
                               profiles=len(profiles), conversation_pages=sum(len(p['topics']) for p in profiles.values()),
                               quest_topics=len({t['title'] for p in profiles.values() for t in p['topics'] if t['title']!='Greeting'}),
                               placement_candidates=len(placements)), entries=audit)
    # Ground results are separate evidence; conversation edits invalidate them.
    # Re-run this audit after population_authoring to record admitted actors.
    manifest, proof = CONTENT/'npc-world-v1.tsv', CONTENT/'npc-world-v1.report.json'
    previous = CONTENT/'npc-dialogue-v1.json'
    if previous.exists() and manifest.exists() and proof.exists():
        evidence=json.loads(proof.read_text(encoding='utf8'))
        if (json.loads(previous.read_text(encoding='utf8'))==result and evidence.get('native_ground_verified')
                and evidence['source_sha256']==hashlib.sha256(previous.read_bytes()).hexdigest()
                and evidence['manifest_sha256']==hashlib.sha256(manifest.read_bytes()).hexdigest()):
            admitted=collections.defaultdict(list)
            for line in manifest.read_text(encoding='utf8').splitlines():
                f=line.split('\t')
                if len(f)==13:admitted[int(f[1],16)].append(f[0])
            for row in audit:
                if row.get('placement')=='candidate for native ground validation':
                    row['grounded_objects']=[iid for d in row['entities'] for iid in admitted[d]]
                    row['placement']='native-ground verified new placement' if row['grounded_objects'] else 'native ground rejected; withheld'
            report['summary']['grounded_placements']=sum(map(len,admitted.values()))
    return result, report


if __name__ == '__main__':
    data, report = build()
    for name, value in [('npc-dialogue-v1.json',data), ('npc-dialogue-v1.audit.json',report)]:
        (CONTENT/name).write_text(json.dumps(value,indent=2)+'\n',encoding='utf8')
    print(json.dumps(report['summary']))
