#!/usr/bin/env python3
"""Real authenticated HTTP upgrade against an isolated, offline release fixture."""
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import socket
import subprocess
import sys
import tarfile
import tempfile
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]
TARGET = Path(os.environ.get('ZORK_TEST_TARGET', ROOT / 'target/debug'))


def wait(check, timeout=60):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        try:
            value = check()
            if value:
                return value
        except (OSError, ValueError, KeyError) as error:
            last = error
        time.sleep(.1)
    raise AssertionError(f'Timed out: {last}')


def port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def main():
    with tempfile.TemporaryDirectory(prefix='zgu-', dir='/tmp') as temporary:
        root = Path(temporary).resolve()
        node = root / 'node'
        bundle = node / 'bin'
        bundle.mkdir(parents=True)
        for name in ['zork', 'zork-station', 'zork-agent']:
            shutil.copy2(TARGET / name, bundle / name)
        (bundle / 'zork-gh').write_text('#!/bin/sh\nexit 0\n')
        (bundle / 'zork-gh').chmod(0o755)
        (bundle / 'VERSION').write_text('0.1.30')
        binds = {name: f'127.0.0.1:{port()}' for name in ['station', 'runtime', 'control', 'agent']}
        (node / 'config.json').write_text(json.dumps({'bind': binds, 'mesh': {'enabled': False}, 'im_connections': []}))
        (node / 'service.json').write_text('{"enabled":true,"start_at_login":false}')
        (node / 'user-data').write_text('preserve me')
        tools = root / 'tools'
        tools.mkdir()
        # Only curl's fixture executable substitutes the fixed public release URL.
        # No production API accepts a caller-selected download source.
        curl = tools / 'curl'
        curl.write_text(f'''#!{sys.executable}
import pathlib, shutil, sys, time
args=sys.argv[1:]
url=next(arg for arg in args if arg.startswith('https://github.com/HOOLC/zork/releases/'))
source=pathlib.Path({str(root / 'releases')!r}) / url.split('/releases/',1)[1]
time.sleep(1)
shutil.copyfile(source,args[args.index('-o')+1])
''')
        curl.chmod(0o755)
        key = ('darwin' if sys.platform == 'darwin' else 'linux') + '-' + ('arm64' if platform.machine() in ['arm64', 'aarch64'] else 'x64')

        def release(version, corrupt=False):
            assets = root / 'releases/download' / ('v' + version)
            assets.mkdir(parents=True)
            archive = assets / f'zork-{version}-{key}.tar.gz'
            with tarfile.open(archive, 'w:gz', compresslevel=1) as tar:
                for name in ['zork', 'zork-station', 'zork-agent', 'zork-gh']:
                    tar.add(bundle / name, arcname=name, recursive=False)
                for name, content in [('VERSION', version), ('LICENSE', 'fixture'), ('Synchronicity.txt', 'fixture')]:
                    member = tarfile.TarInfo(name)
                    data = content.encode()
                    member.size = len(data)
                    tar.addfile(member, io.BytesIO(data))
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            minimum = '15.0' if sys.platform == 'darwin' else '2.39'
            (assets / 'manifest.tsv').write_text(f'{key}\t{archive.name}\t{digest}\t{minimum}\n')
            if corrupt:
                with archive.open('ab') as file:
                    file.write(b'corruption')

        release('0.1.31', corrupt=True)
        release('0.1.32')
        env = dict(os.environ, PATH=f'{tools}:' + os.environ['PATH'], ZORK_REGISTRY_DIR=str(root / 'registry'))
        with (root / 'process.log').open('wb') as log:
            process = subprocess.Popen([str(bundle / 'zork'), 'start', '--data', str(node), '--fake-agent'],
                                       env=env, stdout=log, stderr=log, start_new_session=True)
            try:
                wait(lambda: (node / 'run/node-token.json').exists())
                token = json.loads((node / 'run/node-token.json').read_text())

                def request(method='GET', path='/v1/node/info', body=None, authorized=True):
                    headers = {'Content-Type': 'application/json'}
                    if authorized:
                        headers['Authorization'] = 'Bearer ' + token
                    req = Request('http://' + binds['runtime'] + path, method=method, headers=headers,
                                  data=None if body is None else json.dumps(body).encode())
                    try:
                        response = urlopen(req, timeout=6)
                    except HTTPError as error:
                        response = error
                    with response:
                        return response.status, json.load(response)

                wait(lambda: request()[0] == 200)
                assert request()[1]['update']['supported']
                assert request('POST', '/v1/node/update', {'version': '0.1.31'}, False)[0] == 401
                assert request('POST', '/v1/node/update', {'version': '../bad'})[0] == 400
                before = (node / 'run/zork-station.pid').read_text()
                assert request('POST', '/v1/node/update', {'version': '0.1.31'})[0] == 202
                assert request('POST', '/v1/node/update', {'version': '0.1.31'})[0] == 409
                wait(lambda: request()[1]['update']['status']['phase'] == 'failed')
                assert (node / 'run/zork-station.pid').read_text() == before
                assert (bundle / 'VERSION').read_text() == '0.1.30'
                assert request('POST', '/v1/node/update', {'version': '0.1.32'})[0] == 202
                wait(lambda: request()[1]['update']['status']['phase'] == 'complete')
                info = request()[1]
                assert info['station']['release_version'] == '0.1.32', info
                assert (node / 'run/zork-station.pid').read_text() != before
                assert int((node / 'zork.pid').read_text()) == process.pid
                assert (node / 'user-data').read_text() == 'preserve me'
                assert (next((node / 'run').glob('update-previous-*')) / 'VERSION').read_text() == '0.1.30'
                print('PASS: admin authentication, version validation, concurrent rejection, corrupt download leaves processes untouched, complete HTTP upgrade, stable supervisor PID, retained data and prior bundle')
            except BaseException:
                log.flush()
                print((root / 'process.log').read_text()[-6000:])
                if (node / 'logs/update.log').exists():
                    print((node / 'logs/update.log').read_text()[-6000:])
                raise
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait()


if __name__ == '__main__':
    main()
