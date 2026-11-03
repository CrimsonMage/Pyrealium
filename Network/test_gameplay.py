"""Real UDP gameplay acceptance against Legions DATs and a disposable Pyrealium store.

Verifies the authored starter dungeon circuit, not retail quest/script parity.
No production journals or account credentials are used.
"""
import json
import math
import os
import subprocess
import time
import uuid
from test_network import Peer, ROOT, SERVER, DAT_DIRECTORY, prepare_test_content


def until(peer, start, event, predicate=lambda e: True, timeout=15):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        found = next((e for e in peer.events[start:] if e.get('event') == event and predicate(e)), None)
        if found is not None:
            return found
        time.sleep(.02)
    raise AssertionError(f'{event} missing; recent events={peer.events[-12:]}')


def main():
    data = ROOT / 'Saved/AC2NetworkTests' / ('gameplay-' + uuid.uuid4().hex)
    data.mkdir(parents=True)
    content = prepare_test_content(data)
    # Isolated service fixtures. These are test positions, not retail spawns.
    with content.open('a') as fixture:
        for n,entity,visual,x,y in [(0,0x4700157d,0x1f000d74,58,239),(1,0x4700157e,0x1f000d75,58,238),(2,0x47001581,0x1f000d78,60,238),(3,0x47001573,0x1f0002f4,60,239)]:
            fixture.write(f'{0x8140000000030100+n:016x}\t{entity:08x}\t{visual:08x}\t02990109\t{x}\t{y}\t-44.606205\t1\t0\t0\t0\t-\n')
    log = (data/'server.log').open('w', encoding='utf-8')
    args = [str(SERVER), 'serve', '--bind', '127.0.0.1:19320', '--account', 'local',
            '--data', str(data/'world'), '--game-data', str(DAT_DIRECTORY), '--content', str(content),
            '--initialize-ui', 'true', '--training-kit', 'false', '--lifestones', 'true', '--portals', 'true', '--seconds', '240']
    server = subprocess.Popen(args, env=dict(os.environ, PYREALIUM_UNREAL_LEGIONS_DATA='1'),
                              stdout=log, stderr=log, creationflags=0x08000000)
    peers=[]
    result={}
    try:
        time.sleep(1)
        assert server.poll() is None, (data/'server.log').read_text()
        for name in ['Dungeon Hero','Dungeon Observer']:
            p=Peer();peers.append(p)
            p.send('connect',host='127.0.0.1',port=19320,account='local');p.wait('characters')
            p.send('create',name=name,start_area=0,race=1,sex=4096,physical=[[1,1],[2,1]])
            p.id=p.wait('created')['character']['id']
            p.send('enter',id=p.id);p.wait('player')
            p.spawn=p.wait('object',lambda e:e['id']==p.id)
            p.send('ready');p.wait('status',lambda e:e['phase']=='world')
            until(p,0,'inventory',lambda e:len(e.get('items',[]))>=3)
        a,b=peers
        until(a,0,'object',lambda e:e['id']==b.id)
        def shelter(n,table,key):
            start=len(a.events);a.send('use',target='0',item=str(0x8140000000030100+n))
            until(a,start,'chat',lambda e:e.get('text',{}).get('table')==table and e['text'].get('key')==key)
            time.sleep(1.1)
        shelter(2,0x2500164c,0x8b6b186d) # Sentry cannot bypass Guard/Lieutenant.
        shelter(0,0x25001648,0x2824fd4c)
        shelter(0,0x25001648,0x63691d8f) # Repeated use does not write duplicate progress.
        shelter(1,0x25001649,0x8b6b186f)
        shelter(2,0x2500164c,0x8b6b1869)
        shelter(3,0x2500163e,0x6c459364) # Native training-stone text reference.
        result['shelter_introductions_ordered_native_dialogue']=True
        inventory=next(e for e in a.events if e.get('event')=='inventory')
        items={e['entity']:e for e in a.events if e.get('event')=='object' and e.get('container')==a.id}
        sword,staff,bow=[items[did] for did in [0x470007ee,0x470007e0,0x470007ed]]
        bag=list(inventory['items'])
        shirt=items[0x47000ef8]
        original=a.spawn['visual']
        time.sleep(.15);start=len(a.events);obs=len(b.events)
        a.send('unequip',id=shirt['id'],location=2,slot=len(bag))
        ack=until(a,start,'inventory_result',lambda e:e['id']==shirt['id'])
        assert ack['status']==0 and ack.get('equipped')==0,ack
        bag.insert(ack['slot'],shirt['id'])
        bare=until(a,start,'visual',lambda e:e['id']==a.id)['visual']
        remote=until(b,obs,'visual',lambda e:e['id']==a.id)['visual']
        assert bare==remote and bare!=original
        assert next(v['value'] for v in bare['aspects'] if v['table']==0x20000016 and v['key']==0x10)==0
        # A later observer must receive the changed armor in CreateObject.
        obs=len(b.events);b.send('logout',id=b.id);until(b,obs,'selection')
        obs=len(b.events);b.send('enter',id=b.id);until(b,obs,'player');b.send('ready')
        assert until(b,obs,'object',lambda e:e['id']==a.id)['visual']==bare
        time.sleep(.15);start=len(a.events);obs=len(b.events)
        a.send('equip',id=shirt['id'],location=2,slot=bag.index(shirt['id']))
        ack=until(a,start,'inventory_result',lambda e:e['id']==shirt['id'])
        assert ack['status']==0 and ack.get('equipped')==2,ack
        bag.remove(shirt['id'])
        assert until(a,start,'visual',lambda e:e['id']==a.id)['visual']==original
        assert until(b,obs,'visual',lambda e:e['id']==a.id)['visual']==original
        result['armor_local_remote_and_late_observer']=True
        time.sleep(.15)
        # A rejected attack stops the queue while preserving stance in the HUD
        # and replicated model. Peace is an explicit player action.
        start=len(a.events);obs=len(b.events)
        a.send('target',id='0');a.send('combat',enabled=True)
        until(a,start,'combat',lambda e:e['enabled'])
        until(b,obs,'mode',lambda e:e['id']==a.id and e['mode']==0x4100000f)
        stone=next(e for e in a.events if e.get('event')=='object' and e.get('entity')==0x470007de)
        start=len(a.events);obs=len(b.events)
        a.send('attack',id=stone['id'])
        until(a,start,'combat',lambda e:e['enabled'])
        until(a,start,'mode',lambda e:e['id']==a.id and e['mode']==0x4100000f)
        until(b,obs,'mode',lambda e:e['id']==a.id and e['mode']==0x4100000f)
        a.send('combat',enabled=False);until(a,start,'combat',lambda e:not e['enabled'])
        result['rejected_attack_preserves_local_and_remote_stance']=True
        def equip(item,mode):
            start=len(a.events); obs=len(b.events)
            a.send('equip',id=item['id'],location=0x40,slot=bag.index(item['id']))
            ack=until(a,start,'inventory_result',lambda e:e['id']==item['id'])
            assert ack['status']==0 and ack.get('equipped')==0x40,ack
            bag.remove(item['id'])
            held=until(b,obs,'object',lambda e:e['id']==item['id'] and e.get('parent')==a.id)
            assert held['holding'] != 0,held
            a.send('target',id='0');a.send('combat',enabled=True)
            until(a,start,'combat',lambda e:e['enabled'])
            until(b,obs,'mode',lambda e:e['id']==a.id and e['mode']==mode)
            a.send('combat',enabled=False)
            until(a,start,'combat',lambda e:not e['enabled'])
        def unequip(item):
            time.sleep(.15);start=len(a.events)
            a.send('unequip',id=item['id'],location=0x40,slot=len(bag))
            ack=until(a,start,'inventory_result',lambda e:e['id']==item['id'])
            assert ack['status']==0 and ack.get('equipped')==0,ack
            bag.insert(ack['slot'],item['id'])
            time.sleep(.15)
        pose=a.spawn['placement'];position=list(pose['position'])
        def move(destination,cell=None):
            nonlocal position
            cell=cell or pose['cell']
            # Walk at a bounded speed using native movement messages.
            distance=math.dist(position,destination)
            steps=max(1,math.ceil(distance/.5));old=position[:]
            for i in range(1,steps+1):
                position=[x+(y-x)*i/steps for x,y in zip(old,destination)]
                a.send('move',cell=cell,position=position,heading=90,motion=[0,1,0]);time.sleep(.1)
            a.send('move',cell=cell,position=position,heading=90,motion=[0,0,0])
        equip(sword,0x4100000a)
        start=len(a.events); obs=len(b.events)
        move([position[0]+.5,position[1],position[2]])
        update=until(b,obs,'position',lambda e:e['id']==a.id and math.dist(e['position'],position)<.01)
        result['player_replication']=update['position']
        # A magic skill with a sword must not damage the target or spend vigor.
        idols=[e for e in a.events if e.get('event')=='object' and e.get('entity')==0x470007d1 and e['placement']['cell']==pose['cell']]
        ids=list(dict.fromkeys(e['id'] for e in idols));assert len(ids)>=3,ids
        a.send('skill',skill=0x60,target=ids[0])
        until(a,start,'chat',lambda e:'matching' in str(e));until(a,start,'skill_finished')
        assert not any(e.get('event')=='hit' and e.get('id')==ids[0] for e in a.events[start:])
        result['wrong_weapon_rejected']=True
        # First kill: retaliation, native attack and death all visible to the observer.
        start=len(a.events);obs=len(b.events)
        a.send('attack',id=ids[0])
        until(a,start,'hit',lambda e:e['id']==a.id)
        until(b,obs,'hit',lambda e:e['id']==a.id)
        until(a,start,'quality',lambda e:e.get('id')==ids[0] and e['key']==0x102 and e['value']==0)
        until(b,obs,'quality',lambda e:e.get('id')==ids[0] and e['key']==0x102 and e['value']==0)
        a.send('combat',enabled=False)
        until(a,start,'combat',lambda e:not e['enabled'])
        result['melee_damage_retaliation_death']=True
        # The inferred treasure policy can now give these Idols variable loot.
        # Reopening must preserve that lifetime's roll; exact native trophies
        # and inferred item eligibility have their own acceptance tests.
        def claim(corpse):
            mark=len(a.events);a.send('open_container',id=corpse)
            opened=until(a,mark,'container_state',lambda e:e['open'] and e['id']==corpse)
            assert len(opened['items'])==len(set(opened['items'])),opened
            a.send('close_container',id=corpse)
            until(a,mark,'container_state',lambda e:not e['open'] and e['id']==corpse)
            mark=len(a.events);a.send('open_container',id=corpse)
            reopened=until(a,mark,'container_state',lambda e:e['open'] and e['id']==corpse)
            assert reopened['items']==opened['items'],(opened,reopened)
            a.send('close_container',id=corpse)
            until(a,mark,'container_state',lambda e:not e['open'] and e['id']==corpse)
        claim(ids[0])
        unequip(sword);equip(staff,0x41000011)
        start=len(a.events);obs=len(b.events)
        a.send('skill',skill=0x60,target=ids[1]);until(a,start,'skill_finished')
        until(b,obs,'behavior',lambda e:e['id']==a.id and e['behavior']==0x410000b9)
        # Native perception may have brought this creature into melee before
        # the first spell. Its shared approach need not start at that spell.
        mob_pose=until(b,0,'position',lambda e:e['id']==ids[1])
        until(a,0,'position',lambda e:e['id']==ids[1] and e['position']==mob_pose['position'])
        # Stop only the player's attack, keeping the hostile creature pursuing.
        a.send('combat',enabled=False);until(a,start,'combat',lambda e:not e['enabled'])
        attack_start=len(a.events);a.send('attack',id=ids[1])
        until(a,start,'quality',lambda e:e.get('id')==ids[1] and e['key']==0x102 and e['value']==0)
        a.send('combat',enabled=False)
        until(a,attack_start,'combat',lambda e:not e['enabled'])
        result['magic_and_enemy_replication']=True
        until(a,attack_start,'fx',lambda e:e['id']==a.id and e['fx']==0x810001cd)
        until(b,0,'fx',lambda e:e['id']==ids[1] and e['fx']==0x41000248)
        result['native_cast_and_impact_fx_replicated']=True
        claim(ids[1])
        time.sleep(.3);unequip(staff);equip(bow,0x41000010)
        start=len(a.events);obs=len(b.events)
        a.send('skill',skill=0xcf,target=ids[2]);until(a,start,'skill_finished')
        until(b,obs,'behavior',lambda e:e['id']==a.id and e['behavior']==0x410000e2)
        # A basic missile skill must continue attacking its target without a
        # second attack command, including the killing blow.
        until(a,start,'quality',lambda e:e.get('id')==ids[2] and e['key']==0x102 and e['value']==0)
        a.send('combat',enabled=False)
        until(a,start,'combat',lambda e:not e['enabled']);time.sleep(.3)
        result['missile_skill_autorepeats']=True
        # Receive the combat quest reward, bind a real lifestone and leave via portal.
        mentor=next(e for e in a.events if e.get('event')=='object' and e.get('entity')==0x47001699 and e['placement']['cell']==pose['cell'])
        move(mentor['placement']['position']);start=len(a.events)
        a.send('use',item=mentor['id'],target='0')
        until(a,start,'chat',lambda e:'Training combat complete' in str(e))
        stone=next(e for e in a.events if e.get('event')=='object' and e.get('entity')==0x470007de and e['placement']['cell']==pose['cell'])
        move(stone['placement']['position']);start=len(a.events)
        a.send('use',item=stone['id'],target='0');until(a,start,'chat',lambda e:'bound' in str(e).lower())
        portal=next(e for e in a.events if e.get('event')=='object' and e['id']==str(0x8140000000030002))
        move(portal['placement']['position']);time.sleep(5.1);start=len(a.events);obs=len(b.events)
        a.send('use',item=portal['id'],target='0')
        exit_pose=until(a,start,'position',lambda e:e['id']==a.id and e['cell']==0x74ba0008)
        until(b,obs,'remove',lambda e:e['id']==a.id)
        result['starter_dungeon_exit']=exit_pose['cell']
        time.sleep(5.1);start=len(a.events);obs=len(b.events)
        a.send('recall')
        until(a,start,'position',lambda e:e['id']==a.id and e['cell']==pose['cell'],timeout=30)
        until(b,obs,'object',lambda e:e['id']==a.id,timeout=30)
        result['lifestone_recall_and_interest']=True
        # The observer deliberately lets a respawned creature kill it. Death
        # blocks combat, and resurrection restores a fresh native instance.
        initial_instance=next(e['instance'] for e in idols if e['id']==ids[0])
        respawn=until(b,0,'object',lambda e:e['id']==ids[0] and e['instance']>initial_instance,timeout=65)
        start=len(b.events);obs=len(a.events)
        b.send('skill',skill=0xb0,target=ids[0]);until(b,start,'skill_finished')
        b.send('combat',enabled=False)
        until(b,start,'death',lambda e:e['dead']==1,timeout=65)
        until(a,obs,'quality',lambda e:e.get('id')==b.id and e['key']==0x102 and e['value']==0)
        start=len(b.events);b.send('attack',id=ids[0])
        until(b,start,'chat',lambda e:'dead' in str(e).lower())
        start=len(b.events);b.send('resurrect')
        until(b,start,'death',lambda e:e['dead']==0)
        until(b,start,'quality',lambda e:e.get('id') is None and e['key']==0x102 and e['value']>0)
        result['player_death_and_resurrection']=True
        # Replaying the real journals must preserve gear and the completed quest.
        hero=a.id;bow_id=bow['id']
        for p in peers:p.close()
        (data/'world/stop.request').write_text('stop\n');assert server.wait(timeout=10)==0
        server=subprocess.Popen(args, env=dict(os.environ,PYREALIUM_UNREAL_LEGIONS_DATA='1'),
                                stdout=log,stderr=log,creationflags=0x08000000)
        time.sleep(1);assert server.poll() is None,(data/'server.log').read_text()
        p=Peer();peers.append(p);p.send('connect',host='127.0.0.1',port=19320,account='local');p.wait('characters')
        p.send('enter',id=hero);p.wait('player');p.wait('object',lambda e:e['id']==hero);p.send('ready')
        saved=until(p,0,'inventory');assert {'id':bow_id,'slot':0x40} in saved['equipment'],saved
        p.send('chat',text='@quest status',channel='say')
        until(p,0,'chat',lambda e:'Training combat complete' in str(e))
        time.sleep(.6) # Chat commands share the normal anti-flood interval.
        start=len(p.events);p.send('chat',text='@quest shelter',channel='say')
        until(p,start,'chat',lambda e:"Imbued Digging Tool" in str(e))
        result['restart_preserves_gear_and_quest']=True
        result['shelter_introduction_survives_restart']=True
        result['scope']='Shelter introduction plus Pyrealium authored combat/binding/exit circuit; continuation tested separately by test_shelter.py'
        (data/'result.json').write_text(json.dumps(result,indent=2))
        print(json.dumps(result,indent=2));print(data)
    finally:
        for i,p in enumerate(peers):
            (data/f'peer-{i}.json').write_text(json.dumps(p.events,indent=2))
            p.close()
        if server.poll() is None:
            (data/'world/stop.request').write_text('stop\n');server.wait(timeout=10)
        log.close()

if __name__=='__main__':main()
