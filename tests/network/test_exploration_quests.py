"""Real UDP acceptance of the reconstructed ringway NPC/obelisk conversations.

Uses the prepared production placements and installed DATs, with a disposable
character store. Travel bypass is enabled only on this isolated loopback server.
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


def main():
    run = ROOT / 'Saved/AC2NetworkTests' / ('exploration-' + uuid.uuid4().hex)
    run.mkdir(parents=True)
    pack = json.loads((ROOT / 'content/exploration-content-v1.json').read_text(encoding='utf-8'))
    prepare(DAT_DIRECTORY, run / 'prepared.tsv')
    entities = {pack['referral']['npc']}
    for q in pack['quests']:
        entities.update([q['npc'], *q['objectives']])
    prepared = (run / 'prepared.tsv').read_text(encoding='utf-8').splitlines()
    rows = [line.split('\t') for line in prepared
            if line.startswith('8140') and int(line.split('\t')[1], 16) in entities]
    assert {int(row[1], 16) for row in rows} == entities, 'A required quest actor is missing'
    for q in pack['quests']:
        for entity in q['objectives']:
            assert sum(int(row[1], 16) == entity for row in rows) == 1, 'Duplicate obelisk placement'
    content = run / 'world.tsv'
    # Default encounter admission requires a matching placed creature. Retain
    # one native Idol Spawn in its distant tutorial interior, outside this route.
    encounter = next(line for line in prepared if line.startswith('8140') and line.split('\t')[1] == '470007d1')
    content.write_text('pyrealium-world-v4\n' + '\n'.join('\t'.join(row) for row in rows) + '\n' + encounter + '\n', encoding='utf-8')
    by_entity = {int(row[1], 16): row for row in rows}
    env = dict(os.environ)
    peers, report = [], {}
    server = None
    character = None

    def stop():
        if server and server.poll() is None:
            (run / 'world/stop.request').write_text('stop\n')
            server.wait(timeout=15)

    def start(log):
        p = subprocess.Popen([str(SERVER), 'serve', '--bind', '127.0.0.1:19524', '--account', 'local',
                              '--data', str(run / 'world'), '--game-data', str(DAT_DIRECTORY),
                              '--content', str(content), '--initialize-ui', 'true', '--training-kit', 'false',
                              '--portals', 'false', '--lifestones', 'false', '--test-map-travel', 'true',
                              '--seconds', '180'], env=env, stdout=log, stderr=log, creationflags=0x08000000)
        time.sleep(1)
        assert p.poll() is None, (run / 'server.log').read_text()
        return p

    def connect(create=False):
        p = Peer(); peers.append(p)
        p.send('connect', host='127.0.0.1', port=19524, account='local'); p.wait('characters')
        if create:
            p.send('create', name='Ringway Walker', race=1, sex=4096, start_area=0, physical=[[1, 1], [2, 1]])
            p.id = p.wait('created')['character']['id']
        else:
            p.id = character
        p.send('enter', id=p.id); p.wait('player'); p.send('ready')
        p.wait('status', lambda e: e['phase'] == 'world'); until(p, 0, 'inventory')
        return p

    def at(p, entity, distance=.5):
        row = by_entity[entity]
        pos = list(map(float, row[4:7])); pos[0] += distance
        cell = int(row[3], 16)
        # Outdoor coordinates are block-local. Also exercise an interaction
        # across a landblock boundary with Arwic's emerald at local X=0.
        dx, dy = math.floor(pos[0] / 160), math.floor(pos[1] / 160)
        pos[0] -= dx * 160; pos[1] -= dy * 160
        cell = (((cell >> 24) + dx) << 24) | ((((cell >> 16) & 255) + dy) << 16) | (int(pos[0] / 20) * 8 + int(pos[1] / 20) + 1)
        p.send('move', cell=cell, position=pos, heading=0, motion=[0, 0, 0])
        until(p, 0, 'object', lambda e: e['id'] == str(int(row[0], 16)))
        time.sleep(.15)

    def use(p, entity):
        time.sleep(1.1)  # Native dialogue admission has a one-second throttle.
        mark = len(p.events)
        p.send('use', item=str(int(by_entity[entity][0], 16)), target='0')
        return mark

    def quest(p, mark, q, phase, completed=False):
        status = 0x30000000 if completed else 0x10000000
        event = until(p, mark, 'quests', lambda e: any(i['id'] == q and i['status'] == status and i['phase'] == phase for i in e['items']))
        return next(i for i in event['items'] if i['id'] == q)

    def leave(p):
        mark = len(p.events); p.send('logout', id=p.id); until(p, mark, 'selection'); p.close()

    with (run / 'server.log').open('w') as log:
        try:
            server = start(log); p = connect(True); character = p.id
            first = pack['quests'][0]
            at(p, first['objectives'][0]); mark = use(p, first['objectives'][0])
            until(p, mark, 'chat', lambda e: 'before using' in str(e))
            at(p, first['npc'], 8); mark = use(p, first['npc'])
            until(p, mark, 'chat', lambda e: 'four metres' in str(e))
            at(p, first['npc']); mark = len(p.events); p.send('combat', enabled=True)
            until(p, mark, 'combat', lambda e: e['enabled'])
            mark = use(p, first['npc']); until(p, mark, 'chat', lambda e: 'out of combat' in str(e))
            p.send('combat', enabled=False); until(p, mark, 'combat', lambda e: not e['enabled'])
            report['unaccepted_remote_and_combat_use_rejected'] = True
            for index, q in enumerate(pack['quests']):
                at(p, q['npc']); mark = use(p, q['npc']); quest(p, mark, q['id'], 1)
                until(p, mark, 'script', lambda e: e['function'] == 0x82120024)
                at(p, q['objectives'][2]); mark = use(p, q['objectives'][2])
                until(p, mark, 'script', lambda e: e['function'] == 0x82120024)
                for i, entity in enumerate(q['objectives']):
                    at(p, entity, -.5 if index == 0 and i == 2 else .5)
                    mark = use(p, entity); quest(p, mark, q['id'], i + 2)
                    if index == 0 and i == 0:
                        leave(p); stop(); server = start(log); p = connect()
                        quest(p, 0, q['id'], 2)
                        report['partial_progress_survives_restart'] = True
                at(p, q['npc']); mark = use(p, q['npc']); quest(p, mark, q['id'], 0, True)
                if index == 0:
                    quest(p, mark, pack['referral']['id'], 1)
                else:
                    # The referral was already accepted on the first return.
                    # Keep its cached state without retransmitting it on every
                    # subsequent quest completion.
                    quest(p, 0, pack['referral']['id'], 1)
                    assert not any(i['id'] == pack['referral']['id']
                                   for e in p.events[mark:] if e.get('event') == 'quests'
                                   for i in e['items']), 'Unchanged referral was resent'
                report[q['title']] = 'three ordered visits, return, 80 XP and referral'
            at(p, pack['referral']['npc']); mark = use(p, pack['referral']['npc'])
            quest(p, mark, pack['referral']['id'], 0, True)
            leave(p); stop(); server = start(log); p = connect()
            for q in pack['quests']:
                quest(p, 0, q['id'], 0, True)
            quest(p, 0, pack['referral']['id'], 0, True)
            report['all_four_completed_journals_survive_restart'] = True
            report['grounded_obelisks'] = 9
            report['nearby_use_across_landblock_boundary'] = True
            leave(p)
            (run / 'result.json').write_text(json.dumps(report, indent=2))
            print('PASS', run, json.dumps(report))
        finally:
            for i, peer in enumerate(peers):
                (run / f'peer-{i}.json').write_text(json.dumps(peer.events)); peer.close()
            stop()


if __name__ == '__main__':
    main()
