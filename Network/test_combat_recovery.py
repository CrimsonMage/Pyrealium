"""Authenticated range rejection, equipment recovery and native room-boundary combat.

Uses disposable accounts/content only. Fixture positions test protocol behavior;
they are not claimed retail spawns or a terrain/line-of-sight acceptance test.
"""
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import time
import urllib.request
import uuid
from test_network import Peer, ROOT, SERVER, DAT_DIRECTORY, prepare_test_content
from test_gameplay import until
from prepare_content import Archive


def visible_cells(archive, cell):
    # CEnvCell::StreamPack: frame, flags, environment, portals, PVS, shared cells.
    data = archive.read(cell)
    count = struct.unpack_from('<I', data, 40)[0]
    offset = 44
    for _ in range(count):
        flags = struct.unpack_from('<H', data, offset)[0]
        offset += 8
        if flags & 4:
            stabs = struct.unpack_from('<H', data, offset)[0]
            offset = (offset + 2 + stabs * 2 + 3) & ~3
    offset = (offset + 3) & ~3
    count = struct.unpack_from('<I', data, offset)[0]
    return {cell} | {(cell & 0xffff0000) | v for v in struct.unpack_from(f'<{count}H', data, offset + 4)}


def main():
    directory = ROOT / 'Saved/AC2NetworkTests' / ('combat-recovery-' + uuid.uuid4().hex)
    directory.mkdir(parents=True)
    auth, world = directory / 'accounts', directory / 'world'
    server = Path(os.environ.get('AC2_TEST_SERVER', SERVER))
    for port, kind in [(19490, socket.SOCK_STREAM), (19492, socket.SOCK_DGRAM), (19493, socket.SOCK_DGRAM)]:
        with socket.socket(socket.AF_INET, kind) as check:
            check.bind(('127.0.0.1', port))
    content = prepare_test_content(directory)
    cell = 0x02990109
    archive = Archive(DAT_DIRECTORY / 'cell_1.dat')
    visible = visible_cells(archive, cell)
    adjacent = next(c for c in sorted(visible) if c != cell and c & 0xffff >= 256)
    hidden = next(e[0] for e in archive.entries() if e[0] >> 16 == cell >> 16 and
                  256 <= e[0] & 0xffff < 0xfffe and e[0] not in visible)
    spawn = [59.060577, 240.199, -44.606205]
    source = next(line.split('\t') for line in content.read_text().splitlines()[1:]
                  if line and not line.startswith('#') and line.split('\t')[1] == '470007d1')
    fixture_ids = [0x8140000000030200 + n for n in range(4)]
    with content.open('a') as output:
        for iid, room, offset in [(fixture_ids[0], adjacent, 12), (fixture_ids[1], adjacent, 1),
                                  (fixture_ids[2], hidden, 1), (fixture_ids[3], adjacent, 14)]:
            row = source[:]
            row[0], row[3] = f'{iid:016x}', f'{room:08x}'
            row[4:7] = [str(spawn[0] + offset), str(spawn[1]), str(spawn[2])]
            output.write('\t'.join(row) + '\n')
    env = dict(os.environ, PYREALIUM_UNREAL_LEGIONS_DATA='1')
    log = (directory / 'server.log').open('w', encoding='utf-8')
    password = 'Pyrealium-' + uuid.uuid4().hex
    processes, peers = [], []
    def launch(args, stop):
        process = subprocess.Popen([str(server), *args], env=env, stdout=log, stderr=log, creationflags=0x08000000)
        processes.append((process, stop))
        time.sleep(.8)
        assert process.poll() is None, (directory / 'server.log').read_text()
    def login(account):
        request = urllib.request.Request('http://127.0.0.1:19490/api/login',
            data=json.dumps({'account': account, 'password': password}).encode(),
            headers={'Content-Type': 'application/json'}, method='POST')
        with urllib.request.urlopen(request, timeout=10) as response:
            return json.load(response)
    try:
        launch(['account-web', '--auth-data', str(auth), '--data', str(world), '--web-bind', '127.0.0.1:19490',
                '--public-origin', 'http://127.0.0.1:19490', '--public-host', '127.0.0.1', '--public-port', '19492'],
               auth / 'account-stop.request')
        launch(['serve', '--bind', '127.0.0.1:19492', '--auth-data', str(auth), '--data', str(world),
                '--game-data', str(DAT_DIRECTORY), '--content', str(content), '--initialize-ui', 'true',
                '--training-kit', 'false', '--lifestones', 'false', '--portals', 'false', '--seconds', '90'],
               world / 'stop.request')
        for index, name in enumerate(['Recovery Hero', 'Recovery Observer']):
            ticket = login(f'combat_recovery_{index}')
            peer = Peer(); peers.append(peer)
            peer.send('connect', host=ticket['game_host'], port=ticket['game_port'], account=ticket['account'], ticket=ticket['ticket'])
            ticket = None
            peer.wait('characters')
            peer.send('create', name=name, start_area=0, race=1, sex=4096, physical=[[1, 1], [2, 1]])
            peer.id = peer.wait('created')['character']['id']
            peer.send('enter', id=peer.id); peer.wait('player')
            peer.send('ready'); peer.wait('status', lambda e: e['phase'] == 'world')
            until(peer, 0, 'inventory', lambda e: len(e.get('items', [])) >= 3)
        a, b = peers
        far, near, hidden_id, magic_target = map(str, fixture_ids)
        until(a, 0, 'object', lambda e: e['id'] == near)
        assert not any(e.get('event') == 'object' and e['id'] == hidden_id for e in a.events)
        for command in ['combat', 'attack']:
            start, observed = len(a.events), len(b.events)
            a.send('target', id=far)
            a.send(command, **({'enabled': True} if command == 'combat' else {'id': far}))
            until(a, start, 'chat', lambda e: 'out of attack range' in str(e).lower())
            until(a, start, 'combat', lambda e: not e['enabled'], timeout=4)
            until(a, start, 'mode', lambda e: e['id'] == a.id and e['mode'] == 0x40000001)
            until(b, observed, 'mode', lambda e: e['id'] == a.id and e['mode'] == 0x40000001)
            assert not any(e.get('event') == 'hit' for e in a.events[start:])
        inventory = next(e for e in a.events if e.get('event') == 'inventory')
        sword = next(e for e in a.events if e.get('event') == 'object' and e.get('container') == a.id and e['entity'] == 0x470007ee)
        start = len(a.events)
        a.send('equip', id=sword['id'], location=0x40, slot=inventory['items'].index(sword['id']))
        ack = until(a, start, 'inventory_result', lambda e: e['id'] == sword['id'])
        assert ack['status'] == 0 and ack['equipped'] == 0x40, ack
        # No-target stance remains available, and explicit peace still unlocks equipment.
        start = len(a.events)
        a.send('target', id='0'); a.send('combat', enabled=True)
        until(a, start, 'combat', lambda e: e['enabled'])
        a.send('combat', enabled=False)
        until(a, start, 'combat', lambda e: not e['enabled'])
        # Close, visible room across a cell boundary: melee damage and retaliation.
        start, observed = len(a.events), len(b.events)
        a.send('attack', id=near)
        until(a, start, 'combat', lambda e: e['enabled'])
        until(a, start, 'hit', lambda e: e['id'] == near)
        until(a, start, 'hit', lambda e: e['id'] == a.id)
        until(b, observed, 'hit', lambda e: e['id'] == near)
        until(a, start, 'quality', lambda e: e.get('id') == near and e['key'] == 0x102 and e['value'] == 0)
        until(a, start, 'combat', lambda e: not e['enabled'])
        for peer, mark in [(a, start), (b, observed)]:
            until(peer, mark, 'behavior', lambda e: e['id'] == near and e['behavior'] == 0x4100001f and e['mode'] == 0x41000005)
        # The old 1.5s removal cut the Idol's 2.375s native death short.
        time.sleep(2.6)
        for peer, mark in [(a, start), (b, observed)]:
            assert not any(e.get('event') == 'remove' and e['id'] == near for e in peer.events[mark:])
        for peer, mark in [(a, start), (b, observed)]:
            until(peer, mark, 'remove', lambda e: e['id'] == near, timeout=5)
        # Identical coordinates in a non-visible room never become a valid target.
        start = len(a.events)
        a.send('attack', id=hidden_id); time.sleep(.5)
        assert not any(e.get('event') == 'hit' for e in a.events[start:])
        # At 12-14m across a visible room, pursuit cannot reach the attacker.
        # The old melee-only engagement check healed every ranged hit immediately.
        for definition, target, visual, entity in [
            (0x470007ed, far, 0x1f000066, 0x470000be),
            (0x470007e0, magic_target, 0x1f0003ce, 0x47000393)]:
            item = next(e for e in a.events if e.get('event') == 'object' and
                        e.get('container') == a.id and e.get('entity') == definition)
            start = len(a.events)
            a.send('use', item=item['id'], target='0')
            ack = until(a, start, 'inventory_result', lambda e: e['id'] == item['id'])
            assert ack['status'] == 0 and ack['equipped'] == 0x40, ack
            if definition == 0x470007ed:
                cancel_start, cancel_observed = len(a.events), len(b.events)
                a.send('skill', skill=0xcf, target=target)
                shot = until(a, cancel_start, 'missile', lambda e: e['target'] == target)
                a.send('combat', enabled=False)
                until(a, cancel_start, 'combat', lambda e: not e['enabled'])
                for peer, mark in [(a, cancel_start), (b, cancel_observed)]:
                    until(peer, mark, 'remove', lambda e: e['id'] == shot['id'])
                time.sleep(.2)
                assert not any(e.get('event') == 'hit' and e.get('id') == target for e in a.events[cancel_start:])
            start, observed = len(a.events), len(b.events)
            a.send('attack', id=target)
            until(a, start, 'quality', lambda e: e.get('id') == target and e['key'] == 0x102 and e['value'] == 0)
            until(a, start, 'combat', lambda e: not e['enabled'])
            for peer, mark in [(a, start), (b, observed)]:
                until(peer, mark, 'quality', lambda e: e.get('id') == target and e['key'] == 0x102 and e['value'] == 0)
                health = [e['value'] for e in peer.events[mark:] if e.get('event') == 'quality' and e.get('id') == target and e['key'] == 0x102]
                assert len(health) >= 3 and all(x >= y for x, y in zip(health, health[1:])), health
                missile = until(peer, mark, 'missile', lambda e: e['target'] == target)
                spawn = until(peer, mark, 'object', lambda e: e['id'] == missile['id'])
                assert spawn['entity'] == entity and spawn['visual']['did'] == visual, spawn
                until(peer, mark, 'remove', lambda e: e['id'] == missile['id'])
                assert not any(e.get('event') == 'hit' and e.get('id') == a.id for e in peer.events[mark:])
            time.sleep(.2)
        report = {'result': 'PASS', 'directory': str(directory), 'checks': [
            'authenticated combat-button and attack-key range failures restore local/remote peace',
            'rejected attacks do not deal damage', 'weapon equips immediately after failed attack',
            'no-target stance toggle still works', 'melee damage, retaliation and death across visible room boundary',
            'both clients retain corpse through the full native death before removal',
            'overlapping hidden room is neither sent nor attackable',
            'bow and magic kill at 12-14m across visible room boundary without health resets or remote melee hits',
            'native arrow and magic missiles reach both clients and retire after flight']}
        report['checks'].append('stopping an airborne shot cancels its damage and removes it on both clients')
        (directory / 'result.json').write_text(json.dumps(report, indent=2))
        print(json.dumps(report), flush=True)
    finally:
        password = None
        for peer in peers:
            peer.close()
        for process, stop in reversed(processes):
            if process.poll() is None:
                stop.write_text('stop\n')
                try:
                    process.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    process.kill(); process.wait()
        log.close()


if __name__ == '__main__':
    main()
