"""Real UDP: whole-item world drops, ownership, observers and restart recovery."""
import json
import os
import subprocess
import time
import uuid
from test_network import Peer, ROOT, SERVER, DAT_DIRECTORY, prepare_test_content
from test_gameplay import until


def main():
    data = ROOT / 'Saved/AC2NetworkTests' / ('world-drops-' + uuid.uuid4().hex)
    data.mkdir(parents=True)
    content = prepare_test_content(data)
    args = [str(SERVER), 'serve', '--bind', '127.0.0.1:19482', '--account', 'local',
            '--data', str(data/'world'), '--game-data', str(DAT_DIRECTORY), '--content', str(content),
            '--initialize-ui', 'true', '--training-kit', 'false', '--seconds', '150']
    peers = []
    log = (data/'server.log').open('w')
    def start():
        server = subprocess.Popen(args, env=dict(os.environ),
                                  stdout=log, stderr=log, creationflags=0x08000000)
        time.sleep(1)
        assert server.poll() is None
        return server
    def login(name=None, identity=None):
        p = Peer(); peers.append(p)
        p.send('connect', host='127.0.0.1', port=19482, account='local'); p.wait('characters')
        if name:
            p.send('create', name=name, start_area=0, race=1, sex=4096, physical=[[1,1],[2,1]])
            identity = p.wait('created')['character']['id']
        p.id = identity
        p.send('enter', id=identity);p.wait('player')
        p.spawn = p.wait('object', lambda e:e['id']==identity)
        p.send('ready');p.wait('status', lambda e:e['phase']=='world')
        p.bag = until(p,0,'inventory')['items']
        return p
    def stop(server):
        for p in peers:p.close()
        peers.clear()
        (data/'world/stop.request').write_text('stop\n')
        server.wait(timeout=10)
    server = start()
    try:
        a,b = login('Drop Owner'),login('Drop Observer')
        aid,bid = a.id,b.id
        original = a.bag[0]
        object_item = until(a,0,'object',lambda e:e['id']==original)
        # A foreign IID and a stale source must not remove anything.
        for item,source in [(b.bag[0],0),(original,77)]:
            mark=len(a.events)
            a.send('drop_item',id=item,**{'from':source},quantity=1,landblock=0x02990000)
            until(a,mark,'chat',lambda e:'Drop rejected' in str(e))
        mark=len(a.events);obs=len(b.events)
        a.send('drop_item',id=original,**{'from':0},quantity=object_item['quantity'],landblock=0x02990000)
        until(a,mark,'inventory_remove',lambda e:e['id']==original)
        dropped=until(a,mark,'object',lambda e:int(e['id'])>>48==0x8180)
        ground=dropped['id']
        assert until(b,obs,'object',lambda e:e['id']==ground)['placement']==dropped['placement']
        assert dropped['entity']==object_item['entity']
        # It must survive the dropping account disconnecting and the server restarting.
        stop(server);server=start()
        a,b=login(identity=aid),login(identity=bid)
        assert original not in a.bag
        until(b,0,'object',lambda e:e['id']==ground)
        mark=len(b.events);obs=len(a.events)
        b.send('use',target='0',item=ground)
        picked=until(b,mark,'object',lambda e:e.get('container')==bid and e.get('entity')==object_item['entity'])
        assert picked['id']!=original
        until(b,mark,'chat',lambda e:'Item picked up' in str(e))
        until(a,obs,'remove',lambda e:e['id']==ground)
        time.sleep(.15);mark=len(a.events)
        a.send('use',target='0',item=ground)
        until(a,mark,'chat',lambda e:'Pickup rejected' in str(e))
        stop(server);server=start()
        a,b=login(identity=aid),login(identity=bid)
        assert picked['id'] in b.bag
        assert not any(e.get('event')=='object' and e.get('id')==ground for e in a.events+b.events)
        report={'result':'PASS','directory':str(data),'checks':['foreign/stale drops rejected',
            'both observers see dropped model','drop survives restart','other player picks up once',
            'pickup and ownership survive restart']}
        (data/'result.json').write_text(json.dumps(report,indent=2));print(json.dumps(report))
    finally:
        if server.poll() is None:stop(server)
        log.close()


if __name__=='__main__':main()
