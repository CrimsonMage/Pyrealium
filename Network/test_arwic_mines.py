"""Arwic lesson acceptance: native placements, real UDP, disposable character store.

Only the isolated encounter fixture weakens Outcasts for a quick deterministic
kill. Production creature statistics and production journals are never changed.
"""
import json
import math
import os
import subprocess
import time
import uuid
from test_network import ROOT, SERVER, DAT_DIRECTORY, Peer
from test_gameplay import until
from prepare_content import prepare

PORT = 19525
NPC, OUTCAST, HAMMER, IRON = 0x470015AD, 0x470015AC, 0x470015A9, 0x470010F3
SWORD, GLYPH, GUIDE, RECIPE, QUEST = 0x470015AA, 0x4700109F, 0x47002917, 0x6F001F0E, 0x400000C1


def main():
    run = ROOT / 'Saved/AC2NetworkTests' / ('arwic-mines-' + uuid.uuid4().hex)
    run.mkdir(parents=True)
    prepare(DAT_DIRECTORY, run / 'prepared.tsv')
    rows = [line.split('\t') for line in (run / 'prepared.tsv').read_text(encoding='utf-8').splitlines()
            if line.startswith('8140')]
    pack = [r for r in rows if r[0].startswith('8140d100')]
    assert len(pack) == 6 and sum(int(r[1], 16) == OUTCAST for r in pack) == 4
    overseers = [r for r in rows if int(r[1], 16) == NPC]
    assert len(overseers) == 1, 'Overseer must be present exactly once'
    npc = overseers[0]
    mob = next(r for r in pack if int(r[1], 16) == OUTCAST)
    idol = next(r for r in rows if r[1] == '470007d1')
    content = run / 'world.tsv'
    content.write_text('pyrealium-world-v4\n' + '\n'.join('\t'.join(r) for r in [npc, idol, *pack]) + '\n')
    encounters = run / 'encounters.tsv'
    encounters.write_text((ROOT / 'emulator/content/development-encounters.tsv').read_text() +
                          '\nnpc\t470015ac\t20\t1\t1500\t3\t120000\t19\t2\n')
    server, character, peers = None, None, []
    env = dict(os.environ, PYREALIUM_UNREAL_LEGIONS_DATA='1')

    def start(log):
        p = subprocess.Popen([str(SERVER), 'serve', '--bind', f'127.0.0.1:{PORT}', '--account', 'local',
                              '--data', str(run / 'world'), '--game-data', str(DAT_DIRECTORY),
                              '--content', str(content), '--encounters', str(encounters),
                              '--initialize-ui', 'true', '--training-kit', 'false', '--portals', 'false',
                              '--lifestones', 'false', '--test-map-travel', 'true', '--seconds', '180'],
                             env=env, stdout=log, stderr=log, creationflags=0x08000000)
        time.sleep(1)
        assert p.poll() is None, (run / 'server.log').read_text()
        return p

    def stop():
        if server and server.poll() is None:
            (run / 'world/stop.request').write_text('stop\n')
            server.wait(timeout=15)

    def connect(create=False):
        p = Peer(); peers.append(p)
        p.send('connect', host='127.0.0.1', port=PORT, account='local'); p.wait('characters')
        if create:
            p.send('create', name='Arwic Apprentice', race=1, sex=4096, start_area=0, physical=[[1, 1], [2, 1]])
            p.id = p.wait('created')['character']['id']
        else:
            p.id = character
        p.send('enter', id=p.id); p.wait('player'); p.send('ready')
        p.wait('status', lambda e: e['phase'] == 'world'); until(p, 0, 'inventory')
        return p

    def at(p, row, distance=.5):
        pos = list(map(float, row[4:7])); pos[0] += distance
        cell = int(row[3], 16)
        if cell & 65535 < 256:
            dx, dy = math.floor(pos[0] / 160), math.floor(pos[1] / 160)
            pos[0] -= dx * 160; pos[1] -= dy * 160
            cell = (((cell >> 24) + dx) << 24) | ((((cell >> 16) & 255) + dy) << 16) | (int(pos[0] / 20) * 8 + int(pos[1] / 20) + 1)
        mark = len(p.events)
        p.send('move', cell=cell, position=pos, heading=0, motion=[0, 0, 0])
        until(p, 0, 'object', lambda e: e['id'] == str(int(row[0], 16)))
        time.sleep(.25)
        return mark

    def use(p, item, target='0'):
        time.sleep(1.1); mark = len(p.events); p.send('use', item=item, target=target); return mark

    def quest(p, mark, phase):
        until(p, mark, 'quests', lambda e: any(q['id'] == QUEST and q['phase'] == phase for q in e['items']))

    def owned(p, definition, mark=0):
        return until(p, mark, 'object', lambda e: e.get('entity') == definition and e.get('container') == p.id)

    def craft(p, slots, success):
        time.sleep(1.1); mark = len(p.events)
        p.send('craft_execute', recipe=RECIPE, spinner=0, helper=p.id, ingredients=slots)
        assert (until(p, mark, 'craft_complete')['error'] == 0) == success
        return mark

    def peace(p):
        mark = len(p.events); p.send('combat', enabled=False)
        until(p, mark, 'combat', lambda e: not e['enabled'])

    def leave(p):
        mark = len(p.events); p.send('logout', id=p.id); until(p, mark, 'selection'); p.close()

    with (run / 'server.log').open('w') as log:
        try:
            server = start(log); p = connect(True); character = p.id
            assert RECIPE not in until(p, 0, 'craft_registry')['recipes']
            # The client cannot unlock a recipe by forging an execute request.
            starter = owned(p, 0x470007EE)
            craft(p, [{'ordinal': 1, 'item': starter['id']}], False)
            at(p, npc, 8); mark = use(p, str(int(npc[0], 16)))
            until(p, mark, 'chat', lambda e: 'four metres' in str(e))
            at(p, npc); mark = len(p.events); p.send('combat', enabled=True)
            until(p, mark, 'combat', lambda e: e['enabled'])
            mark = use(p, str(int(npc[0], 16))); until(p, mark, 'chat', lambda e: 'out of combat' in str(e))
            peace(p); mark = use(p, str(int(npc[0], 16))); quest(p, mark, 1)
            until(p, mark, 'chat', lambda e: 'flat hammers' in str(e))
            # Combat, owned corpse creation and native loot claims, not a grant shortcut.
            at(p, mob); mid = str(int(mob[0], 16)); mark = len(p.events)
            p.send('attack', id=mid)
            until(p, mark, 'quality', lambda e: e.get('id') == mid and e['key'] == 0x102 and e['value'] == 0)
            peace(p); mark = len(p.events); p.send('open_container', id=mid)
            corpse = until(p, mark, 'container_state', lambda e: e['id'] == mid and e['open'])
            hammer = until(p, mark, 'object', lambda e: e.get('entity') == HAMMER)
            mark = len(p.events); p.send('take_item', container=mid, id=hammer['id'], **{'from': corpse['items'].index(hammer['id'])})
            owned(p, HAMMER, mark); quest(p, mark, 2)
            leave(p); stop(); server = start(log); p = connect(); quest(p, 0, 2)
            assert RECIPE not in until(p, 0, 'craft_registry')['recipes']
            at(p, npc); mark = use(p, owned(p, HAMMER)['id'], str(int(npc[0], 16))); quest(p, mark, 3)
            iron = owned(p, IRON, mark); assert iron['quantity'] == 20
            registry = until(p, mark, 'craft_registry', lambda e: RECIPE in e['recipes'])
            assert not registry['replace'] and registry['catalog'][0]['ingredients'][0]['name'] == 'Iron'
            time.sleep(1.1); mark = len(p.events)
            p.send('craft_validate', recipe=RECIPE, ordinal=1, item=iron['id'], spinner=0)
            assert until(p, mark, 'craft_ingredient')['error'] == 0
            mark = craft(p, [{'ordinal': 1, 'item': iron['id']}], True)
            sword = owned(p, SWORD, mark); quest(p, mark, 4)
            mark = len(p.events); p.send('examine', id=sword['id'])
            assert 'Weapon damage: 10' in str(until(p, mark, 'examination'))
            # Replay of the consumed Iron identity must fail without granting a second sword.
            craft(p, [{'ordinal': 1, 'item': iron['id']}], False)
            leave(p); stop(); server = start(log); p = connect(); quest(p, 0, 4)
            assert RECIPE in until(p, 0, 'craft_registry')['recipes']
            at(p, npc); mark = use(p, owned(p, SWORD)['id'], str(int(npc[0], 16))); quest(p, mark, 0)
            owned(p, GLYPH, mark); guide = owned(p, GUIDE, mark)
            mark = use(p, guide['id']); until(p, mark, 'chat', lambda e: 'Page 1/15' in str(e))
            mark = use(p, guide['id']); until(p, mark, 'chat', lambda e: 'Page 2/15' in str(e))
            mark = use(p, str(int(npc[0], 16))); until(p, mark, 'chat', lambda e: '20 hours' in str(e))
            assert not any(e.get('event') == 'object' and e.get('entity') in (GLYPH, GUIDE) for e in p.events[mark:])
            leave(p); stop(); server = start(log); p = connect(); quest(p, 0, 0)
            owned(p, GLYPH); owned(p, GUIDE)
            assert not any(e.get('event') == 'object' and e.get('container') == p.id and e.get('entity') == SWORD for e in p.events)
            leave(p)
            report = {'result': 'PASS', 'production_mine_spawners': len(pack), 'checks': [
                'single Overseer, four Outcasts, two beetles in validated rooms',
                'unlearned craft, remote NPC and combat NPC use rejected',
                'actual combat and owned corpse hammer claim',
                'native quest journal and recipe unlock survive restarts',
                '20 Iron, no forge, guaranteed sword with damage inspection',
                'consumed ingredient replay rejected',
                'sword turn-in, glyph and readable 15-page guide, no duplicate rewards']}
            (run / 'result.json').write_text(json.dumps(report, indent=2)); print(json.dumps(report)); print('Evidence:', run)
        finally:
            for i, p in enumerate(peers):
                (run / f'peer-{i}.json').write_text(json.dumps(p.events)); p.close()
            stop()


if __name__ == '__main__':
    main()
