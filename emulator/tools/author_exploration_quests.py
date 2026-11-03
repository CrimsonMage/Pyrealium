"""Author a versioned ringway quest policy from recovered native text and guides.

Does not execute archived scripts. NPC/obelisk IDs come from installed templates;
guide rewards/timers and placement offsets remain explicit reconstruction policy.
Do not overwrite a released version when changing journal/reward semantics.
"""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def build():
    content = ROOT / 'emulator/content'
    read = lambda p: json.loads(p.read_text(encoding='utf-8'))
    wiki_path = ROOT / 'Saved/ContentRecovery/wiki-content.json'
    text_path = content / 'recovered-quest-text.json'
    template_path = ROOT / 'Saved/current-native-templates.json'
    wiki = {a['title']: a for a in read(wiki_path)['articles']}
    texts = {(int(a['quest'], 16), a['field']): a for a in read(text_path)['candidates']}
    templates = read(template_path)
    specs = [
        (0x40000016, 'Arwic', 0x4700159f, [0x47000872, 0x47000877, 0x4700087b],
         [(46.9, 0), (42.1, 8.5), (34.3, -2)]),
        (0x40000017, 'Lost Wish', 0x470015a1, [0x47000881, 0x47000893, 0x4700087c],
         [(53.1, 9.9), (53.5, 19), (47.3, 21.4)]),
        (0x40000018, 'Esper', 0x470015a0, [0x4700086b, 0x47000865, 0x47000866],
         [(41.2, -25.9), (29.8, -24.8), (37.8, -19.8)]),
    ]
    quests, npcs, used = [], [], set()
    for qid, region, npc, objectives, anchors in specs:
        title = region + ' Obelisk Search'
        article = wiki[title]
        assert article['facts']['XP Reward'] == '80'
        assert article['facts']['Time Limit'] == '24 hours'
        assert article['facts']['Repeat Time'] == '20 hours'
        native = [texts[qid, field] for field in ['summary'] + [f'phases/{i}/journal' for i in range(1, 5)]]
        quests.append(dict(id=qid, title=title, npc=npc, npc_name=templates[str(npc)]['name'],
                           objectives=objectives, text=native[0]['text_raw'].strip(),
                           phases=[r['text_raw'].strip() for r in native[1:]],
                           xp=80, level=1, limit_seconds=86400, repeat_seconds=72000,
                           evidence=dict(url=article['url'], html_sha256=article['html_sha256'],
                                         native_fields=native)))
        used.add(npc)
        for entity, (north, east) in zip(objectives, anchors):
            # Source coordinates are tenths of a compass degree. Map landscape
            # units use 10.2 per degree and 20 metres per unit (world centre
            # 1020,1020), as in author_recovery_content.coordinate. Offset 8m east of
            # the outgoing ringway before the native-ground search (<=32m).
            npcs.append(dict(id=0x8140d00000000000 + entity, entity=entity,
                             land=[1020 + east*10.2 + .4, 1020 + north*10.2],
                             evidence=dict(north=north, east=east,
                                           basis='Recovered journal outgoing ringway coordinate; inferred obelisk 8m east, native ground search <=32m.')))
            used.add(entity)
    referral = wiki['Find the Arwic Mines Overseer']
    return dict(version=1, policy='Reconstructed quest progression, XP and timers. Native text/IDs; guide rewards. Buff effects are not enabled.',
                quests=quests, referral=dict(id=0x400000c0, npc=0x470015ad,
                title='Find the Arwic Mines Overseer', level=1, xp=0,
                limit_seconds=172800, repeat_seconds=86400,
                text='Speak to the Arwic Mines Overseer in Arwic at 44.0N, 6.1W.',
                evidence=dict(url=referral['url'], html_sha256=referral['html_sha256'])),
                camps=[], npcs=npcs, templates={str(i): templates[str(i)] for i in sorted(used)},
                sources={p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in [text_path, template_path]})


if __name__ == '__main__':
    output = ROOT / 'emulator/content/exploration-content-v1.json'
    if output.exists():
        raise SystemExit('Refusing to overwrite released quest policy; use a new version for semantic changes.')
    output.write_text(json.dumps(build(), indent=2) + '\n', encoding='utf-8')
