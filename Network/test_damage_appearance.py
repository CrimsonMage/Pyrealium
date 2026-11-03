"""Real UDP health replication needed by native persistent wound rendering.

Uses disposable actors/accounts; does not alter production placement or saves.
"""
import json
import os
import subprocess
import time
import uuid
from test_network import ROOT, SERVER, DAT_DIRECTORY, Peer
from test_gameplay import until


def main():
    run = ROOT / 'Saved/AC2NetworkTests' / ('damage-' + uuid.uuid4().hex)
    run.mkdir(parents=True)
    actor = str(0x8140000000030a00)
    content = run / 'world.tsv'
    content.write_text('pyrealium-world-v3\n'
        '8140000000030a00\t470007d1\t1f0003d1\t76b8003c\t159.9\t70\t182.5\t1\t0\t0\t0\t-\n')
    rules = run / 'encounters.tsv'
    rules.write_text('pyrealium-encounters-v1\nplayer\t40\t1\t2000\t3\nregen\t0\t0\t1000\n'
        'npc\t470007d1\t100\t55\t5000\t3\t30000\t5\t1\n')
    peers = []
    with (run / 'server.log').open('w') as log:
        server = subprocess.Popen([str(SERVER), 'serve', '--bind', '127.0.0.1:19398', '--account', 'local',
            '--data', str(run / 'world'), '--game-data', str(DAT_DIRECTORY), '--content', str(content),
            '--encounters', str(rules), '--initialize-ui', 'true', '--training-kit', 'false',
            '--lifestones', 'false', '--portals', 'false', '--seconds', '90'],
            env=dict(os.environ, PYREALIUM_UNREAL_LEGIONS_DATA='1'), stdout=log, stderr=log, creationflags=0x08000000)
        try:
            time.sleep(1)
            assert server.poll() is None, (run / 'server.log').read_text()
            for name in ['Wound Hero', 'Wound Observer']:
                p = Peer(); peers.append(p)
                p.send('connect', host='127.0.0.1', port=19398, account='local'); p.wait('characters')
                p.send('create', name=name, race=1, sex=4096, start_area=0, physical=[[1, 1], [2, 1]])
                p.id = p.wait('created')['character']['id']
                p.send('enter', id=p.id); p.wait('player'); p.send('ready')
                p.wait('status', lambda e: e['phase'] == 'world')
                p.send('move', cell=0x76b8003c, position=[158.2, 70, 182.5], heading=0, motion=[0, 0, 0])
                until(p, 0, 'object', lambda e: e['id'] == actor)
            a, b = peers
            def vital(p, mark, who, key, predicate=lambda value: True):
                return until(p, mark, 'quality', lambda e: (e.get('id') == who or (who == p.id and e.get('id') is None)) and e.get('table') == 4
                    and e.get('key') == key and predicate(e['value']))['value']
            maximum = vital(a, 0, a.id, 0x107)
            assert vital(b, 0, a.id, 0x107) == maximum
            assert vital(a, 0, actor, 0x107) == vital(b, 0, actor, 0x107) == 100
            ma, mb = len(a.events), len(b.events)
            a.send('attack', id=actor)
            injured = vital(a, ma, actor, 0x102, lambda v: 0 < v < 100)
            a.send('stop_attack'); until(a, ma, 'combat', lambda e: not e['enabled'])
            assert vital(b, mb, actor, 0x102) == injured
            hurt = vital(a, ma, a.id, 0x102, lambda v: 0 < v < maximum)
            assert vital(b, mb, a.id, 0x102) == hurt
            # A returning observer gets current public vitals without needing a new hit.
            mark = len(b.events); b.send('logout', id=b.id); until(b, mark, 'selection')
            mark = len(b.events); b.send('enter', id=b.id); until(b, mark, 'player'); b.send('ready')
            assert vital(b, mark, actor, 0x107) == 100
            assert vital(b, mark, actor, 0x102) == injured
            assert vital(b, mark, a.id, 0x107) == maximum
            assert vital(b, mark, a.id, 0x102) == hurt
            # Creature disengagement heals the material state on both clients.
            ma, mb = len(a.events), len(b.events)
            a.send('move', cell=0x77b8000c, position=[30, 70, 182.5], heading=0, motion=[0, 0, 0])
            assert vital(a, ma, actor, 0x102, lambda v: v == 100) == 100
            assert vital(b, mb, actor, 0x102, lambda v: v == 100) == 100
            result = {'result': 'PASS', 'directory': str(run), 'checks': [
                'creature damage reaches owner and observer', 'player damage reaches owner and observer',
                'late observer receives current/max health for player and creature', 'healing reaches both clients']}
            (run / 'result.json').write_text(json.dumps(result, indent=2)); print(json.dumps(result))
        finally:
            for index, p in enumerate(peers):
                (run / f'peer-{index}.json').write_text(json.dumps(p.events)); p.close()
            if server.poll() is None:
                (run / 'world/stop.request').write_text('stop\n')
                try: server.wait(timeout=8)
                except subprocess.TimeoutExpired: server.terminate(); server.wait()


if __name__ == '__main__':
    main()
