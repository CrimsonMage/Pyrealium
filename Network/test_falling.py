"""Real UDP: DAT-grounded fall damage, once per landing, persisted across login.

Movement is a synthetic trajectory in a disposable world; this does not claim
retail damage-formula parity or verify interior/building collision.
"""
import json
import os
import subprocess
import time
import uuid
from test_network import Peer, ROOT, SERVER, DAT_DIRECTORY, prepare_test_content
from test_gameplay import until


def main():
    data = ROOT / 'Saved/AC2NetworkTests' / ('falling-' + uuid.uuid4().hex)
    data.mkdir(parents=True)
    content = prepare_test_content(data)
    rules=data/'encounters.tsv'
    rules.write_text((ROOT/'emulator/content/development-encounters.tsv').read_text().replace('regen\t2\t4\t1000','regen\t0\t0\t1000'))
    log = (data/'server.log').open('w')
    server = subprocess.Popen([str(SERVER), 'serve', '--bind', '127.0.0.1:19392',
        '--account', 'local', '--data', str(data/'world'), '--game-data', str(DAT_DIRECTORY),
        '--content', str(content), '--encounters', str(rules), '--initialize-ui', 'true', '--training-kit', 'false',
        '--test-map-travel', 'true', '--seconds', '90'],
        env=dict(os.environ, PYREALIUM_UNREAL_LEGIONS_DATA='1'), stdout=log, stderr=log,
        creationflags=0x08000000)
    p = Peer()
    try:
        time.sleep(1)
        assert server.poll() is None
        p.send('connect', host='127.0.0.1', port=19392, account='local'); p.wait('characters')
        p.send('create', name='Fall Tester', start_area=0, race=1, sex=4096, physical=[[1,1],[2,1]])
        identity = p.wait('created')['character']['id']
        p.send('enter', id=identity); p.wait('player'); p.send('ready'); p.wait('status', lambda e:e['phase']=='world')
        p.send('chat', text='@maptravel 692.25 828.5', channel='say')
        pose = p.wait('position', lambda e:e['id']==identity)
        x,y,z = pose['position']; cell = pose['cell']
        def move(height):
            p.send('move', cell=cell, position=[x,y,height], heading=0, motion=[0,0,0])
            time.sleep(.11)
        # Rising clears a previous fall; a 3 m ordinary jump must be harmless.
        mark = len(p.events)
        for dz in [0,1,2,3,2,1,0,0]: move(z+dz)
        assert not any('fall damage' in str(e) for e in p.events[mark:])
        # No damage in the air; apply once when the received feet touch DAT ground.
        for i in range(1,19): move(z+i)
        mark = len(p.events)
        for i in range(9,0,-1): move(z+i*2)
        assert not any('fall damage' in str(e) for e in p.events[mark:])
        move(z)
        notice = until(p,mark,'chat',lambda e:'fall damage' in str(e))
        health = until(p,mark,'quality',lambda e:e.get('key')==0x102)['value']
        assert 0 < int(health) < 100, health
        for _ in range(6): move(z)
        assert sum('fall damage' in str(e) for e in p.events[mark:]) == 1
        p.send('logout', id=identity); p.wait('selection')
        mark=len(p.events);p.send('enter', id=identity);until(p,mark,'player')
        p.send('ready');until(p,mark,'status',lambda e:e['phase']=='world')
        restored=until(p,mark,'quality',lambda e:e.get('key')==0x102)['value']
        assert int(restored)==int(health),(restored,health)
        result={'result':'PASS','health_after_fall':health,'notice':notice,'directory':str(data)}
        (data/'result.json').write_text(json.dumps(result,indent=2));print(json.dumps(result))
    finally:
        p.close()
        if server.poll() is None:
            (data/'world/stop.request').write_text('stop\n');server.wait(timeout=10)
        log.close()


if __name__ == '__main__': main()
