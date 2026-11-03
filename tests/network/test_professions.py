"""Profession acceptance over real UDP, with isolated characters and workshops."""
import json
import os
import re
import subprocess
import time
import uuid
import test_crafting as craft
from test_network import ROOT, SERVER, DAT_DIRECTORY
from test_gameplay import until
VERMIN=str(0x8140000000030B10)


def buy(p, definition, count=1):
    time.sleep(.3); mark=len(p.events); p.send('use',item=craft.VENDOR,target='0')
    shop=until(p,mark,'shop'); offer=next(o for o in shop['offers'] if o['entity']==definition)
    for _ in range(count):
        time.sleep(.3); mark=len(p.events); p.send('shop_buy',vendor=craft.VENDOR,offer=offer['id'])
        until(p,mark,'object',lambda e:e.get('entity')==definition and e.get('container')==p.id)
        until(p,mark,'chat',lambda e:'Bought ' in str(e))
    p.send('shop_close',vendor=craft.VENDOR);time.sleep(1.1)


def owned(p, definition):
    removed={e['id'] for e in p.events if e.get('event')=='inventory_remove'}
    return [e for e in p.events if e.get('event')=='object' and e.get('container')==p.id and e.get('entity')==definition and e['id'] not in removed]


def execute(p, recipe, spinner, slots):
    time.sleep(1.1);mark=len(p.events)
    p.send('craft_execute',recipe=recipe,spinner=spinner,helper=p.id,ingredients=[{'ordinal':i,'item':iid} for i,iid in slots.items()])
    result=until(p,mark,'craft_complete',lambda e:e['recipe']==recipe)
    return mark,result


