"""Native UDP Flame Bolt casting against Legions DATs and disposable characters.

Verifies trained skill routing, weapon checks, remote story/FX, burn ticks,
and protected targets. Fixture positions are test data, not retail.
"""
import json
import os
import subprocess
import time
import uuid
from test_network import Peer, ROOT, SERVER, DAT_DIRECTORY, prepare_test_content
from test_gameplay import until


def main():
    directory = ROOT / 'Saved/AC2NetworkTests' / ('active-skills-' + uuid.uuid4().hex)
    directory.mkdir(parents=True)
    content = prepare_test_content(directory)
    source = next(line.split('\t') for line in content.read_text().splitlines()
                  if '\t470007d1\t' in line)
    ids = [str(0x8140000000030500 + n) for n in range(2)]
    with content.open('a') as out:
        for iid, x in zip(ids, [60.0, 62.0]):
            row = source[:]
            row[0], row[3] = f'{int(iid):016x}', '02990109'
            row[4:7] = [str(x), '240.199', '-44.606205']
            out.write('\t'.join(row) + '\n')
    log = (directory / 'server.log').open('w', encoding='utf-8')
    process = subprocess.Popen([str(SERVER), 'serve', '--bind', '127.0.0.1:19520',
        '--account', 'local', '--data', str(directory / 'world'), '--game-data', str(DAT_DIRECTORY),
        '--content', str(content), '--initialize-ui', 'true', '--training-kit', 'false',
        '--lifestones', 'false', '--portals', 'false', '--seconds', '100'],
        stdout=log, stderr=log, env=dict(os.environ),
        creationflags=0x08000000)
    peers = []
    try:
        time.sleep(1)
        assert process.poll() is None, (directory / 'server.log').read_text()
        for name in ['Flame Tester', 'Flame Observer']:
            p = Peer(); peers.append(p)
            p.send('connect', host='127.0.0.1', port=19520, account='local'); p.wait('characters')
            p.send('create', name=name, start_area=0, race=1, sex=4096, physical=[[1, 1], [2, 1]])
            p.id = p.wait('created')['character']['id']
            p.send('enter', id=p.id); p.wait('player')
            p.send('ready'); p.wait('status', lambda e: e['phase'] == 'world')
            until(p, 0, 'inventory', lambda e: len(e.get('items', [])) >= 3)
        hero, observer = peers
        until(hero, 0, 'object', lambda e: e['id'] == ids[1])
        # Earn a skill credit through normal combat (level 2 is 40 XP).
        mark = len(hero.events)
        hero.send('attack', id=ids[0])
        until(hero, mark, 'quality', lambda e: e.get('id') == ids[0] and e['key'] == 0x102 and e['value'] == 0)
        hero.send('combat', enabled=False)
        until(hero, mark, 'combat', lambda e: not e['enabled'])
        hero.send('skill_train', skill=0x59)
        time.sleep(.3)
        mark = len(hero.events)
        hero.send('skill', skill=0x59, target=ids[1])
        until(hero, mark, 'chat', lambda e: 'requires a stave' in str(e))
        until(hero, mark, 'skill_finished')
        staff = next(e for e in hero.events if e.get('event') == 'object'
                     and e.get('container') == hero.id and e.get('entity') == 0x470007e0)
        mark = len(hero.events)
        hero.send('use', item=staff['id'], target='0')
        until(hero, mark, 'inventory_result', lambda e: e['id'] == staff['id'] and e['status'] == 0 and e['equipped'] == 0x40)
        mark = len(hero.events)
        hero.send('skill', skill=0x59, target=observer.id)
        until(hero, mark, 'chat', lambda e: 'Select a creature' in str(e))
        until(hero, mark, 'skill_finished')
        # A spell can miss; retry once its DAT cooldown expires, never fabricate a hit.
        for attempt in range(5):
            mark, remote = len(hero.events), len(observer.events)
            hero.send('skill', skill=0x59, target=ids[1])
            until(hero, mark, 'skill_finished')
            until(observer, remote, 'behavior', lambda e: e['id'] == hero.id and e['behavior'] == 0x410000b6)
            until(observer, remote, 'fx', lambda e: e['id'] == ids[1] and e['fx'] == 0x41000241)
            try:
                until(hero, mark, 'hit', lambda e: e['id'] == ids[1] and e['damage'] > 0, timeout=2)
                break
            except AssertionError:
                if attempt == 4:
                    raise
        time.sleep(2.2)
        hits = [e for e in hero.events[mark:] if e.get('event') == 'hit' and e['id'] == ids[1]]
        assert len(hits) >= 3, hits  # one impact plus at least two burn ticks
        assert hits[0]['damage'] > 3 and all(hit['damage'] == 1 for hit in hits[1:]), hits
        assert not any('not available' in str(e).lower() for e in hero.events[mark:]), hero.events[mark:]
        result = {'native_flame_bolt_story_and_fx': True, 'burn_ticks': len(hits) - 1,
                  'trained_skill_and_weapon_checks': True, 'player_target_protected': True}
        (directory / 'result.json').write_text(json.dumps(result, indent=2))
        print(json.dumps(result)); print(directory)
    finally:
        for index, peer in enumerate(peers):
            (directory / f'peer-{index}.json').write_text(json.dumps(peer.events, indent=2))
            peer.close()
        if process.poll() is None:
            (directory / 'world/stop.request').write_text('stop\n')
            process.wait(timeout=10)
        log.close()


if __name__ == '__main__':
    main()
