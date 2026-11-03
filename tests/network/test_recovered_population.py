"""Real UDP gameplay at translated capture locations; no test spawns/balance."""
import json,os,subprocess,time,uuid
from test_network import ROOT,SERVER,DAT_DIRECTORY,Peer
from test_gameplay import until
from prepare_content import prepare

def main():
    run=ROOT/'Saved/AC2NetworkTests'/('recovered-population-'+uuid.uuid4().hex);run.mkdir(parents=True)
    content=run/'world.tsv';prepare(DAT_DIRECTORY,content)
    rows={f[0]:f for l in content.read_text(encoding='utf-8').splitlines() if l.startswith('81') for f in [l.split('\t')]}
    target='8140a52c0d2c05a7'  # Cutter Beetle, captured beside Cragstone, native 46 HP.
    vendor='8140ce693e4b8e95'  # Captured Human Weapon Vendor, Cragstone.
    npc='8140a8708a363c27'     # Katriya the Town Crier, actual captured town location.
    peers=[];report=[]
    with (run/'server.log').open('w') as log:
        server=subprocess.Popen([str(SERVER),'serve','--bind','127.0.0.1:19506','--account','local',
            '--data',str(run/'world'),'--game-data',str(DAT_DIRECTORY),'--content',str(content),
            '--initialize-ui','true','--training-kit','false','--encounters','training',
            '--portals','false','--lifestones','false','--test-map-travel','true','--seconds','240'],
            env=dict(os.environ),stdout=log,stderr=log,creationflags=0x08000000)
        try:
            time.sleep(1);assert server.poll() is None,(run/'server.log').read_text()
            for name in ['Population Hunter','Population Observer']:
                p=Peer();peers.append(p);p.send('connect',host='127.0.0.1',port=19506,account='local');p.wait('characters')
                p.send('create',name=name,start_area=0,race=1,sex=4096,physical=[[1,1],[2,1]])
                p.id=p.wait('created')['character']['id'];p.send('enter',id=p.id);p.wait('player');p.send('ready')
                p.wait('status',lambda e:e['phase']=='world');until(p,0,'inventory')
            a,b=peers
            def visit(p,iid):
                f=rows[iid];point=list(map(float,f[4:7]));point[0]+=1
                mark=len(p.events);p.send('move',cell=int(f[3],16),position=point,heading=0,motion=[0,0,0])
                obj=until(p,0,'object',lambda e:e['id']==str(int(iid,16)))
                time.sleep(.15)
                assert obj['entity']==int(f[1],16);assert obj['name']['literal']==f[12]
                assert all(abs(x-y)<.001 for x,y in zip(obj['placement']['position'],map(float,f[4:7])))
                return obj
            tid=str(int(target,16));visit(a,target);initial=visit(b,target)
            mark=len(a.events);observed=len(b.events);a.send('chat',text='@spawns',channel='say')
            until(a,mark,'chat',lambda e:'Cutter Beetle' in str(e))
            a.send('attack',id=tid)
            until(a,mark,'quality',lambda e:e.get('id')==tid and e.get('key')==0x102 and e['value']==0,timeout=30)
            until(b,observed,'quality',lambda e:e.get('id')==tid and e.get('key')==0x102 and e['value']==0)
            until(a,mark,'combat',lambda e:e['enabled'])
            # Killing a creature stops the swing queue, not stance or selection.
            a.send('combat',enabled=False);until(a,mark,'combat',lambda e:not e['enabled'])
            bm=len(b.events);b.send('open_container',id=tid);until(b,bm,'chat',lambda e:'Loot rejected' in str(e))
            a.send('use',item=tid,target='0');opened=until(a,mark,'container_state',lambda e:e['id']==tid and e['open'])
            assert len(opened['items'])==1
            dropped=until(a,mark,'object',lambda e:e['id']==opened['items'][0])
            assert dropped['entity']==0x47000f25, dropped # Captured Cutter Beetle loot: Wood Dagger.
            loot_mark=len(a.events);a.send('loot',id=tid)
            item=until(a,loot_mark,'object',lambda e:e.get('container')==a.id)
            until(a,loot_mark,'chat',lambda e:'Loot collected' in str(e))
            replay=len(a.events);a.send('loot',id=tid);until(a,replay,'chat',lambda e:'Loot rejected' in str(e))
            report.append(dict(monster=rows[target],loot=item['entity'],observer_saw_death=True))
            visit(a,vendor);vid=str(int(vendor,16));mark=len(a.events);a.send('use',item=vid,target='0')
            shop=until(a,mark,'shop');assert shop['offers']
            # Sell a genuine starter item, then buy through this placed vendor.
            bag=next(e['items'] for e in reversed(a.events) if e.get('event')=='inventory')
            weapon=next(e for e in a.events if e.get('event')=='object' and e.get('id') in bag and e.get('entity')==0x470007ed)
            time.sleep(.3);mark=len(a.events);a.send('shop_sell',item=weapon['id'],slot=bag.index(weapon['id']),quantity=1)
            until(a,mark,'inventory_remove',lambda e:e['id']==weapon['id'])
            offer=next(o for o in shop['offers'] if o['entity']==0x47001744)
            time.sleep(.3);mark=len(a.events);a.send('shop_buy',vendor=vid,offer=offer['id'])
            until(a,mark,'object',lambda e:e.get('entity')==offer['entity'] and e.get('container')==a.id)
            time.sleep(.3);a.send('shop_close',vendor=vid);until(a,mark,'shop_close')
            mark=len(a.events);a.send('attack',id=vid);until(a,mark,'chat',lambda e:'not an attackable creature' in str(e))
            report.append(dict(vendor=rows[vendor],buy_sell=True,protected=True))
            visit(a,npc);nid=str(int(npc,16));mark=len(a.events);a.send('attack',id=nid)
            until(a,mark,'chat',lambda e:'not an attackable creature' in str(e))
            a.send('use',item=nid,target='0');until(a,mark,'chat',lambda e:'Additional scripted actions' in str(e))
            report.append(dict(npc=rows[npc],protected=True,examine_only=True))
            # This observer stays at the spawner while the hunter travels away.
            # Player visibility/logout must not create extra population slots.
            born=until(b,observed,'object',lambda e:e.get('id')==tid and e.get('instance',0)>initial['instance'],timeout=75)
            until(b,observed,'quality',lambda e:e.get('id')==tid and e.get('key')==0x102 and e['value']==46)
            assert born['instance']==initial['instance']+1
            assert born['placement']==initial['placement'],(born,initial)
            assert born['name']==initial['name'], 'Respawn inherited a corpse name'
            time.sleep(.4)
            assert len([e for e in b.events[observed:] if e.get('event')=='object' and e.get('id')==tid])==1
            report.append(dict(spawner=target,initial_instance=initial['instance'],replacement_instance=born['instance'],
                               exact_anchor=True,one_replacement=True))
            (run/'result.json').write_text(json.dumps(dict(result='PASS',checks=report),indent=2))
            print('PASS: recovered wild monster kill/loot/observer death, captured vendor buy/sell, noncombat NPC protection.',run)
        finally:
            for i,p in enumerate(peers):
                (run/f'peer-{i}.json').write_text(json.dumps(p.events));p.close()
            if server.poll() is None:
                (run/'world/stop.request').write_text('stop\n');server.wait(timeout=10)

if __name__=='__main__':
    main()