def main():
    run=ROOT/'Saved/AC2NetworkTests'/('professions-'+uuid.uuid4().hex);run.mkdir(parents=True)
    content,encounters=craft.fixture(run)
    # The large quest reward is an acceptance-test budget, never production stock.
    encounters.write_text(encounters.read_text().replace('300000\t40\t2','300000\t40\t500'))
    with encounters.open('a') as f:f.write('npc\t47000f7e\t20\t1\t1500\t3\t300000\t11\t2\n')
    with content.open('a') as f:
        f.write(f'{int(VERMIN):016x}\t47000f7e\t1f0004e0\t02990109\t60\t242\t-44.606205\t1\t0\t0\t0\t-\n')
        for n,entity,visual in [(0,0x4700082c,0x1f00065a),(1,0x4700082d,0x1f00056a)]:
            f.write(f'{0x8140000000030B00+n:016x}\t{entity:08x}\t{visual:08x}\t02990109\t60\t240\t-44.606205\t1\t0\t0\t0\t-\n')
    peers=[];server=None
    def start(log):
        s=subprocess.Popen([str(SERVER),'serve','--bind',f'127.0.0.1:{craft.PORT}','--account','local','--data',str(run/'world'),
            '--game-data',str(DAT_DIRECTORY),'--content',str(content),'--encounters',str(encounters),'--initialize-ui','true',
            '--training-kit','false','--portals','false','--lifestones','false','--test-map-travel','true','--seconds','360'],env=dict(os.environ),stdout=log,stderr=log,creationflags=0x08000000)
        time.sleep(1);assert s.poll() is None;return s
    def stop():
        if server and server.poll() is None:(run/'world/stop.request').write_text('stop\n');server.wait(timeout=12)
    try:
        with (run/'server.log').open('w') as log:
            server=start(log);p=craft.connect('Profession Tester');peers.append(p);character=p.id
            registry=until(p,0,'craft_registry');assert len(registry['catalog'])>=341 and len(registry['progress'])==26
            assert registry['success'][500]==80
            craft.prepare_practice(p)
            for d in [0x470010fe,0x470010f3,0x470010f5]:buy(p,d,2)
            for d in [0x470010fe,0x470010f3,0x470010f5]:
                stacks=owned(p,d);assert len(stacks)==1 and stacks[0]['quantity']==200,stacks
            mark=len(p.events);p.send('combat',enabled=True);until(p,mark,'combat',lambda e:e['enabled'])
            mark,reply=execute(p,0x6f00261c,1,{2:owned(p,0x470010fe)[0]['id']});assert reply['error']!=0
            until(p,mark,'chat',lambda e:'peace mode' in str(e))
            assert sum(i['quantity'] for i in owned(p,0x470010fe))==200
            mark=len(p.events);p.send('combat',enabled=False);until(p,mark,'combat',lambda e:not e['enabled'])
            # Nonconsecutive native ingredient ordinal, exact level and live XP.
            successes=0
            for _ in range(12):
                material=owned(p,0x470010fe)[0]['id']
                mark,reply=execute(p,0x6f00261c,1,{2:material})
                progress=until(p,mark,'craft_skills')['progress'][0]
                assert progress['id']==0x6f0023a8
                if reply['error']==0:successes+=1
                if successes==3:break
            assert successes==3,'No three successes in 12 attempts'
            assert progress['available']==42
            mark=len(p.events);p.send('craft_raise',skill=0x6f0023a8)
            raised=until(p,mark,'craft_skills')['progress'][0];assert raised['level']==2 and raised['available']==2,raised
            jacket=owned(p,0x47002aed)[-1]
            mark=len(p.events);p.send('examine',id=jacket['id'])
            panel=until(p,mark,'examination');assert 'Armor: 10' in str(panel) and 'Crafted level: 1' in str(panel),panel
            # A 200-unit refining job consumes the merged supplier materials.
            mark,reply=execute(p,0x6f003315,1,{1:owned(p,0x470010f3)[0]['id'],2:owned(p,0x470010f5)[0]['id']})
            assert reply['error']==0,reply
            steel=until(p,mark,'object',lambda e:e.get('entity')==0x470027a6 and e.get('container')==p.id)
            assert steel['quantity']==66,steel
            for d in [0x470010f3,0x470010f5]:buy(p,d,2)
            mark,reply=execute(p,0x6f003315,1,{1:owned(p,0x470010f3)[0]['id'],2:owned(p,0x470010f5)[0]['id']})
            assert reply['error']==0,reply
            combined=until(p,mark,'object',lambda e:e.get('entity')==0x470027a6 and e.get('container')==p.id)
            assert combined['quantity']==132 and combined['id']!=steel['id'],combined
            assert len(owned(p,0x470027a6))==1
            # Craft a working mining tool, generate a map and find its native region.
            catalog=json.loads((ROOT/'content/professions-v1.json').read_text())
            recipe=next(r for r in catalog['recipes'] if r['id']==0x6f002854)
            ingredient=next(i for i in recipe['ingredients'] if i['minimum']<=1 and i['quantities'][1]>0);raw=ingredient['choices'][0]
            buy(p,raw,2)
            for _ in range(12):
                mark,reply=execute(p,recipe['id'],1,{ingredient['ordinal']:owned(p,raw)[0]['id']})
                if reply['error']==0:break
            assert reply['error']==0
            pick=owned(p,0x47002deb)[-1]['id']
            # Hunting knives consume a different native raw ingredient. The
            # corpse must be owned, dead, recent and not already butchered.
            buy(p,0x470010f5)
            for _ in range(12):
                _,reply=execute(p,0x6f002851,1,{2:owned(p,0x470010f5)[0]['id']})
                if reply['error']==0:break
            assert reply['error']==0
            knife=owned(p,0x47002de8)[-1]['id']
            time.sleep(1.6);mark=len(p.events);p.send('use',item=knife,target=VERMIN)
            until(p,mark,'chat',lambda e:'recent corpse' in str(e))
            mark=len(p.events);p.send('attack',id=VERMIN)
            until(p,mark,'quality',lambda e:e.get('id')==VERMIN and e['key']==0x102 and e['value']==0)
            mark=len(p.events);p.send('combat',enabled=False);until(p,mark,'combat',lambda e:not e['enabled'])
            time.sleep(1.6);mark=len(p.events);p.send('use',item=knife,target=VERMIN)
            until(p,mark,'chat',lambda e:'Gathered ' in str(e));until(p,mark,'craft_skills',lambda e:e['progress'][0]['id']==0x6f002560)
            time.sleep(1.6);mark=len(p.events);p.send('use',item=knife,target=VERMIN)
            until(p,mark,'chat',lambda e:'already been butchered' in str(e))
            maps={m['id']:m for m in catalog['maps']}
            recipe=next(r for r in catalog['recipes'] if r['kind']=='survey' and all(maps[o['item']]['region']==0 and next(m for m in catalog['mines'] if m['id']==maps[o['item']]['mine'])['material']==0x470010f3 for o in r['outputs']))
            mark,reply=execute(p,recipe['id'],0,{})
            assert reply['error']==0,reply
            survey=until(p,mark,'object',lambda e:e.get('entity') in maps and e.get('container')==p.id)
            mid=int(survey['id']);nid=str(0x8190000000000000|(mid&0xffffffffffff))
            # Same-continent repeated surveys reject without granting extra maps.
            _,reply=execute(p,recipe['id'],0,{});assert reply['error']!=0
            # Refresh the authoritative native bag list, then prove that the
            # owner-bound map cannot be handed to another player via a drop.
            mark=len(p.events);p.send('logout',id=p.id);until(p,mark,'selection');p.close();peers.remove(p)
            p=craft.connect(character=character);peers.append(p)
            bag=until(p,0,'inventory')['items'];mark=len(p.events)
            p.send('drop_item',id=survey['id'],**{'from':bag.index(survey['id'])},quantity=1,landblock=0x02990000)
            until(p,mark,'chat',lambda e:'cannot be dropped' in str(e))
            assert not any(e.get('event')=='inventory_remove' and e.get('id')==survey['id'] for e in p.events[mark:])
            mark=len(p.events);p.send('examine',id=survey['id']);panel=until(p,mark,'examination');assert 'Survey site:' in str(panel)
            coords=re.search(r'Survey site: ([0-9.]+)([NS]), ([0-9.]+)([EW])',str(panel));assert coords,panel
            north=float(coords[1])*(-1 if coords[2]=='S' else 1);east=float(coords[3])*(-1 if coords[4]=='W' else 1)
            time.sleep(2);mark=len(p.events);p.send('chat',text=f'@maptravel {1020+east*10.2} {1020+north*10.2}',channel='say')
            until(p,mark,'position',lambda e:e['id']==p.id)
            time.sleep(2);mark=len(p.events);p.send('use',item=survey['id'],target='0')
            until(p,mark,'object',lambda e:e['id']==nid)
            mark=len(p.events);p.send('combat',enabled=True);until(p,mark,'combat',lambda e:e['enabled'])
            time.sleep(1.6);mark=len(p.events);p.send('use',item=pick,target=nid)
            until(p,mark,'chat',lambda e:'peace mode' in str(e))
            mark=len(p.events);p.send('combat',enabled=False);until(p,mark,'combat',lambda e:not e['enabled'])
            # No manufacture away from workshops, even with valid materials.
            mark,reply=execute(p,0x6f00261c,1,{2:owned(p,0x470010fe)[0]['id']});assert reply['error']!=0
            until(p,mark,'chat',lambda e:'workshop' in str(e))
            for _ in range(2):
                time.sleep(1.6);mark=len(p.events);p.send('use',item=pick,target=nid)
                until(p,mark,'chat',lambda e:'Gathered ' in str(e));until(p,mark,'craft_skills',lambda e:e['progress'][0]['id']==0x6f002579)
            # Saved costs/XP and stale item identities cannot be replayed.
            mark=len(p.events);p.send('logout',id=p.id);until(p,mark,'selection');p.close();peers.remove(p);stop()
        with (run/'restart.log').open('w') as log:
            server=start(log);p=craft.connect(character=character);peers.append(p)
            registry=until(p,0,'craft_registry');progress=next(s for s in registry['progress'] if s['id']==0x6f0023a8)
            assert progress['level']==2 and progress['available']==2,progress
            assert owned(p,0x470027a6)[0]['quantity']==132
            assert any(i['id']==jacket['id'] for i in owned(p,0x47002aed))
            until(p,0,'object',lambda e:e['id']==nid)
            time.sleep(1.6);mark=len(p.events);p.send('use',item=pick,target=nid)
            until(p,mark,'chat',lambda e:'Gathered ' in str(e));until(p,mark,'remove',lambda e:e['id']==nid)
            time.sleep(1.6);mark=len(p.events);p.send('use',item=pick,target=nid)
            until(p,mark,'chat',lambda e:'expired' in str(e) or 'belongs' in str(e))
        report={'result':'PASS','checks':['26 saved professions and native success table','supplier and refinery stack merging','combat crafting/gathering reject without consuming resources','native XP and rank increase','crafted item inspection','refining quantity','hunting knife and butchery XP','live creature and repeat butchery rejected','survey cooldown and map guidance','survey ownership cannot be transferred','native deposit discovery','workshop distance enforced','tool use and mining XP','mine charges and inventory survive restart','exhausted mine removal']}
        (run/'result.json').write_text(json.dumps(report,indent=2));print(json.dumps(report));print('Evidence:',run)
    finally:
        for p in peers:p.close()
        stop()


if __name__=='__main__':main()
