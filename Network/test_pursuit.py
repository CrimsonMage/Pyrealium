"""Real UDP pursuit across a DAT-verified block boundary and return to home.

The single creature is an isolated test fixture, never a production spawn.
"""
import json
import os
import subprocess
import time
import uuid
from test_network import ROOT, SERVER, DAT_DIRECTORY, Peer
from test_gameplay import until


def main():
    run = ROOT / 'Saved/AC2NetworkTests' / ('pursuit-' + uuid.uuid4().hex)
    run.mkdir(parents=True)
    actor = str(0x8140000000030a00)
    content = run / 'world.tsv'
    content.write_text('pyrealium-world-v3\n'
        '8140000000030a00\t470007d1\t1f0003d1\t76b8003c\t159.9\t70\t182.5\t1\t0\t0\t0\t-\n')
    rules = run / 'encounters.tsv'
    rules.write_text('pyrealium-encounters-v1\nplayer\t5\t1\t2000\t3\nregen\t0\t0\t1000\n'
        'npc\t470007d1\t1000\t1\t3000\t3\t30000\t5\t1\n')
    peers = []
    with (run / 'server.log').open('w') as log:
        server = subprocess.Popen([str(SERVER), 'serve', '--bind', '127.0.0.1:19396', '--account', 'local',
            '--data', str(run / 'world'), '--game-data', str(DAT_DIRECTORY), '--content', str(content),
            '--encounters', str(rules), '--initialize-ui', 'true', '--training-kit', 'false',
            '--lifestones', 'false', '--portals', 'false', '--seconds', '90'],
            env=dict(os.environ, PYREALIUM_UNREAL_LEGIONS_DATA='1'), stdout=log, stderr=log, creationflags=0x08000000)
        try:
            time.sleep(1)
            assert server.poll() is None, (run / 'server.log').read_text()
            for name in ['Pursuit Hero', 'Pursuit Observer']:
                p = Peer(); peers.append(p)
                p.send('connect', host='127.0.0.1', port=19396, account='local'); p.wait('characters')
                p.send('create', name=name, race=1, sex=4096, start_area=0, physical=[[1, 1], [2, 1]])
                p.id = p.wait('created')['character']['id']
                p.send('enter', id=p.id); p.wait('player'); p.send('ready')
                p.wait('status', lambda e: e['phase'] == 'world')
                p.send('move', cell=0x76b8003c, position=[158.2, 70, 182.5], heading=0, motion=[0, 0, 0])
                until(p, 0, 'object', lambda e: e['id'] == actor)
            a, b = peers
            mark = len(a.events); a.send('attack', id=actor)
            until(a, mark, 'hit', lambda e: e['id'] == actor)
            a.send('stop_attack'); until(a, mark, 'combat', lambda e: not e['enabled'])
            ma, mb = len(a.events), len(b.events)
            a.send('move', cell=0x77b80004, position=[5, 70, 182.5], heading=0, motion=[0, 0, 0])
            crossed = until(a, ma, 'position', lambda e: e['id'] == actor and e['cell'] == 0x77b80004)
            observed = until(b, mb, 'position', lambda e: e['id'] == actor and e['cell'] == crossed['cell'])
            assert crossed['position'] == observed['position']
            assert 0 <= crossed['position'][0] < 1, crossed
            until(a, ma, 'position', lambda e: e['id'] == actor and e['cell'] == 0x77b80004 and e['position'][0] >= 1)
            # Late observers receive the current position, not the captured home.
            mark = len(b.events); b.send('logout', id=b.id); until(b, mark, 'selection')
            mark = len(b.events); b.send('enter', id=b.id); until(b, mark, 'player'); b.send('ready')
            snapshot = until(b, mark, 'object', lambda e: e['id'] == actor)['placement']
            assert snapshot['cell'] == 0x77b80004 and snapshot['position'][0] < 5, snapshot
            # Exceed homesickness; the creature retraces validated steps.
            ma, mb = len(a.events), len(b.events)
            a.send('move', cell=0x77b8000c, position=[30, 70, 182.5], heading=0, motion=[0, 0, 0])
            returned = until(b, mb, 'position', lambda e: e['id'] == actor and e['cell'] == 0x76b8003c
                and abs(e['position'][0] - 159.9) < .04, timeout=20)
            assert abs(returned['position'][2] - 182.5) < .01
            positions = [e for e in b.events[mb:] if e.get('event') == 'position' and e['id'] == actor]
            assert len(positions) >= 2, 'Return should walk, not teleport'
            for first, second in zip(positions, positions[1:]):
                def x(e): return (e['cell'] >> 24) * 160 + e['position'][0]
                assert abs(x(first) - x(second)) <= .25
            result = {'result': 'PASS', 'directory': str(run), 'checks': [
                'native floor crossing at landblock boundary', 'same movement replicated to two players',
                'late observer receives current creature pose', 'homesickness retraces steps to authored home']}
            (run / 'result.json').write_text(json.dumps(result, indent=2)); print(json.dumps(result))
        finally:
            for index, p in enumerate(peers):
                (run / f'peer-{index}.json').write_text(json.dumps(p.events)); p.close()
            if server.poll() is None:
                (run / 'world/stop.request').write_text('stop\n')
                try: server.wait(timeout=8)
                except subprocess.TimeoutExpired: server.terminate(); server.wait()


if __name__ == '__main__': main()
