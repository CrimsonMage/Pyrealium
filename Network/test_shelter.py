"""Shelter tool/glyph/Menhir path over UDP, with isolated authored test fixtures.
Verifies rules and replication; does not claim recovered retail spawn coordinates.
"""
import json, math, os, subprocess, time, uuid
from test_network import Peer, ROOT, SERVER, DAT_DIRECTORY, prepare_test_content
from test_gameplay import until

def main():
    run=ROOT/'Saved/AC2NetworkTests'/('shelter-'+uuid.uuid4().hex);run.mkdir(parents=True)
    content=prepare_test_content(run)
    with content.open('a') as f:
        for n,e,v,x,y in [(0,0x157d,0xd74,58,239),(1,0x157e,0xd75,58,238),(2,0x1581,0xd78,60,238)]:
            f.write(f'{0x8140000000030200+n:016x}\t{0x47000000+e:08x}\t{0x1f000000+v:08x}\t02990109\t{x}\t{y}\t-44.606205\t1\t0\t0\t0\t-\n')
        for n in range(3):
            f.write(f'{0x8140000000030210+n:016x}\t4700158b\t1f000036\t0299011d\t99.63357\t242.79227\t-44.962006\t1\t0\t0\t0\t-\n')
    log=(run/'server.log').open('w');env=dict(os.environ,PYREALIUM_UNREAL_LEGIONS_DATA='1')
    args=[str(SERVER),'serve','--bind','127.0.0.1:19360','--account','local','--data',str(run/'world'),'--game-data',str(DAT_DIRECTORY),'--content',str(content),'--initialize-ui','true','--training-kit','false','--portals','true','--seconds','240']
    server=subprocess.Popen(args,env=env,stdout=log,stderr=log,creationflags=0x08000000);peers=[];result={}
    try:
        time.sleep(1);assert server.poll() is None,(run/'server.log').read_text()
        for name in ['Shelter Runner','Shelter Observer']:
            p=Peer();peers.append(p);p.send('connect',host='127.0.0.1',port=19360,account='local');p.wait('characters')
            p.send('create',name=name,race=1,sex=4096,start_area=0,physical=[[1,1],[2,1]])
            p.id=p.wait('created')['character']['id'];p.send('enter',id=p.id);p.wait('player')
            p.spawn=p.wait('object',lambda e:e['id']==p.id);p.send('ready');p.wait('status',lambda e:e['phase']=='world')
            p.inventory=until(p,0,'inventory');p.bag=list(p.inventory['items']);p.pos=list(p.spawn['placement']['position'])
        a,b=peers
        def use(i,target='0'):
            time.sleep(1.1);start=len(a.events);a.send('use',item=str(i),target=str(target));return start
        for n,table,key in [(2,0x2500164c,0x8b6b186d),(0,0x25001648,0x2824fd4c),(1,0x25001649,0x8b6b186f),(2,0x2500164c,0x8b6b1869)]:
            start=use(0x8140000000030200+n);until(a,start,'chat',lambda e:e['text']['table']==table and e['text']['key']==key)
        def move(p,dest,cell):
            old=p.pos[:];steps=max(1,math.ceil(math.dist(old,dest)/.5))
            for i in range(1,steps+1):
                p.pos=[x+(y-x)*i/steps for x,y in zip(old,dest)]
                p.send('move',cell=cell,position=p.pos,heading=90,motion=[0,1,0]);time.sleep(.1)
            p.send('move',cell=cell,position=p.pos,heading=90,motion=[0,0,0])
        for p in peers:move(p,[98.63357,242.79227,-44.962006],0x0299011d)
        sword=next(e for e in a.events if e.get('event')=='object' and e.get('container')==a.id and e['entity']==0x470007ee)
        start=len(a.events);a.send('equip',id=sword['id'],location=0x40,slot=a.bag.index(sword['id']));until(a,start,'inventory_result',lambda e:e['status']==0);a.bag.remove(sword['id'])
        def kill(i,definition):
            start=len(a.events);a.send('attack',id=str(i))
            item=until(a,start,'object',lambda e:e.get('container')==a.id and e.get('entity')==definition)
            until(a,start,'inventory_result',lambda e:e['id']==item['id']);a.bag.append(item['id'])
            a.send('combat',enabled=False);time.sleep(.3);return item
        tool=kill(0x8140000000030043,0x47001587)
        until(a,0,'behavior',lambda e:e['id']==str(0x8140000000030043) and e['behavior']==0x4100001f)
        # A remote Scout cannot consume the tool.
        start=use(0x8140000000030044);until(a,start,'chat',lambda e:'four' in str(e).lower())
        move(a,[94.63357,241.79227,-44.962006],0x0299011d)
        start=use(0x8140000000030044);until(a,start,'chat',lambda e:e['text']['table']==0x2500164b and e['text']['key']==0x11ded2d4)
        until(a,start,'inventory_remove',lambda e:e['id']==tool['id'])
        a.bag.remove(tool['id'])
        for d in [0x47001583,0x47001584,0x47001585]:a.bag.append(until(a,start,'object',lambda e:e.get('container')==a.id and e['entity']==d)['id'])
        start=use(0x8140000000030044);until(a,start,'chat',lambda e:e['text']['key']==0x8b6b1867)
        assert not any(e.get('event')=='object' and e.get('container')==a.id for e in a.events[start:])
        result['tool_consumed_once_three_native_potions']=True
        move(a,[98.63357,242.79227,-44.962006],0x0299011d)
        # Additional Drudges grant the three race-specific armor profiles.
        for n,d,slot,aspect in [(0,0x47001951,16,0x20000118),(1,0x47001952,8,0x20000068),(2,0x47001954,2,0x20000064)]:
            item=kill(0x8140000000030210+n,d)
            old=next(e['id'] for e in a.inventory['equipment'] if e['slot']==slot)
            source=a.bag.index(item['id'])
            start=len(a.events);obs=len(b.events);a.send('equip',id=item['id'],location=slot,slot=a.bag.index(item['id']))
            ack=until(a,start,'inventory_result',lambda e:e['id']==item['id']);assert ack['status']==0,ack
            displaced=until(a,start,'inventory_result',lambda e:e['id']==old)
            assert displaced['status']==0 and displaced['equipped']==0 and displaced['slot']==source,displaced
            local=until(a,start,'visual',lambda e:e['id']==a.id)['visual'];remote=until(b,obs,'visual',lambda e:e['id']==a.id)['visual'];assert local==remote
            assert any(t['table']==aspect and t['key']==16 and t['value']==1 for t in local['aspects']),local
            # Equip replaces the slot; rebuild the dense bag from authoritative results.
            a.bag.remove(item['id'])
            for e in a.events[start:]:
                if e.get('event')=='inventory_result' and not e.get('equipped') and e['status']==0 and e['id'] not in a.bag:a.bag.insert(e['slot'],e['id'])
        result['additional_armor_visible_on_owner_and_observer']=True
        result['armor_replacement_returns_old_item_to_source_slot']=True
        # Dense bag sorting must report the final index, not the pre-removal boundary.
        for source,destination in [(0,len(a.bag)),(len(a.bag)-1,0)]:
            time.sleep(.2);iid=a.bag[source];start=len(a.events)
            a.send('move_item',id=iid,**{'from':source,'to':destination,'quantity':1})
            ack=until(a,start,'inventory_result',lambda e:e['id']==iid)
            final=destination-int(source<destination)
            assert ack['status']==0 and ack['slot']==final,ack
            a.bag.pop(source);a.bag.insert(final,iid)
        result['dense_bag_reorder_both_directions']=True
        glyph=kill(0x8140000000030045,0x47001582)
        start=use(glyph['id'],0x8140000000030044);until(a,start,'chat',lambda e:'Target the Menhir Portal' in str(e))
        start=use(glyph['id'],0x8140000000030047)
        arrival=until(a,start,'position',lambda e:e['id']==a.id and e['cell']==0x03f40154)
        assert math.dist(arrival['position'],[176,62,15.32])<.01,arrival
        until(a,start,'inventory_remove',lambda e:e['id']==glyph['id'])
        a.pos=list(arrival['position']);a.bag.remove(glyph['id'])
        scout=until(a,start,'object',lambda e:e.get('entity')==0x4700157f and e.get('placement',{}).get('cell')==0x03f40154)
        start=use(scout['id']);until(a,start,'chat',lambda e:e['text']['key']==0x8b6b1863)
        # Approach the shard from the clear side instead of spawning inside it.
        move(a,[178,64,15.25],0x03f40154)
        start=use(0x8140000000030048);until(a,start,'chat',lambda e:'Shelter Hold complete' in str(e))
        result['glyph_target_checked_consumed_and_vault_completed']=True
        # Simulate a character saved at the previous obstructed arrival.
        move(a,[178.730484,62.94236,15.121846],0x03f40154)
        hero=a.id
        for p in peers:p.close()
        (run/'world/stop.request').write_text('stop');assert server.wait(timeout=10)==0
        server=subprocess.Popen(args,env=env,stdout=log,stderr=log,creationflags=0x08000000);time.sleep(1)
        p=Peer();peers.append(p);p.send('connect',host='127.0.0.1',port=19360,account='local');p.wait('characters');p.send('enter',id=hero);p.wait('player');p.send('ready')
        spawn=until(p,0,'object',lambda e:e['id']==hero);assert spawn['placement']['cell']==0x03f40154
        assert math.dist(spawn['placement']['position'],[176,62,15.32])<.01,spawn
        result['legacy_obstructed_arrival_recovered']=True
        p.send('chat',channel='say',text='@quest shelter');until(p,0,'chat',lambda e:'Shelter Hold complete' in str(e))
        result['completion_inventory_and_vault_survive_restart']=True
        (run/'result.json').write_text(json.dumps(result,indent=2));print(json.dumps(result,indent=2));print(run)
    finally:
        for n,p in enumerate(peers):
            (run/f'peer-{n}.json').write_text(json.dumps(p.events,indent=2));p.close()
        if server.poll() is None:(run/'world/stop.request').write_text('stop');server.wait(timeout=10)
        log.close()

if __name__=='__main__':main()
