"""Map testing travel over real UDP.

All accounts, characters and servers use disposable stores under Saved.
"""
import json
import os
import subprocess
import time
import uuid
from test_network import Peer, ROOT, SERVER, DAT_DIRECTORY, prepare_test_content
from test_gameplay import until


def main():
    enabled = '--disabled' not in os.sys.argv
    directory = ROOT / 'Saved/AC2NetworkTests' / ('map-travel-' + uuid.uuid4().hex)
    directory.mkdir(parents=True)
    content = prepare_test_content(directory)
    env = dict(os.environ)
    log = (directory / 'server.log').open('w', encoding='utf-8')
    server = subprocess.Popen([str(SERVER), 'serve', '--bind', '127.0.0.1:19488',
        '--account', 'local', '--data', str(directory / 'world'), '--game-data', str(DAT_DIRECTORY),
        '--content', str(content), '--initialize-ui', 'true', '--training-kit', 'false',
        '--lifestones', 'false', '--portals', 'false', '--test-map-travel', str(enabled).lower(),
        '--seconds', '180'], env=env, stdout=log, stderr=log, creationflags=0x08000000)
    peers = []
    try:
        time.sleep(1)
        assert server.poll() is None, (directory / 'server.log').read_text()
        for name in (['Map Traveler'] if not enabled else ['Map Traveler', 'Map Observer']):
            peer = Peer()
            peers.append(peer)
            peer.send('connect', host='127.0.0.1', port=19488, account='local')
            peer.wait('characters')
            peer.send('create', name=name, start_area=0, race=1, sex=4096, physical=[[1, 1], [2, 1]])
            peer.id = peer.wait('created')['character']['id']
            assert peer.id != '0'
            peer.send('enter', id=peer.id)
            peer.wait('player')
            peer.spawn = peer.wait('object', lambda e: e['id'] == peer.id)
            peer.send('ready')
            peer.wait('status', lambda e: e['phase'] == 'world')
            if enabled:
                until(peer, 0, 'chat', lambda e: '[Pyrealium:map-travel-v1]' in str(e))
        a = peers[0]
        if not enabled:
            start = len(a.events)
            a.send('chat', text='@maptravel 692.25 828.5', channel='say')
            until(a, start, 'chat', lambda e: 'Map travel is disabled' in str(e))
            assert not any(e['event'] == 'position' and e.get('id') == a.id for e in a.events[start:])
            assert not any('[Pyrealium:map-travel-v1]' in str(e) for e in a.events)
        else:
            b = peers[1]
            until(a, 0, 'object', lambda e: e['id'] == b.id)
            for command, message in [('@maptravel NaN 828', 'outside the world'),
                                     ('@maptravel 2040 1', 'outside the world'),
                                     ('@maptravel 692', 'Use @maptravel')]:
                time.sleep(.55)
                start = len(a.events)
                a.send('chat', text=command, channel='say')
                until(a, start, 'chat', lambda e: message in str(e))
                assert not any(e['event'] == 'position' and e.get('id') == a.id for e in a.events[start:])
            start = len(a.events)
            a.send('combat', enabled=True)
            until(a, start, 'combat', lambda e: e['enabled'])
            time.sleep(.55)
            start = len(a.events)
            a.send('chat', text='@maptravel 692.25 828.5', channel='say')
            until(a, start, 'chat', lambda e: 'Leave combat' in str(e))
            a.send('combat', enabled=False)
            until(a, start, 'combat', lambda e: not e['enabled'])
            time.sleep(.55)
            start, observer = len(a.events), len(b.events)
            a.send('chat', text='@maptravel 692.25 828.5', channel='say')
            pose = until(a, start, 'position', lambda e: e['id'] == a.id)
            assert pose['cell'] >> 16 == 0x5667, pose
            assert abs(pose['position'][0] - 85) < .01 and abs(pose['position'][1] - 90) < .01, pose
            # Far travel removes the actor from the previous scene, then a newly
            # arriving observer receives the authoritative outdoor position.
            until(b, observer, 'remove', lambda e: e['id'] == a.id)
            start = len(b.events)
            b.send('chat', text='@maptravel 692.25 828.5', channel='say')
            until(b, start, 'position', lambda e: e['id'] == b.id)
            remote = until(b, start, 'object', lambda e: e['id'] == a.id)['placement']
            assert remote['cell'] == pose['cell'] and remote['position'] == pose['position'], (remote, pose)
            time.sleep(.55)
            start = len(a.events)
            a.send('chat', text='@maptravel 694 828', channel='say')
            until(a, start, 'chat', lambda e: 'Wait a few seconds' in str(e))
            # Persisted location survives a normal logout/re-entry.
            start = len(a.events)
            a.send('logout', id=a.id)
            until(a, start, 'selection')
            start = len(a.events)
            a.send('enter', id=a.id)
            restored = until(a, start, 'object', lambda e: e['id'] == a.id)['placement']
            assert restored['cell'] == pose['cell'] and restored['position'] == pose['position']
        report = {'result': 'PASS', 'enabled': enabled, 'directory': str(directory)}
        (directory / 'result.json').write_text(json.dumps(report, indent=2))
        print(json.dumps(report))
    finally:
        for peer in peers:
            peer.close()
        if server.poll() is None:
            (directory / 'world/stop.request').write_text('stop\n')
            try:
                server.wait(timeout=8)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
        log.close()


if __name__ == '__main__':
    main()
