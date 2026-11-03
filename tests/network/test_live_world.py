"""HTTP world edits published to two real UDP clients, using a disposable store."""
import json
import os
import subprocess
import time
import urllib.error
import urllib.request
import uuid

from test_network import ROOT, SERVER, DAT_DIRECTORY, Peer, prepare_test_content
from test_gameplay import until


def main():
    run = ROOT / 'Saved/AC2NetworkTests' / ('live-world-' + uuid.uuid4().hex)
    run.mkdir(parents=True)
    content = prepare_test_content(run)
    overlay = run / 'overlay.tsv'
    rules_path = overlay.with_suffix('.rules.tsv')
    empty = 'pyrealium-world-v3\n'
    overlay.write_text(empty)
    row = next(line.split('\t') for line in content.read_text().splitlines() if '\t470007d1\t' in line)
    identity = 0x8140000000030800
    row[0], row[3] = f'{identity:016x}', '02990109'
    row[4:7] = ['62.0', '240.199', '-44.606205']
    world = empty + '\t'.join(row) + '\n'
    rules = ('pyrealium-world-rules-v1\n'
             f'stats\t{identity:016x}\t333\t4\t-\t9\t-\t-\t-\t-\t-\t1000\n')
    peers, process, token = [], None, None

    def request(method, path, body=None, credential=None):
        req = urllib.request.Request('http://127.0.0.1:19529' + path, method=method,
            data=None if body is None else json.dumps(body).encode(),
            headers={'Authorization': 'Bearer ' + (credential or token), 'Content-Type': 'application/json'})
        try:
            with urllib.request.urlopen(req, timeout=30) as response:
                return response.status, json.load(response)
        except urllib.error.HTTPError as error:
            return error.code, json.load(error)

    def sync(base, text, rule_text):
        return request('POST', '/admin/sync', dict(base=base, world=text, rules=rule_text))

    def start(log):
        nonlocal process, token
        process = subprocess.Popen([str(SERVER), 'serve', '--bind', '127.0.0.1:19528',
            '--gun-bind', 'off', '--account', 'local', '--data', str(run / 'store'),
            '--game-data', str(DAT_DIRECTORY), '--content', str(content),
            '--content-overlay', str(overlay), '--admin-bind', '127.0.0.1:19529',
            '--initialize-ui', 'true', '--training-kit', 'false', '--encounters', 'training',
            '--lifestones', 'false', '--portals', 'false', '--seconds', '150'],
            env=dict(os.environ), stdout=log, stderr=log,
            creationflags=0x08000000)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            assert process.poll() is None, 'Server exited; see ' + str(run / 'server.log')
            try:
                token = (run / 'store/admin.token').read_text()
                if request('GET', '/admin/status')[0] == 200:
                    return
            except (OSError, ValueError):
                pass
            time.sleep(.1)
        raise AssertionError('Admin endpoint failed to start')

    def stop():
        for peer in peers:
            peer.close()
        peers.clear()
        if process and process.poll() is None:
            (run / 'store/stop.request').write_text('stop\n')
            process.wait(timeout=15)

    def connect(name):
        peer = Peer()
        peers.append(peer)
        peer.send('connect', host='127.0.0.1', port=19528, account='local')
        peer.wait('characters')
        peer.send('create', name=name, race=1, sex=4096, start_area=0, physical=[[1, 1], [2, 1]])
        peer.id = peer.wait('created')['character']['id']
        peer.send('enter', id=peer.id)
        peer.wait('player')
        peer.send('ready')
        peer.wait('status', lambda e: e['phase'] == 'world')
        return peer

    with (run / 'server.log').open('w') as log:
        try:
            start(log)
            assert request('GET', '/admin/status', credential='wrong')[0] == 401
            for name in ['Editor Observer', 'Editor Witness']:
                connect(name)
            code, initial = request('GET', '/admin/status')
            assert code == 200
            marks = [len(p.events) for p in peers]
            code, added = sync(initial['revision'], world, rules)
            assert code == 200 and added['saved'] and added['added'] == 1, added
            for peer, mark in zip(peers, marks):
                until(peer, mark, 'object', lambda e: e['id'] == str(identity))
                until(peer, mark, 'quality', lambda e: e.get('id') == str(identity) and e['key'] == 0x107 and e['value'] == 333)
            marks = [len(p.events) for p in peers]
            updated_rules = rules.replace('\t333\t', '\t123\t')
            code, updated = sync(added['revision'], world, updated_rules)
            assert code == 200 and updated['saved'], updated
            for peer, mark in zip(peers, marks):
                until(peer, mark, 'quality', lambda e: e.get('id') == str(identity) and e['key'] == 0x102 and e['value'] == 123)
            # A failure discovered after creature staging must not partly apply.
            invalid = updated_rules.replace('\t123\t', '\t999\t') + f'loot\t{identity:016x}\t1\t47ffffff\n'
            assert sync(updated['revision'], world, invalid)[0] == 400
            assert request('GET', '/admin/status')[1]['revision'] == updated['revision']
            assert sync(initial['revision'], empty, '')[0] == 409
            assert overlay.read_text() == world and rules_path.read_text() == updated_rules
            # Restart from saved files, then remove the creature live.
            stop()
            start(log)
            peer = connect('Editor Restart')
            until(peer, 0, 'quality', lambda e: e.get('id') == str(identity) and e['key'] == 0x107 and e['value'] == 123)
            code, current = request('GET', '/admin/status')
            mark = len(peer.events)
            code, removed = sync(current['revision'], empty, '')
            assert code == 200 and removed['saved'] and removed['removed'] == 1, removed
            until(peer, mark, 'remove', lambda e: e['id'] == str(identity))
            result = dict(two_client_live_spawn=True, live_vitals=True, rejected_edit_atomic=True,
                          stale_edit_rejected=True, restart_persistence=True, live_removal=True)
            (run / 'result.json').write_text(json.dumps(result, indent=2))
            print('PASS', run, json.dumps(result))
        finally:
            for index, peer in enumerate(peers):
                (run / f'peer-{index}.json').write_text(json.dumps(peer.events))
            stop()


if __name__ == '__main__':
    main()
