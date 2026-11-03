"""Door replication and native merchant transactions over real UDP, isolated store."""
import json
import os
import subprocess
import time
import uuid
from test_network import ROOT, SERVER, DAT_DIRECTORY, Peer, prepare_test_content
from test_gameplay import until

VENDOR = str(0x8140000000030900)
DOOR = str(0x8140000000030901)
LOCKED = str(0x8140000000030902)
CHEST = str(0x8140000000030903)
SIGN = str(0x8140000000030904)


def fixture(directory, isolated=False):
    content = prepare_test_content(directory)
    if isolated:
        content.write_text('pyrealium-world-v3\n')
    # Only test fixtures: native assets, deliberately placed within reach of spawn.
    with content.open('a') as f:
        for iid, entity, visual, x, y in [(VENDOR,0x47003724,0x1f001941,60,241),
                (DOOR,0x4700114a,0x1f00006c,57,241),(LOCKED,0x4700174e,0x1f00034b,59,242),(CHEST,0x4700154b,0x1f000dd5,58,241)]:
            f.write(f'{int(iid):016x}\t{entity:08x}\t{visual:08x}\t02990109\t{x}\t{y}\t-44.606205\t1\t0\t0\t0\t-\n')
    return content


def main():
    run=ROOT/'Saved/AC2NetworkTests'/('services-'+uuid.uuid4().hex)
    run.mkdir(parents=True)
    content=fixture(run,isolated=True)
    with content.open('a') as f:
        f.write(f'{int(SIGN):016x}\t47001382\t1f0002f4\t02990109\t58\t242\t-44.606205\t1\t0\t0\t0\t-\n')
    rows=content.read_text().splitlines()
    content.write_text('pyrealium-world-v4\n'+'\n'.join(line+'\t'+('Captured Door Name' if line.startswith(f'{int(DOOR):016x}') else '-') for line in rows[1:])+'\n')
    peers=[]
    with (run/'server.log').open('w') as log:
        server=subprocess.Popen([str(SERVER),'serve','--bind','127.0.0.1:19386','--account','local',
            '--data',str(run/'world'),'--game-data',str(DAT_DIRECTORY),'--content',str(content),
            '--initialize-ui','true','--training-kit','false','--encounters','none','--seconds','100'],
            env=dict(os.environ),stdout=log,stderr=log,creationflags=0x08000000)
        try:
            time.sleep(1);assert server.poll() is None,(run/'server.log').read_text()
            def join(name):
                p=Peer();peers.append(p);p.send('connect',host='127.0.0.1',port=19386,account='local');p.wait('characters')
                p.send('create',name=name,race=1,sex=4096,start_area=0,physical=[[1,1],[2,1]])
                p.id=p.wait('created')['character']['id'];p.send('enter',id=p.id);p.wait('player')
                p.spawn=p.wait('object',lambda e:e['id']==p.id);p.send('ready');p.wait('status',lambda e:e['phase']=='world')
                until(p,0,'inventory');return p
            a=join('Service Tester');b=join('Service Observer')
            def inspect(target):
                time.sleep(.3);mark=len(a.events);a.send('examine',id=target)
                view=until(a,mark,'examination',lambda e:e['id']==target)
                return '\n'.join(n.get('text',{}).get('literal') or '' for n in view['nodes'])
            own=inspect(a.id)
            assert all(s in own for s in ['Service Tester','Human','Health:','Vigor:','Experience:','Available skill credits:']),own
            other=inspect(b.id)
            assert all(s in other for s in ['Service Observer','Human','Health:']),other
            assert 'Experience:' not in other and 'Gold:' not in other,other
            assert 'Vendor.' in inspect(VENDOR)
            assert 'Locked door.' in inspect(LOCKED)
            door=inspect(DOOR);assert 'Captured Door Name' in door and 'Door.' in door,door
            assert 'Chest.' in inspect(CHEST)
            assert 'shores of Lake Blessed' in inspect(SIGN)
            ma=len(a.events);a.send('use',item=SIGN,target='0')
            until(a,ma,'chat',lambda e:e.get('text',{}).get('key')==0x6c459364 and e['text'].get('table')==0x250014c9)
            assert not any('Additional scripted' in str(e) for e in a.events[ma:])
            owned_weapon=next(e for e in a.events if e.get('event')=='object' and e.get('container')==a.id and e.get('entity')==0x470007ed)
            gear=inspect(owned_weapon['id']);assert 'Quantity:' in gear and 'Main hand' in gear,gear
            named=until(a,0,'object',lambda e:e['id']==DOOR)
            assert named['name']['literal']=='Captured Door Name',named
            ma,mb=len(a.events),len(b.events);a.send('use',item=CHEST,target='0')
            opened=until(a,ma,'container_state',lambda e:e['id']==CHEST and e['open'])
            assert opened['items']==[],opened
            until(a,ma,'chat',lambda e:'no configured contents' in str(e))
            until(a,ma,'mode',lambda e:e['id']==CHEST and e['mode']==0x41000002)
            until(b,mb,'mode',lambda e:e['id']==CHEST and e['mode']==0x41000002)
            mb=len(b.events);b.send('use',item=CHEST,target='0')
            until(b,mb,'container_state',lambda e:e['id']==CHEST and e['open'])
            ma=len(a.events);a.send('close_container',id=CHEST)
            until(a,ma,'container_state',lambda e:not e['open'])
            time.sleep(.2)
            assert not any(e.get('event')=='mode' and e.get('id')==CHEST and e.get('mode')==0x41000003 for e in a.events[ma:]),'First viewer closed a shared chest'
            ma,mb=len(a.events),len(b.events);b.send('logout',id=b.id);until(b,mb,'selection')
            until(a,ma,'mode',lambda e:e['id']==CHEST and e['mode']==0x41000003)
            ma=len(a.events);a.send('use',item=CHEST,target='0')
            until(a,ma,'container_state',lambda e:e['id']==CHEST and e['open'])
            mb=len(b.events);b.send('enter',id=b.id);until(b,mb,'player');b.send('ready')
            until(b,mb,'mode',lambda e:e['id']==CHEST and e['mode']==0x41000002)
            ma,mb=len(a.events),len(b.events)
            a.send('use',item=DOOR,target='0')
            until(a,ma,'mode',lambda e:e['id']==DOOR and e['mode']==0x41000002)
            until(b,mb,'mode',lambda e:e['id']==DOOR and e['mode']==0x41000002)
            # A late observer receives the current state after CreateObject.
            mb=len(b.events);b.send('logout',id=b.id);until(b,mb,'selection')
            mb=len(b.events);b.send('enter',id=b.id);until(b,mb,'player');b.send('ready')
            until(b,mb,'mode',lambda e:e['id']==DOOR and e['mode']==0x41000002)
            time.sleep(.8);ma=len(a.events);a.send('use',item=DOOR,target='0')
            until(a,ma,'mode',lambda e:e['id']==DOOR and e['mode']==0x41000003)
            time.sleep(.8);ma=len(a.events);a.send('use',item=LOCKED,target='0')
            until(a,ma,'chat',lambda e:'locked' in str(e))
            ma=len(a.events);a.send('use',item=VENDOR,target='0')
            shop=until(a,ma,'shop')
            # Profession materials extend the original four-item stock. Check
            # usable, distinct offer identities instead of the obsolete size.
            assert len(shop['offers'])>=4
            assert len({o['id'] for o in shop['offers']})==len(shop['offers'])
            offer=next(o for o in shop['offers'] if o['entity']==0x47001744)
            time.sleep(.3);ma=len(a.events);a.send('shop_buy',vendor=VENDOR,offer=offer['id'])
            until(a,ma,'chat',lambda e:'enough gold' in str(e))
            bag=next(e['items'] for e in reversed(a.events) if e.get('event')=='inventory')
            weapon=next(e for e in a.events if e.get('event')=='object' and e.get('id') in bag and e.get('entity')==0x470007ed)
            ma=len(a.events);a.send('shop_sell',item=weapon['id'],slot=bag.index(weapon['id']),quantity=1)
            until(a,ma,'inventory_remove',lambda e:e['id']==weapon['id'])
            until(a,ma,'chat',lambda e:'Sold equipment for 3 gold' in str(e))
            time.sleep(.3);ma=len(a.events);a.send('shop_buy',vendor=VENDOR,offer=offer['id'])
            acquired=until(a,ma,'object',lambda e:e.get('entity')==0x47001744 and e.get('container')==a.id)
            until(a,ma,'chat',lambda e:'Bought mushroom for 2 gold' in str(e))
            time.sleep(.3);ma=len(a.events);a.send('shop_close',vendor=VENDOR);until(a,ma,'shop_close')
            a.send('shop_buy',vendor=VENDOR,offer=offer['id'])
            until(a,ma,'chat',lambda e:'before buying' in str(e))
            # New shop offer identities invalidate the previous catalog.
            time.sleep(.3);ma=len(a.events);a.send('use',item=VENDOR,target='0');new=until(a,ma,'shop')
            assert new['offers'][0]['id']!=shop['offers'][0]['id']
            time.sleep(.3);ma=len(a.events);a.send('shop_buy',vendor=VENDOR,offer=offer['id'])
            until(a,ma,'chat',lambda e:'supported stock' in str(e))
            # Ownership and combat are revalidated at transaction time.
            ma=len(b.events);b.send('use',item=VENDOR,target='0');until(b,ma,'shop');time.sleep(.3)
            mb=len(b.events);b.send('shop_sell',item=acquired['id'],slot=0,quantity=1)
            until(b,mb,'chat',lambda e:'invalid' in str(e).lower() or 'owned' in str(e))
            ma=len(a.events);a.send('combat',enabled=True);until(a,ma,'combat',lambda e:e['enabled'])
            a.send('shop_buy',vendor=VENDOR,offer=new['offers'][0]['id'])
            until(a,ma,'chat',lambda e:'combat' in str(e).lower() or 'before buying' in str(e))
            a.send('combat',enabled=False)
            until(a,ma,'combat',lambda e:not e['enabled'])
            pose=a.spawn['placement'];position=list(pose['position']);position[0]+=12
            a.send('move',cell=pose['cell'],position=position,heading=0,motion=[0,1,0])
            until(a,ma,'container_state',lambda e:e['id']==CHEST and not e['open'])
            until(b,mb,'mode',lambda e:e['id']==CHEST and e['mode']==0x41000003)
            time.sleep(.3);ma=len(a.events);a.send('use',item=DOOR,target='0')
            until(a,ma,'chat',lambda e:'four metres' in str(e))
            ma=len(a.events);a.send('use',item=SIGN,target='0')
            until(a,ma,'chat',lambda e:'four metres' in str(e))
            ma=len(a.events);a.send('use',item=VENDOR,target='0')
            until(a,ma,'chat',lambda e:'four metres' in str(e))
            result={'result':'PASS','checks':['door open/close replicated to two players and late observer',
                'self/other-player, equipment, vendor, door and chest inspections','v4 captured name preserved','chest open state replicated, last-viewer close, logout, late observer and out-of-range close','locked door rejected','native shop opens with training kit disabled','insufficient funds rejected',
                'owned gear sold and gold spent on native catalog purchase','stale offer and ownership rejected',
                'combat prevents trading','native gateway description and sign use','remote sign, door and vendor use rejected']}
            (run/'result.json').write_text(json.dumps(result,indent=2));print(json.dumps(result))
        finally:
            for p in peers:p.close()
            if server.poll() is None:
                (run/'world/stop.request').write_text('stop\n')
                try:server.wait(timeout=8)
                except subprocess.TimeoutExpired:server.terminate();server.wait()


if __name__=='__main__':main()
