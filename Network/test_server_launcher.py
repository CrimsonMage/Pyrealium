"""Exercise normal/repeated startup, lock ownership, native login, and shutdown."""
import os
import socket
import subprocess
import time
import uuid
import json
import urllib.request

from test_network import DAT_DIRECTORY, Peer, ROOT, SERVER


def main():
    if not SERVER.is_file():
        raise RuntimeError('Build Pyrealium before testing its existing-binary startup path.')
    directory = ROOT / 'Saved/AC2NetworkTests' / ('launcher-' + uuid.uuid4().hex)
    directory.mkdir(parents=True)
    world = directory / 'world store'
    port = 19490
    account_port = 19492
    # Reproduce the user's crashed-launch marker without touching their store.
    exited=subprocess.Popen(['cmd.exe','/d','/c','exit','0'],creationflags=subprocess.CREATE_NO_WINDOW)
    exited.wait()
    world.mkdir()
    (world/'server.lock').write_text(str(exited.pid)+'\n')
    (directory/'accounts').mkdir()
    (directory/'accounts/accounts.lock').write_text(str(exited.pid)+'\n')
    for number in (port, port + 1):
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as check:
            check.bind(('127.0.0.1', number))
    log_path = directory / 'launcher.log'
    peer = None
    web = None
    password = uuid.uuid4().hex
    with log_path.open('w', encoding='utf-8') as log:
        process = subprocess.Popen([
            'cmd.exe', '/d', '/c', str(ROOT / 'start-pyrealium.bat'),
            '-Port', str(port), '-DataDirectory', str(world),
            '-AccountPort', str(account_port),
            '-DatDirectory', str(DAT_DIRECTORY),
        ], cwd=directory, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
            creationflags=subprocess.CREATE_NO_WINDOW)
        try:
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                output = log_path.read_text(encoding='utf-8', errors='replace')
                assert process.poll() is None, output
                # The launcher prepares content before starting the native process.
                if 'Pyrealium: UDP' in output:
                    break
                time.sleep(.1)
            else:
                raise AssertionError('Launcher did not start the server: ' + output)
            assert 'Pyrealium source offer:' not in output, 'Normal startup unexpectedly rebuilt the server.'
            assert 'Recovered stale server.lock' in output and 'Recovered stale accounts.lock' in output,output
            peer = Peer()
            request=urllib.request.Request(f'http://127.0.0.1:{account_port}/api/login',
                data=json.dumps({'account':'launcher_test','password':password}).encode(),
                headers={'Content-Type':'application/json'},method='POST')
            with urllib.request.urlopen(request,timeout=10) as response:login=json.load(response)
            assert login['created'] is True
            peer.send('connect', host=login['game_host'], port=login['game_port'], account=login['account'],ticket=login['ticket'])
            login=None
            peer.wait('characters')
            peer.close()
            peer = None
            lock = world / 'server.lock'
            lock_owner = lock.read_text()
            content = world / 'legions-world.tsv'
            content_time = content.stat().st_mtime_ns
            repeated = subprocess.run([
                'cmd.exe', '/d', '/c', str(ROOT / 'start-pyrealium.bat'),
                '-Port', str(port), '-DataDirectory', str(world),
                '-DatDirectory', str(DAT_DIRECTORY),
            ], cwd=directory, stdin=subprocess.DEVNULL, capture_output=True, text=True,
                timeout=15, creationflags=subprocess.CREATE_NO_WINDOW)
            assert repeated.returncode == 0, repeated.stdout + repeated.stderr
            assert 'already running' in repeated.stdout, repeated.stdout
            assert lock.read_text() == lock_owner
            assert content.stat().st_mtime_ns == content_time, 'Repeated startup rewrote live content.'
            rebuild = subprocess.run([
                'powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass',
                '-File', str(ROOT / 'Start-Pyrealium.ps1'), '-Rebuild',
                '-DataDirectory', str(world),
            ], cwd=directory, capture_output=True, text=True, timeout=15,
                creationflags=subprocess.CREATE_NO_WINDOW)
            assert rebuild.returncode != 0 and 'before rebuilding' in rebuild.stderr, rebuild
            # A recycled PID (here this Python process) must never be accepted
            # as a server owner, nor authorize removal of the store's lock.
            other_store = directory / 'unverified store'
            other_store.mkdir()
            other_lock = other_store / 'server.lock'
            other_lock.write_text(str(os.getpid()) + '\n')
            rejected = subprocess.run([
                'powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass',
                '-File', str(ROOT / 'Start-Pyrealium.ps1'),
                '-DataDirectory', str(other_store),
            ], cwd=directory, capture_output=True, text=True, timeout=15,
                creationflags=subprocess.CREATE_NO_WINDOW)
            assert rejected.returncode != 0 and 'cannot be confirmed' in rejected.stderr, rejected
            assert other_lock.read_text().strip() == str(os.getpid())
            # Use the same public launcher for graceful shutdown, from another cwd.
            subprocess.run([
                'powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass',
                '-File', str(ROOT / 'Start-Pyrealium.ps1'), '-Stop',
                '-DataDirectory', str(world),
            ], cwd=directory, stdout=log, stderr=log, check=True, timeout=30,
                creationflags=subprocess.CREATE_NO_WINDOW)
            assert process.wait(timeout=10) == 0, log_path.read_text(errors='replace')
            assert not lock.exists(), 'Graceful shutdown did not release the store lock.'
            assert not (directory/'accounts/accounts.lock').exists(), 'Owned account service was not stopped.'

            # Reproduce the live account child left after its launcher exits.
            # The process below quotes every switch, exactly like Start-Process
            # in Start-Pyrealium.ps1; stores contain spaces to exercise Windows argv.
            # The independent account copy may remain running while game
            # builds are updated. Verify native tickets across both binaries.
            account_args = [str(ROOT/'target/release/pyrealium.exe'), 'account-web', '--auth-data', str(directory/'accounts'),
                            '--data', str(world), '--web-bind', f'127.0.0.1:{account_port}',
                            '--public-origin', f'http://127.0.0.1:{account_port}',
                            '--public-host', '127.0.0.1', '--public-port', str(port)]
            web = subprocess.Popen(' '.join('"'+a+'"' for a in account_args),
                                   stdout=log, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW)
            account_lock = directory/'accounts/accounts.lock'
            deadline = time.monotonic()+10
            while time.monotonic()<deadline:
                assert web.poll() is None, log_path.read_text(errors='replace')
                try:
                    with urllib.request.urlopen(f'http://127.0.0.1:{account_port}/health', timeout=1) as response:
                        if json.load(response)['status']=='ok':
                            break
                except OSError:
                    time.sleep(.1)
            else:
                raise AssertionError('Standalone account service did not become ready')
            account_owner = account_lock.read_text()
            launcher = ['powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass',
                        '-File', str(ROOT/'Start-Pyrealium.ps1'), '-SkipBuild',
                        '-DataDirectory', str(world), '-AuthDirectory', str(directory/'accounts'),
                        '-Port', str(port), '-AccountPort', str(account_port),
                        '-DatDirectory', str(DAT_DIRECTORY), '-Content', str(content)]
            # Refuse mismatched services without disturbing their process/store.
            for setting, value, option in (
                    ('-GameHost', 'other.example.com', '--public-host'),
                    ('-PublicOrigin', 'https://accounts.example.com', '--public-origin'),
                    ('-Port', str(port+10), '--public-port'),
                    ('-AccountPort', str(account_port+10), '--web-bind'),
                    ('-DataDirectory', str(directory/'other world'), '--data')):
                incompatible = launcher.copy()
                if setting in incompatible:
                    incompatible[incompatible.index(setting)+1] = value
                else:
                    incompatible += [setting, value]
                rejected = subprocess.run(incompatible, cwd=directory, capture_output=True, text=True,
                                          timeout=15, creationflags=subprocess.CREATE_NO_WINDOW)
                assert rejected.returncode != 0 and option in rejected.stderr, rejected
                assert web.poll() is None and account_lock.read_text()==account_owner
                assert not lock.exists(), 'Mismatched account service started a game server'
            # A PID marker naming a different executable cannot authorize reuse.
            unverified_auth = directory/'unverified accounts'
            unverified_auth.mkdir()
            (unverified_auth/'accounts.lock').write_text(str(os.getpid())+'\n')
            invalid = launcher.copy()
            invalid[invalid.index('-AuthDirectory')+1] = str(unverified_auth)
            rejected = subprocess.run(invalid, cwd=directory, capture_output=True, text=True,
                                      timeout=15, creationflags=subprocess.CREATE_NO_WINDOW)
            assert rejected.returncode != 0 and 'cannot be confirmed' in rejected.stderr, rejected
            assert (unverified_auth/'accounts.lock').read_text().strip()==str(os.getpid())

            process = subprocess.Popen(launcher, cwd=directory, stdout=log, stderr=log,
                                       creationflags=subprocess.CREATE_NO_WINDOW)
            deadline=time.monotonic()+20
            while time.monotonic()<deadline:
                output=log_path.read_text(errors='replace')
                assert process.poll() is None, output
                if 'Reusing matching account service' in output and lock.exists():
                    break
                time.sleep(.1)
            else:
                raise AssertionError('Account reuse did not start the game: '+output)
            # Existing credentials still work and produce native game tickets.
            with urllib.request.urlopen(request,timeout=10) as response:login=json.load(response)
            assert login['created'] is False
            peer=Peer()
            peer.send('connect', host=login['game_host'], port=login['game_port'],
                      account=login['account'], ticket=login['ticket'])
            login=None
            peer.wait('characters')
            peer.close()
            peer=None
            (world/'stop.request').write_text('stop\n')
            assert process.wait(timeout=15)==0, log_path.read_text(errors='replace')
            assert not lock.exists()
            assert web.poll() is None and account_lock.read_text()==account_owner, 'Reused account service was stopped'
            print('PASS: startup, native login, repeated start, live rebuild rejection, '
                  'stale recovery, unverified lock preservation, matching account reuse, '
                  'configuration mismatch rejection, and graceful shutdown; log:', log_path)
        finally:
            if peer:
                peer.close()
            if process.poll() is None:
                world.mkdir(parents=True, exist_ok=True)
                (world / 'stop.request').write_text('stop\n')
                try:
                    process.wait(timeout=8)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            if web and web.poll() is None:
                (directory/'accounts/account-stop.request').write_text('stop\n')
                web.wait(timeout=10)


if __name__ == '__main__':
    main()
