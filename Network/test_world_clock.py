"""Clock progression through real native UDP, including idle clients and restart."""
import json
import os
import subprocess
import time
import uuid
from test_network import ROOT, SERVER, DAT_DIRECTORY, Peer


def main():
    directory = ROOT / 'Saved/WorldClockTests' / uuid.uuid4().hex
    directory.mkdir(parents=True)
    world = directory / 'world'
    log = (directory / 'server.log').open('w', encoding='utf-8')
    args = [str(SERVER), 'serve', '--bind', '127.0.0.1:19290', '--account', 'local',
            '--data', str(world), '--game-data', str(DAT_DIRECTORY),
            '--initialize-ui', 'true', '--training-kit', 'false', '--content', 'none',
            '--encounters', 'none', '--lifestones', 'false', '--portals', 'false', '--seconds', '45']
    peers = []
    server = None

    def start():
        process = subprocess.Popen(args, env=dict(os.environ, PYREALIUM_UNREAL_LEGIONS_DATA='1'),
                                   stdout=log, stderr=log, creationflags=0x08000000)
        time.sleep(.5)
        assert process.poll() is None, (directory / 'server.log').read_text()
        return process

    def connect():
        peer = Peer()
        peers.append(peer)
        peer.send('connect', host='127.0.0.1', port=19290, account='local')
        sample = peer.wait('clock')
        return peer, sample['server_time'], time.monotonic()

    try:
        server = start()
        a, first, t0 = connect()
        # No movement, characters or gameplay events needed to advance time.
        update = a.wait('clock', lambda e: e['server_time'] > first + 4, timeout=8)['server_time']
        elapsed = time.monotonic() - t0
        assert abs((update - first) - elapsed) < .5, (first, update, elapsed)
        b, second, t1 = connect()
        assert abs((second - first) - (t1 - t0)) < .5, (first, second)
        for peer in peers:
            peer.close()
        (world / 'stop.request').write_text('stop\n')
        assert server.wait(timeout=8) == 0
        # Simulate a world already running for days: native calendar should consume
        # arbitrary server seconds, not a reset-at-noon session time.
        origin = json.loads((world / 'clock-origin.json').read_text())
        origin['unix_seconds'] -= 7347 * 3
        (world / 'clock-origin.json').write_text(json.dumps(origin))
        server = start()
        c, restored, t2 = connect()
        assert abs((restored - first) - (t2 - t0) - 7347 * 3) < .5, restored
        repeat = c.wait('clock', lambda e: e['server_time'] > restored + 4, timeout=8)['server_time']
        assert 4 < repeat - restored < 6, (restored, repeat)
        d, _, _ = connect()
        for peer, name in [(c, 'Clock Walker'), (d, 'Clock Observer')]:
            peer.send('create', name=name, start_area=0, race=1, sex=4096, physical=[[1, 1], [2, 1]])
            peer.id = peer.wait('created')['character']['id']
            peer.send('enter', id=peer.id)
            peer.wait('player')
            peer.spawn = peer.wait('object', lambda e: e['id'] == peer.id)
            peer.send('ready')
            peer.wait('status', lambda e: e['phase'] == 'world')
        pose = c.spawn['placement']
        destination = list(pose['position'])
        destination[0] += .5
        c.send('move', cell=pose['cell'], position=destination, heading=90, motion=[0, 1, 0])
        moved = d.wait('position', lambda e: e['id'] == c.id)
        assert abs(moved['position'][0] - destination[0]) < .01, moved
        assert repeat <= moved['time'] < repeat + 15, moved
        report = dict(result='PASS', initial=first, idle_update=update,
                      second_client=second, after_restart=restored, repeated=repeat,
                      checks=['idle clock progresses', 'periodic native TIME_SYNC decoded',
                              'different login times share clock', 'durable origin across restart',
                              'multi-day elapsed time preserved',
                              'world entry and replicated movement use synchronized seconds'])
        (directory / 'result.json').write_text(json.dumps(report, indent=2))
        print(json.dumps(dict(report, directory=str(directory))))
    finally:
        for peer in peers:
            peer.close()
        if server is not None and server.poll() is None:
            (world / 'stop.request').write_text('stop\n')
            try:
                server.wait(timeout=8)
            except subprocess.TimeoutExpired:
                server.kill()
        log.close()


if __name__ == '__main__':
    main()
