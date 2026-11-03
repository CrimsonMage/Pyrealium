"""Native crafting, mentor lessons and atomic persistence over isolated UDP.

The nearby actors and quick combat rules below are acceptance fixtures, not
retail placement/balance claims. No production character data is touched.
"""
import json
import os
import subprocess
import time
import uuid
from test_network import ROOT, SERVER, DAT_DIRECTORY, Peer
from test_gameplay import until

PORT = 19528
MENTOR, VENDOR = [str(0x8140000000030A00 + n) for n in range(2)]
IDOLS = [str(0x8140000000030A10 + n) for n in range(3)]
HEALTH, VIGOR = 0x6F001F17, 0x6F001F18
MUCOR, TALLOW = 0x47001744, 0x470027A7


def fixture(run):
    rows = [(MENTOR, 0x47001699, 0x1F000DEB, 59, 240),
            (VENDOR, 0x47003724, 0x1F001941, 60, 241)]
    rows += [(iid, 0x470007D1, 0x1F0003D1, 58 + n * .5, 242) for n, iid in enumerate(IDOLS)]
    content = run / 'world.tsv'
    content.write_text('pyrealium-world-v3\n' + ''.join(
        f'{int(iid):016x}\t{entity:08x}\t{visual:08x}\t02990109\t{x}\t{y}\t-44.606205\t1\t0\t0\t0\t-\n'
        for iid, entity, visual, x, y in rows))
    encounters = run / 'encounters.tsv'
    encounters.write_text('pyrealium-encounters-v1\nplayer\t20\t2\t1000\t3\nregen\t2\t4\t1000\n'
                          'npc\t470007d1\t20\t1\t1500\t3\t300000\t40\t2\n')
    return content, encounters


def connect(name=None, character=None):
    p = Peer()
    p.send('connect', host='127.0.0.1', port=PORT, account='local');p.wait('characters')
    if character is None:
        p.send('create', name=name, start_area=0, race=1, sex=4096, physical=[[1,1],[2,1]])
        character = p.wait('created')['character']['id']
    p.id = character;p.send('enter', id=character);p.wait('player')
    p.send('ready');p.wait('status', lambda e:e['phase']=='world')
    until(p, 0, 'inventory')
    registry = until(p, 0, 'craft_registry')
    assert registry['replace'] and {HEALTH,VIGOR}.issubset(registry['recipes']) and len(registry['recipes'])>=269, registry
    return p


def talk(p, text):
    time.sleep(1.1);mark = len(p.events);p.send('use', item=MENTOR, target='0')
    until(p, mark, 'chat', lambda e:text in str(e))


def buy_pair(p):
    time.sleep(.3);mark=len(p.events);p.send('use',item=VENDOR,target='0')
    shop=until(p,mark,'shop');items=[]
    for definition in [MUCOR,TALLOW]:
        offer=next(o for o in shop['offers'] if o['entity']==definition)
        time.sleep(.3);mark=len(p.events);p.send('shop_buy',vendor=VENDOR,offer=offer['id'])
        item=until(p,mark,'object',lambda e:e.get('entity')==definition and e.get('container')==p.id)
        until(p,mark,'chat',lambda e:'Bought ' in str(e));items.append(item['id'])
    p.send('shop_close',vendor=VENDOR);until(p,mark,'shop_close');time.sleep(1.1)
    return items


def prepare_practice(p):
    # Complete the existing authored combat prerequisite using ordinary attacks.
    bag=next(e['items'] for e in reversed(p.events) if e.get('event')=='inventory')
    sword=next(e for e in p.events if e.get('entity')==0x470007EE and e.get('container')==p.id)
    mark=len(p.events);p.send('equip',id=sword['id'],location=0x40,slot=bag.index(sword['id']))
    assert until(p,mark,'inventory_result',lambda e:e['id']==sword['id'])['status']==0
    for iid in IDOLS:
        mark=len(p.events);p.send('attack',id=iid)
        until(p,mark,'quality',lambda e:e.get('id')==iid and e['key']==0x102 and e['value']==0)
    mark=len(p.events);p.send('combat',enabled=False);until(p,mark,'combat',lambda e:not e['enabled'])
    talk(p,'Training combat complete')
    talk(p,'Field Supplies:')
    buy_pair(p)
    talk(p,'Field Supplies complete')
    talk(p,'Potion Practice:')
    return buy_pair(p)


