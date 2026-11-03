"""Starter armor replication for all six supported bodies, using an isolated store."""
import json
import os
import subprocess
import time
import uuid
from test_network import Peer, ROOT, SERVER, DAT_DIRECTORY, prepare_test_content
from test_gameplay import until


def main():
    run=ROOT/'Saved/AC2NetworkTests'/('armor-'+uuid.uuid4().hex)
    run.mkdir(parents=True)
    content=prepare_test_content(run)
    log=(run/'server.log').open('w',encoding='utf-8')
    server=subprocess.Popen([str(SERVER),'serve','--bind','127.0.0.1:19350','--account','local',
        '--data',str(run/'world'),'--game-data',str(DAT_DIRECTORY),'--content',str(content),
        '--initialize-ui','true','--training-kit','false','--seconds','240'],
        env=dict(os.environ),stdout=log,stderr=log,creationflags=0x08000000)
    peers=[];checks=[]
    try:
        time.sleep(1)
        assert server.poll() is None,(run/'server.log').read_text()
        for race in [1,2,4]:
            pair=[]
            for sex,role in [(4096,'Man'),(8192,'Woman')]:
                name={1:'Human',2:'Lugian',4:'Tumerok'}[race]+' '+role
                p=Peer();peers.append(p);pair.append(p)
                p.send('connect',host='127.0.0.1',port=19350,account='local');p.wait('characters')
                p.send('create',name=name,start_area=0,race=race,sex=sex,physical=[[1,1],[2,1],[0x800,0.25],[0x100,0.5]])
                response=p.wait('created');p.id=response['character']['id'];assert p.id!='0',(race,sex,response)
                p.send('enter',id=p.id);p.wait('player');p.spawn=p.wait('object',lambda e:e['id']==p.id)
                p.send('ready');p.wait('status',lambda e:e['phase']=='world')
                p.inventory=until(p,0,'inventory');p.bag=list(p.inventory['items'])
            a,b=pair
            until(a,0,'object',lambda e:e['id']==b.id)
            for p,observer in [(a,b),(b,a)]:
                original=p.spawn['visual']
                slots=[2,8] if race==4 else [2,8,16] # Tumerok starter kit has no boots.
                for slot in slots:
                    item=next(i['id'] for i in p.inventory['equipment'] if i['slot']==slot)
                    start=len(p.events);obs=len(observer.events)
                    time.sleep(.15);p.send('unequip',id=item,location=slot,slot=len(p.bag))
                    ack=until(p,start,'inventory_result',lambda e:e['id']==item);assert ack['status']==0 and ack['equipped']==0
                    p.bag.insert(ack['slot'],item)
                    bare=until(p,start,'visual',lambda e:e['id']==p.id)['visual']
                    assert bare!=original and bare==until(observer,obs,'visual',lambda e:e['id']==p.id)['visual']
                    # Only the relevant worn flag may change; physique/colour stay intact.
                    differences=[(x,y) for x,y in zip(original['aspects'],bare['aspects']) if x!=y]
                    assert len(differences)==1 and differences[0][0]['key']==16
                    assert differences[0][0]['value']==1 and differences[0][1]['value']==0
                    start=len(p.events);obs=len(observer.events)
                    time.sleep(.15);p.send('equip',id=item,location=slot,slot=p.bag.index(item))
                    ack=until(p,start,'inventory_result',lambda e:e['id']==item);assert ack['status']==0 and ack['equipped']==slot
                    p.bag.remove(item)
                    assert until(p,start,'visual',lambda e:e['id']==p.id)['visual']==original
                    assert until(observer,obs,'visual',lambda e:e['id']==p.id)['visual']==original
                checks.append({'race':race,'body':p.spawn['visual']['did'],'slots':slots})
            for p in pair:
                start=len(p.events);p.send('logout',id=p.id);until(p,start,'selection')
                p.send('delete',id=p.id);p.wait('deleted');p.close()
        result={'result':'PASS','checks':checks,'directory':str(run)}
        (run/'result.json').write_text(json.dumps(result,indent=2));print(json.dumps(result,indent=2))
    finally:
        for p in peers:p.close()
        if server.poll() is None:(run/'world/stop.request').write_text('stop\n');server.wait(timeout=10)
        log.close()


if __name__=='__main__':main()
