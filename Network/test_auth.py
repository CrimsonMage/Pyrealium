"""Account-service login and ticket interoperability over loopback; secrets stay in memory."""
import io
import json
import os
import subprocess
import time
import urllib.error
import urllib.request
import uuid
import zipfile
from test_network import Peer, ROOT, SERVER, DAT_DIRECTORY, prepare_test_content

def main():
    directory=ROOT/'Saved/AC2NetworkTests'/('auth-'+uuid.uuid4().hex)
    directory.mkdir(parents=True)
    content=prepare_test_content(directory)
    auth=directory/'accounts';world=directory/'world'
    log=(directory/'server.log').open('w',encoding='utf-8')
    env=dict(os.environ,PYREALIUM_UNREAL_LEGIONS_DATA='1')
    web=subprocess.Popen([str(SERVER),'account-web','--auth-data',str(auth),'--data',str(world),
        '--web-bind','127.0.0.1:19280','--public-origin','http://127.0.0.1:19280',
        '--public-host','127.0.0.1','--public-port','19282'],stdout=log,stderr=log,creationflags=0x08000000)
    game=None;peers=[]
    def post(path,password,account='network_test'):
        request=urllib.request.Request('http://127.0.0.1:19280/api/'+path,
            data=json.dumps({'account':account,'password':password}).encode(),
            headers={'Content-Type':'application/json'},method='POST')
        with urllib.request.urlopen(request,timeout=10) as response:return json.load(response)
    try:
        time.sleep(.6)
        password='Pyrealium-'+uuid.uuid4().hex
        first=post('login',password)
        assert first['created'] is True
        first=None
        # A new service process must retain the original password, not register again.
        (auth/'account-stop.request').write_text('stop\n')
        assert web.wait(timeout=8)==0
        web=subprocess.Popen([str(SERVER),'account-web','--auth-data',str(auth),'--data',str(world),
            '--web-bind','127.0.0.1:19280','--public-origin','http://127.0.0.1:19280',
            '--public-host','127.0.0.1','--public-port','19282'],stdout=log,stderr=log,creationflags=0x08000000)
        time.sleep(.6)
        try:post('login','incorrect-password-123')
        except urllib.error.HTTPError as e:assert e.code==401
        else:raise AssertionError('Wrong password accepted')
        login=post('login',password)
        assert login['created'] is False
        try:post('login','short','invalid_password_test')
        except urllib.error.HTTPError as e:assert e.code==400
        else:raise AssertionError('Short initial password accepted')
        for name in ('local','admin'):
            try:post('login',password,name)
            except urllib.error.HTTPError as e:assert e.code==409
            else:raise AssertionError('Reserved legacy account was claimed')
        password=None
        game=subprocess.Popen([str(SERVER),'serve','--bind','127.0.0.1:19282','--auth-data',str(auth),
            '--data',str(world),'--game-data',str(DAT_DIRECTORY),
            '--content',str(content),
            '--training-kit','false','--lifestones','false','--portals','false','--seconds','90'],
            env=env,stdout=log,stderr=log,creationflags=0x08000000)
        time.sleep(.7)
        assert game.poll() is None
        valid=Peer();peers.append(valid)
        valid.send('connect',host=login['game_host'],port=login['game_port'],account=login['account'],ticket=login['ticket'])
        valid.wait('characters')
        replay=Peer();peers.append(replay)
        replay.send('connect',host=login['game_host'],port=login['game_port'],account=login['account'],ticket=login['ticket'])
        login=None
        no_ticket=Peer();peers.append(no_ticket)
        no_ticket.send('connect',host='127.0.0.1',port=19282,account='network_test')
        time.sleep(2)
        assert not any(e.get('event')=='characters' for p in [replay,no_ticket] for e in p.events)
        source=urllib.request.urlopen('http://127.0.0.1:19280/source.zip',timeout=10).read()
        with zipfile.ZipFile(io.BytesIO(source)) as z:
            assert 'emulator/src/server.rs' in z.namelist()
            assert 'LICENSE' in z.namelist()
            assert not any('accounts.journal' in name or '/data/' in name for name in z.namelist())
        report={'result':'PASS','checks':['first login creates account','password persists across service restart','wrong password rejected without overwrite','reserved legacy accounts protected','native one-use ticket login','ticket replay rejected','anonymous login rejected','corresponding source downloadable'],'directory':str(directory)}
        (directory/'result.json').write_text(json.dumps(report,indent=2))
        print(json.dumps(report))
    finally:
        for p in peers:p.close()
        if game and game.poll() is None:
            (world/'stop.request').write_text('stop\n')
            try:game.wait(timeout=8)
            except subprocess.TimeoutExpired:game.kill()
        if web.poll() is None:
            (auth/'account-stop.request').write_text('stop\n')
            try:web.wait(timeout=8)
            except subprocess.TimeoutExpired:web.kill()
        log.close()

if __name__=='__main__':main()