def validate(p, recipe, ordinal, item, accepted):
    mark=len(p.events);p.send('craft_validate',recipe=recipe,ordinal=ordinal,item=item)
    reply=until(p,mark,'craft_ingredient',lambda e:e['recipe']==recipe and e['ordinal']==ordinal)
    assert reply['item']==item and (reply['error']==0)==accepted,reply


def craft(p, recipe, ingredients, success):
    mark=len(p.events);p.send('craft_execute',recipe=recipe,ingredients=ingredients)
    reply=until(p,mark,'craft_complete',lambda e:e['recipe']==recipe)
    assert reply['notify'] and (reply['error']==0)==success,reply
    return mark


def main():
    run=ROOT/'Saved/AC2NetworkTests'/'craft-'
    run=run.with_name(run.name+uuid.uuid4().hex);run.mkdir(parents=True)
    content, encounters=fixture(run);peers=[];server=None;character=None
    env=dict(os.environ,PYREALIUM_UNREAL_LEGIONS_DATA='1')
    def start(log):
        process=subprocess.Popen([str(SERVER),'serve','--bind',f'127.0.0.1:{PORT}','--account','local',
            '--data',str(run/'world'),'--game-data',str(DAT_DIRECTORY),'--content',str(content),
            '--encounters',str(encounters),'--initialize-ui','true','--training-kit','false',
            '--portals','false','--lifestones','false','--seconds','240'],env=env,
            stdout=log,stderr=log,creationflags=0x08000000)
        time.sleep(1);assert process.poll() is None,(run/'server.log').read_text();return process
    def stop():
        if server and server.poll() is None:
            (run/'world/stop.request').write_text('stop\n');server.wait(timeout=12)
    try:
        with (run/'server.log').open('w') as log:
            server=start(log);p=connect('Craft Tester');peers.append(p);character=p.id
            items=prepare_practice(p)
            for ordinal,item in enumerate(items,1):validate(p,HEALTH,ordinal,item,True)
            validate(p,HEALTH,1,items[1],False)
            validate(p,0x6F001F0E,1,items[0],False)  # Unimplemented recipe is not granted.
            validate(p,HEALTH,1,MENTOR,False)  # World actor cannot substitute an owned ingredient.
            mark=len(p.events);p.send('combat',enabled=True);until(p,mark,'combat',lambda e:e['enabled'])
            validate(p,HEALTH,1,items[0],False);craft(p,HEALTH,items,False)
            p.send('combat',enabled=False);until(p,mark,'combat',lambda e:not e['enabled'])
            mark=craft(p,HEALTH,items,True)
            output=until(p,mark,'object',lambda e:e.get('entity')==0x47001745 and e.get('container')==p.id)
            for item in items:until(p,mark,'inventory_remove',lambda e:e['id']==item)
            until(p,mark,'chat',lambda e:'you crafted a potion' in str(e))
            time.sleep(1.1);failed=craft(p,HEALTH,items,False)
            assert not any(e.get('event')=='object' and e.get('entity')==0x47001745 for e in p.events[failed:]),'Replay created another potion'
            talk(p,'Potion Practice complete')
            ingredients=buy_pair(p)
            for ordinal,item in enumerate(ingredients,1):validate(p,VIGOR,ordinal,item,True)
            mark=craft(p,VIGOR,ingredients,True)
            until(p,mark,'object',lambda e:e.get('entity')==0x47001746 and e.get('container')==p.id)
            mark=len(p.events);p.send('logout',id=p.id);until(p,mark,'selection');p.close();peers.remove(p)
            stop()
        with (run/'server-restart.log').open('w') as log:
            server=start(log);p=connect(character=character);peers.append(p)
            bag=until(p,0,'inventory')['items']
            assert all(i not in bag for i in items),'Consumed ingredient reappeared after restart'
            quest=until(p,0,'quests',lambda e:any(q['id']==0x4000FF03 for q in e['items']))
            practice=next(q for q in quest['items'] if q['id']==0x4000FF03)
            assert 'Potion Practice complete' in str(practice),practice
            assert output['id'] in bag,'Crafted potion lost after restart'
        report={'result':'PASS','checks':['normal profile grants native recipe registry',
            'mentor advances combat -> Field Supplies -> Potion Practice',
            'native crafting consumes ingredients and credits quest atomically','server restart preserves items and quest']}
        report['checks']+=['wrong ingredient, unknown recipe, ownership, combat and replay rejected','health and vigor recipes']
        (run/'result.json').write_text(json.dumps(report,indent=2));print(json.dumps(report));print('Evidence:',run)
    finally:
        for p in peers:p.close()
        stop()


if __name__=='__main__':
    main()
