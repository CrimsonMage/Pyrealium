"""Exercise real admitted outdoor capture placements, without adding test spawns."""
import json
import math
import os
import subprocess
import time
import uuid
from test_network import ROOT, SERVER, DAT_DIRECTORY, Peer
from test_gameplay import until
from prepare_content import prepare


def main():
    run=ROOT/'Saved/AC2NetworkTests'/('outdoor-'+uuid.uuid4().hex)
    run.mkdir(parents=True)
    content=run/'world.tsv';prepare(DAT_DIRECTORY,content)
    rows={int(f[0],16):f for line in content.read_text(encoding='utf-8').splitlines()[1:]
          if line and not line.startswith('#') for f in [line.split('\t')]}
    # Different capture IDs/subcells must not stack Orven and Adaro in Arwic.
    for entity, retained, superseded in [(0x47001c27, 0x8140d31eddf3ea49, 0x8140dc346c4fa355),
                                         (0x47001bf2, 0x81409443b0ad9f3c, 0x8140cb3aa56b9e04)]:
        assert [iid for iid, row in rows.items() if int(row[1], 16) == entity] == [retained]
        assert superseded not in rows
    # Two independent outdoor observations south of Cragstone. Their captured
    # coordinates, scale and native combat profiles are left unchanged.
    targets=[0x81409bec26cb5af8,0x8140b739fea6ec9a]
    # Native floor at captured X+12, verified by the geometry integration test.
    chase_heights=[141.96219,126.52316]
    # Native MotionInterpDesc forward limit times inherited VisualDesc Y scale
    # and captured PhysicsDesc scale. Neither is the old uniform 2m/s step.
    visual_speeds={0x1f000031:6.5,0x1f0004e0:6.5*1.15}
    assert all(target in rows for target in targets)
    env=dict(os.environ)
    peers=[];report=[]
    with (run/'server.log').open('w') as log:
        server=subprocess.Popen([str(SERVER),'serve','--bind','127.0.0.1:19486','--account','local',
            '--data',str(run/'world'),'--game-data',str(DAT_DIRECTORY),'--content',str(content),
            '--initialize-ui','true','--training-kit','false','--encounters','training',
            '--lifestones','false','--portals','false','--test-map-travel','true','--seconds','240'],
            env=env,stdout=log,stderr=log,creationflags=0x08000000)
        try:
            time.sleep(1);assert server.poll() is None,(run/'server.log').read_text()
            for name in ['Outdoor Scout','Outdoor Observer']:
                p=Peer();peers.append(p)
                p.send('connect',host='127.0.0.1',port=19486,account='local');p.wait('characters')
                p.send('create',name=name,start_area=0,race=1,sex=4096,physical=[[1,1],[2,1]])
                p.id=p.wait('created')['character']['id'];p.send('enter',id=p.id);p.wait('player')
                p.send('ready');p.wait('status',lambda e:e['phase']=='world')
            # The local survey never compares dungeon coordinates to outdoors.
            mark=len(peers[0].events)
            peers[0].send('chat',text='@spawns',channel='say')
            until(peers[0],mark,'chat',lambda e:'Go outdoors' in str(e))
            # Orven is a captured town NPC, not an encounter. The former
            # catalog fallback gave him 60 HP, a damage rule and potion loot.
            town_id=0x8140d31eddf3ea49
            town=rows[town_id];town_point=list(map(float,town[4:7]));town_point[0]+=2
            a=peers[0];a.send('move',cell=int(town[3],16),position=town_point,heading=0,motion=[0,0,0])
            until(a,0,'object',lambda e:e['id']==str(town_id))
            mark=len(a.events);a.send('attack',id=str(town_id))
            until(a,mark,'chat',lambda e:'not an attackable creature' in str(e))
            time.sleep(.6);survey=len(a.events);a.send('chat',text='@spawns',channel='say')
            summary=until(a,survey,'chat',lambda e:'Imported outdoor creatures:' in str(e))
            time.sleep(.3)
            survey_lines=[e for e in a.events[survey:] if e.get('event')=='chat']
            assert not any(name in str(e) for e in survey_lines for name in ['Orven','Pectar']),survey_lines
            assert not any(e.get('event')=='hit' and e.get('id')==str(town_id) for e in a.events[mark:])
            report.append({'protected_npc':town[1],'survey':survey_lines})
            # The authored training mentor has native combat statistics but
            # serves a dialogue role. Never expose him as a killable quest giver.
            mentor_id=0x814000000003000a;mentor=rows[mentor_id]
            near=list(map(float,mentor[4:7]));near[0]+=2
            a.send('move',cell=int(mentor[3],16),position=near,heading=0,motion=[0,0,0])
            until(a,0,'object',lambda e:e['id']==str(mentor_id))
            mark=len(a.events);a.send('attack',id=str(mentor_id))
            until(a,mark,'chat',lambda e:'not an attackable creature' in str(e))
            report.append({'protected_guide':mentor[1]})
            # Regression: the placed Crawlspace Vermin variant was omitted from
            # the combat import even though its native template is hostile.
            vermin_id=0x81408d10ecc7698b;vermin=rows[vermin_id]
            assert int(vermin[1],16)==0x47001073
            near=list(map(float,vermin[4:7]));near[0]+=1.5
            mark=len(a.events);a.send('move',cell=int(vermin[3],16),position=near,heading=0,motion=[0,0,0])
            until(a,mark,'object',lambda e:e['id']==str(vermin_id))
            until(a,mark,'quality',lambda e:e.get('id')==str(vermin_id) and e.get('key')==0x107 and e['value']==81)
            a.send('attack',id=str(vermin_id))
            until(a,mark,'hit',lambda e:e['id']==str(vermin_id),timeout=12)
            assert not any('not an attackable creature' in str(e) for e in a.events[mark:])
            a.send('combat',enabled=False);until(a,mark,'combat',lambda e:not e['enabled'])
            report.append({'native_vermin_attackable':vermin[1],'health':81})
            for target,chase_height in zip(targets,chase_heights):
                f=rows[target];cell=int(f[3],16);point=list(map(float,f[4:7]));near=point[:];near[0]+=2
                speed=visual_speeds[int(f[2],16)]*(1. if f[11]=='-' else float(f[11]))
                for p in peers:
                    mark=len(p.events)
                    p.send('move',cell=cell,position=near,heading=0,motion=[0,0,0])
                    event=until(p,0,'object',lambda e:e['id']==str(target))
                    assert event['placement']['cell']==cell
                    assert all(abs(a-b)<.001 for a,b in zip(event['placement']['position'],point))
                    assert event['entity']==int(f[1],16)
                    until(p,0,'quality',lambda e:e.get('id')==str(target) and e.get('key')==0x107 and e['value']>0)
                a,b=peers;mark=len(a.events);observed=len(b.events)
                time.sleep(.6) # respect the native chat rate limit
                survey=len(a.events);a.send('chat',text='@spawns',channel='say')
                until(a,survey,'chat',lambda e:'Imported outdoor creatures:' in str(e))
                until(a,survey,'chat',lambda e:f'{cell:08X}' in str(e))
                a.send('attack',id=str(target))
                hit=until(a,mark,'hit',lambda e:e['id']==str(target))
                until(b,observed,'quality',lambda e:e.get('id')==str(target) and e.get('key')==0x102)
                until(b,observed,'mode',lambda e:e['id']==str(target) and e['mode']==0x41000012)
                a.send('stop_attack');until(a,mark,'combat',lambda e:not e['enabled'])
                chase=point[:];chase[0]+=12;chase[2]=chase_height
                ma,mb=len(a.events),len(b.events)
                a.send('move',cell=cell,position=chase,heading=0,motion=[0,0,0])
                moved=until(a,ma,'position',lambda e:e['id']==str(target) and e['position'][0]>point[0]+1)
                observed=until(b,mb,'position',lambda e:e['id']==str(target) and e['time']==moved['time'])
                assert moved['position']==observed['position']
                assert all(abs(v)<.01 for v in moved['motion']), 'Server steps must not encode backwards/downward motion'
                # A late observer gets the moving pose and combat stance, not
                # the capture's old home or an absent/zero initial mode.
                late=len(b.events);b.send('logout',id=b.id);until(b,late,'selection')
                b.send('enter',id=b.id);until(b,late,'player');b.send('ready')
                snapshot=until(b,late,'object',lambda e:e['id']==str(target))
                assert snapshot['placement']['position'][0]>point[0]+.5,snapshot
                until(a,ma,'hit',lambda e:e['id']==a.id,timeout=20)
                # Disengage without creating another spawn. The non-attacking
                # observer remains to verify the walk back to the exact home.
                departed=len(a.events);returned_mark=len(b.events)
                a.send('logout',id=a.id);until(a,departed,'selection')
                returned=until(b,returned_mark,'position',lambda e:e['id']==str(target)
                    and abs(e['position'][0]-point[0])<.04,timeout=20)
                assert all(abs(x-y)<.03 for x,y in zip(returned['position'],point)),returned
                steps=[e for e in b.events[returned_mark:] if e.get('event')=='position' and e['id']==str(target)]
                assert len(steps)>5,'Returning home must walk, not teleport'
                measured=[]
                for previous,current in zip(steps,steps[1:]):
                    dt=current['time']-previous['time']
                    assert dt>0
                    distance=math.hypot(*(current['position'][axis]-previous['position'][axis] for axis in [0,1]))
                    assert distance<=speed*min(dt,.25)+.015,'Native speed limit must bound each update'
                    measured.append(distance/dt)
                assert max(measured)>speed*.9,'Creature must reach its native pace, not the old 2m/s fallback'
                a.send('enter',id=a.id);until(a,departed,'player');a.send('ready');until(a,departed,'status',lambda e:e['phase']=='world')
                report.append({'id':f[0],'entity':f[1],'cell':f[3],'position':point,'hit':hit,
                    'pursuit':moved['position'],'late_observer':snapshot['placement'],'return_steps':len(steps),
                    'native_speed_mps':speed,'measured_peak_mps':max(measured)})
            (run/'result.json').write_text(json.dumps({'result':'PASS','observations':report},indent=2))
            print('PASS: captured outdoor creatures replicate damage, pursue on native terrain, retaliate and walk home; late observers receive their current pose.',run)
        finally:
            for i,p in enumerate(peers):
                (run/f'peer-{i}.json').write_text(json.dumps(p.events));p.close()
            if server.poll() is None:
                (run/'world/stop.request').write_text('stop\n')
                try:server.wait(timeout=10)
                except subprocess.TimeoutExpired:server.terminate();server.wait()


if __name__=='__main__':
    main()
