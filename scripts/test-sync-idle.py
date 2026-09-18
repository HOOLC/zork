#!/usr/bin/env python3
"""Exercise the real Station HTTP/SSE path: reads must never trigger more reads."""
import json
import os
from pathlib import Path
import queue
import socket
import subprocess
import tempfile
import threading
import time
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get('ZORK_TEST_BIN_DIR', str(ROOT / 'target/debug'))) / 'zork-station'


def main():
    with tempfile.TemporaryDirectory(prefix='zork-sync-idle-') as scratch:
        root = Path(scratch)
        sockets = [socket.socket() for _ in range(4)]
        for sock in sockets:
            sock.bind(('127.0.0.1', 0))
        bind = dict(zip(('station', 'runtime', 'control', 'agent'),
                        (f'127.0.0.1:{sock.getsockname()[1]}' for sock in sockets)))
        for sock in sockets:
            sock.close()
        token = 'isolated-sync-fixture'
        (root / 'config.json').write_text(json.dumps({
            'bind': bind, 'admin': {'token': token}, 'mesh': {'enabled': False, 'name': 'Before'},
        }))
        url = 'http://' + bind['runtime']

        def request(method, path, body=None):
            req = Request(url + path, method=method,
                          headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'},
                          data=None if body is None else json.dumps(body).encode())
            with urlopen(req, timeout=10) as response:
                return json.load(response)

        def pull(after=None):
            reply = request('POST', '/v1/node/sync', {'scope': {'type': 'catalog'}, 'after': after})
            assert reply['status'] == 'page', reply
            assert reply['page']['last'], 'fixture unexpectedly requires pagination'
            return reply['page']

        events = queue.Queue()
        with (root / 'station.log').open('wb') as log:
            station = subprocess.Popen([str(BINARY), '--data', str(root), '--fake-agent'], stdout=log, stderr=log)
            try:
                end = time.monotonic() + 30
                while True:
                    try:
                        initial = pull()
                        break
                    except (OSError, ValueError):
                        if station.poll() is not None or time.monotonic() > end:
                            raise AssertionError((root / 'station.log').read_text())
                        time.sleep(.05)

                def subscribe():
                    try:
                        with urlopen(Request(url + '/v1/im/events', headers={'Authorization': 'Bearer ' + token}), timeout=15) as response:
                            for line in response:
                                if line.startswith(b'data:'):
                                    events.put(json.loads(line[5:]))
                    except OSError:
                        pass  # The fixture deliberately closes the Station at the end.

                reader = threading.Thread(target=subscribe, daemon=True)
                reader.start()
                hint = events.get(timeout=5)
                assert hint['catalog'] == initial['through']
                for _ in range(20):
                    page = pull(initial['through'])
                    assert page['through'] == initial['through'] and not page['records']
                    assert request('POST', '/v1/node/sync/receipt', {'request_id': 'unknown'}) is None
                try:
                    raise AssertionError(f'read emitted notification: {events.get(timeout=.5)}')
                except queue.Empty:
                    pass

                request('PUT', '/v1/node/name', {'name': 'After'})
                deadline = time.monotonic() + 5
                renamed = False
                count = 0
                while time.monotonic() < deadline:
                    try:
                        events.get(timeout=.5)
                    except queue.Empty:
                        if renamed:
                            break
                        continue
                    count += 1
                    assert count < 20, 'notification/pull feedback loop'
                    page = pull()
                    renamed |= any(r['kind'] == 'device' and r['value']['name'] == 'After' for r in page['records'])
                assert renamed, 'file-backed name change was not projected'
                assert events.empty()
                request('PUT', '/v1/node/profiles/toggle', {'provider': 'openai-compatible', 'billing': 'usage', 'base_url': 'http://127.0.0.1:9/v1', 'auth': {'key': 'fixture'}, 'models': [{'id': 'model', 'api': 'openai-completions', 'thinking': ['off'], 'default_thinking': 'off', 'capabilities': {'input': ['text']}, 'limits': {'context_window_tokens': 32000, 'max_output_tokens': 4096}, 'default': True}]})
                request('PUT', '/v1/node/profiles/toggle/models/enabled', {'model_id': 'model', 'enabled': False})
                snapshot = pull()
                assert any(r.get('value', {}).get('profile_id') == 'toggle' and r['value']['models'][0]['enabled'] is False for r in snapshot['records'])
                while True:
                    try: events.get(timeout=.5)
                    except queue.Empty: break
                for _ in range(10):
                    request('PUT', '/v1/node/profiles/toggle/models/enabled', {'model_id': 'model', 'enabled': False})
                    unchanged = pull(snapshot['through'])
                    assert unchanged['through'] == snapshot['through'] and not unchanged['records']
                try: raise AssertionError(f'idempotent model toggle emitted notification: {events.get(timeout=.5)}')
                except queue.Empty: pass
                # Upgrade progress shares the durable catalog notification path.
                temporary = root / 'run/update.json.tmp'
                temporary.write_text(json.dumps({'phase': 'complete', 'version': '1.2.3', 'message': 'fixture'}))
                temporary.replace(root / 'run/update.json')
                deadline = time.monotonic() + 5
                while True:
                    assert time.monotonic() < deadline, 'upgrade progress notification missing'
                    events.get(timeout=5)
                    page = pull()
                    if any(record.get('value', {}).get('update', {}).get('status', {}).get('phase') == 'complete' for record in page['records']):
                        break
                print(f'PASS: 20 sync reads + 20 receipt reads stayed quiet; rename converged and stopped after {count} hints; model disable converged and repeated writes stayed quiet')
            finally:
                station.terminate()
                try:
                    assert station.wait(timeout=15) == 0
                except subprocess.TimeoutExpired:
                    station.kill()
                    station.wait()
                    raise


if __name__ == '__main__':
    main()
