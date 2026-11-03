"""Real UDP integration test against an isolated Pyrealium store (no public servers)."""
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
BRIDGE = ROOT / 'Network/Bridge/target/release/ac2-network-bridge.exe'
_GAME_SERVER = ROOT / 'target/game/release/pyrealium.exe'
SERVER = Path(os.environ.get('AC2_TEST_SERVER', _GAME_SERVER if _GAME_SERVER.is_file()
                             else ROOT / 'target/release/pyrealium.exe')).resolve()
PYTHON = os.sys.executable
DAT_DIRECTORY = Path(os.environ.get('AC2_DAT_DIRECTORY', ROOT / "Asheron's Call 2 - Legions")).resolve()


def prepare_test_content(directory):
    from prepare_content import prepare
    content = directory / 'world.tsv'
    prepare(DAT_DIRECTORY, content)
    # Existing isolated service fixtures append v3 rows. Names/v4 acceptance is
    # tested separately; keep fixture construction independent of that format.
    lines=content.read_text(encoding='utf-8').splitlines()
    if lines[0]=='pyrealium-world-v4':
        content.write_text('pyrealium-world-v3\n'+'\n'.join('\t'.join(line.split('\t')[:12]) for line in lines[1:])+'\n',encoding='utf-8')
    return content

class Peer:
    def __init__(self):
        self.p = subprocess.Popen([str(BRIDGE)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.PIPE, text=True, encoding='utf-8', creationflags=0x08000000)
        self.q = queue.Queue()
        self.events = []
        def read():
            for line in self.p.stdout:
                try:
                    event = json.loads(line)
                    self.events.append(event)
                    self.q.put(event)
                except ValueError:
                    pass
        threading.Thread(target=read, daemon=True).start()
    def send(self, command, **values):
        self.p.stdin.write(json.dumps(dict(command=command, **values)) + '\n')
        self.p.stdin.flush()
    def wait(self, event, predicate=lambda e: True, timeout=15):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            try:
                v = self.q.get(timeout=min(1, max(.01, end-time.monotonic())))
            except queue.Empty:
                continue
            if v.get('event') == event and predicate(v):
                return v
        raise AssertionError(f'Timeout waiting for {event}; recent events={self.events[-12:]}')
    def close(self):
        if self.p.poll() is None:
            self.send('quit')
            try: self.p.wait(timeout=5)
            except subprocess.TimeoutExpired: self.p.kill()

def main():
    data = ROOT / 'Saved/AC2NetworkTests' / uuid.uuid4().hex
    data.mkdir(parents=True)
    content = prepare_test_content(data)
    env = dict(os.environ, PYREALIUM_UNREAL_LEGIONS_DATA='1')
    log = (data / 'server.log').open('w', encoding='utf-8')
    server_args = [str(SERVER), 'serve', '--bind', '127.0.0.1:19270', '--account', 'local',
        '--data', str(data/'world'), '--game-data', str(DAT_DIRECTORY),
        '--content', str(content), '--training-kit','false',
        '--lifestones','false','--portals','false','--seconds','180']
    server = subprocess.Popen(server_args, env=env, stdout=log,stderr=log, creationflags=0x08000000)
    peers=[]
    try:
        time.sleep(1)
        assert server.poll() is None, (data/'server.log').read_text()
        for name in ['Network Alpha','Network Beta']:
            p=Peer();peers.append(p)
            p.send('connect',host='127.0.0.1',port=19270,account='local')
            p.wait('characters')
            p.send('create',name=name,start_area=0,race=1,sex=4096,physical=[[1,1],[2,1]])
            created=p.wait('created')
            assert created['character']['id']!='0',created
            p.id=created['character']['id']
            p.send('enter',id=p.id)
            p.wait('player')
            p.spawn=p.wait('object',lambda e:e['id']==p.id)
            p.send('ready')
            p.wait('status',lambda e:e['phase']=='world')
        a,b=peers
        a.wait('object',lambda e:e['id']==b.id)
        # Client sends one native movement message; the observer must receive it.
        pose=a.spawn['placement']; moved=list(pose['position']);moved[0]+=.5
        a.send('move',cell=pose['cell'],position=moved,heading=90,motion=[0,1,0])
        update=b.wait('position',lambda e:e['id']==a.id)
        assert abs(update['position'][0]-moved[0])<.01,update
        a.send('chat',text='AC2 network smoke',channel='say')
        b.wait('chat',lambda e:'AC2 network smoke' in str(e))
        target=next(e for e in a.events if e.get('event')=='object' and e.get('entity')==0x470007d1)
        a.send('attack',id=target['id'])
        a.wait('quality',lambda e:e.get('id')==target['id'] and e.get('key')==0x102 and e.get('value')==0,timeout=25)
        item=next((e for e in a.events if e.get('event')=='object' and e.get('container')==a.id),None)
        if item is None: item=a.wait('object',lambda e:e.get('container')==a.id)
        assert item['entity']!=0 and item['quantity']==1,item
        a.send('stop_attack')
        a.send('logout',id=a.id)
        b.wait('remove',lambda e:e['id']==a.id)
        a.wait('selection')
        a.send('enter',id=a.id);a.wait('player');a.wait('object',lambda e:e['id']==a.id);a.send('ready')
        a.wait('object',lambda e:e['id']==item['id'])
        b.wait('object',lambda e:e['id']==a.id)
        # Restart the process, replay the journals, and reconnect through real UDP.
        for p in peers: p.close()
        (data/'world/stop.request').write_text('stop\n')
        assert server.wait(timeout=10)==0
        server = subprocess.Popen(server_args, env=env, stdout=log,stderr=log, creationflags=0x08000000)
        time.sleep(1)
        assert server.poll() is None, (data/'server.log').read_text()
        p=Peer();peers.append(p)
        p.send('connect',host='127.0.0.1',port=19270,account='local')
        listing=p.wait('characters')
        assert {a.id,b.id}.issubset({c['id'] for c in listing['characters']}),listing
        p.send('enter',id=a.id);p.wait('player');p.wait('object',lambda e:e['id']==a.id);p.send('ready')
        restored=p.wait('object',lambda e:e['id']==item['id'])
        assert restored['container']==a.id and restored['quantity']==1,restored
        report={'result':'PASS','checks':['native UDP login','character creation','world entry','two-peer visibility','movement replication','server chat','mob kill','journaled loot grant','logout removal','re-entry with owned loot','server restart with persisted characters and loot'], 'directory':str(data)}
        (data/'result.json').write_text(json.dumps(report,indent=2))
        print(json.dumps(report))
    finally:
        for i,p in enumerate(peers):
            p.close()
            (data/f'peer-{i}.json').write_text(json.dumps(p.events,indent=2),encoding='utf-8')
        if server.poll() is None:
            (data/'world/stop.request').write_text('stop\n')
            try:server.wait(timeout=8)
            except subprocess.TimeoutExpired:server.kill()
        log.close()

if __name__=='__main__': main()
