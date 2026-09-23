#!/usr/bin/env python3
"""Real browser streams against an isolated Station."""
import concurrent.futures
import importlib.util
import json
import os
from pathlib import Path
import queue
import shutil
import sqlite3
import subprocess
import tempfile
import threading
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('fixture', ROOT / 'scripts/test-mesh.py')
f = importlib.util.module_from_spec(spec)
spec.loader.exec_module(f)
REPORT = Path(os.environ.get('ZORK_TEST_ARTIFACT_DIR', ROOT / 'artifacts/mesh-notify-followup'))
TOKEN = 'notification-fixture'


def request(node, method, path, body=None):
    req = Request(node.url + path, method=method,
                  headers={'Authorization': 'Bearer ' + TOKEN, 'Content-Type': 'application/json'},
                  data=None if body is None else json.dumps(body).encode())
    try:
        response = urlopen(req, timeout=30)
    except HTTPError as error:
        response = error
    with response:
        return response.status, json.load(response)


def admin(node, method, path, body=None):
    status, value = request(node, method, path, body)
    assert status in (200, 201, 202), (status, value)
    return value


class Events:
    def __init__(self, stream, sse=False):
        self.queue = queue.Queue()
        self.values = []
        def read():
            try:
                name, data = '', []
                for raw in stream:
                    line = raw.decode().rstrip('\r\n')
                    if not sse:
                        value = json.loads(line)
                        self.values.append(value)
                        self.queue.put(value)
                    elif line.startswith('event: '):
                        name = line[7:]
                    elif line.startswith('data: '):
                        data.append(line[6:])
                    elif not line and data:
                        value = {'name': name, 'data': json.loads('\n'.join(data))}
                        self.values.append(value)
                        self.queue.put(value)
                        name, data = '', []
            except Exception as error:
                self.queue.put(error)
            finally:
                self.queue.put(None)
        threading.Thread(target=read, daemon=True).start()

    def until(self, check, timeout=35):
        deadline = time.monotonic() + timeout
        while True:
            value = self.queue.get(timeout=max(.01, deadline - time.monotonic()))
            assert isinstance(value, dict), ('event source stopped', value)
            if check(value):
                return value


def main():
    REPORT.mkdir(parents=True, exist_ok=True)
    helpers, streams = [], []
    results = []
    with tempfile.TemporaryDirectory(prefix='zork-notify-', dir='/tmp') as scratch:
        root = Path(scratch)
        os.environ['ZORK_REGISTRY_DIR'] = str(root / 'registry')
        node = f.Node(root / 'authority')
        node.config['admin'] = {'token': TOKEN}
        (node.root / 'config.json').write_text(json.dumps(node.config))
        node.start()
        try:
            f.wait(lambda: node.request('GET', '/readyz')[0] == 200, 'Station ready')
            f.wait(lambda: admin(node, 'GET', '/v1/node/mesh').get('origin'), 'Mesh ready')
            admin(node, 'POST', '/v1/node/agents', {'id': 'browser', 'name': 'Browser fixture', 'role': 'leader',
                'profile_id': 'fixture', 'model': 'fixture-model', 'thinking': 'off'})
            session = admin(node, 'POST', '/v1/node/agents/browser/open', {})['session_id']
            base = '/v1/im/sessions/' + session + '/browser/'
            registration = {'client_id': 'fixture-browser', 'secret': 'a' * 64, 'name': 'Fixture', 'replies': []}
            def open_events():
                stream = urlopen(Request(node.url + base + 'events', method='POST',
                    headers={'Authorization': 'Bearer ' + TOKEN, 'Content-Type': 'application/json'},
                    data=json.dumps(registration).encode()), timeout=35)
                streams.append(stream)
                result = Events(stream, True)
                result.until(lambda v: v['name'] == 'browser_commands')
                return result
            first = open_events()
            wrong = {**registration, 'secret': 'b' * 64}
            assert request(node, 'POST', base + 'events', wrong)[0] == 400
            second = open_events()
            first.until(lambda v: v['name'] == 'error', timeout=3)
            # Staying idle longer than the former 20s lease must preserve the stream.
            deadline = time.monotonic() + 22
            while time.monotonic() < deadline:
                try:
                    event = second.queue.get(timeout=deadline - time.monotonic())
                    assert event and event['name'] == 'heartbeat', event
                except queue.Empty:
                    break
            with concurrent.futures.ThreadPoolExecutor() as pool:
                command = {'session_id': session, 'device_id': 'fixture-browser',
                           'command': {'request_id': 'once', 'action': {'op': 'list'}}}
                delivered = pool.submit(admin, node, 'POST', '/v1/browser/command', command)
                frame = second.until(lambda v: v['name'] == 'browser_commands' and v['data']['commands'])
                assert frame['data']['commands'][0]['request_id'] == 'once'
                receipt = {**registration, 'replies': [{'request_id': 'once', 'result': {'tabs': [], 'fixture': 'done'}}]}
                assert admin(node, 'POST', base + 'receipts', receipt)['accepted'] == ['once']
                assert delivered.result(timeout=5) == {'tabs': [], 'fixture': 'done'}
                assert admin(node, 'POST', '/v1/browser/command', command) == {'tabs': [], 'fixture': 'done'}
                assert admin(node, 'POST', base + 'receipts', receipt)['accepted'] == ['once']
            results.append('browser_idle_heartbeat_reconnect_generation_and_idempotent_receipts')
            admin(node, 'POST', base + 'receipts', {**registration, 'disconnect': True})
            second.until(lambda v: v['name'] == 'error', timeout=3)
            assert request(node, 'POST', base + 'events', registration)[0] == 400
            assert request(node, 'POST', base + 'poll', registration)[0] == 400
            results.append('takeover_revokes_late_reconnect_and_old_poll_registration_is_rejected')
            (REPORT / 'browser-events.json').write_text(json.dumps(second.values, indent=2))
        finally:
            for helper, log in helpers:
                if helper.poll() is None:
                    helper.terminate()
                    try: helper.wait(timeout=35)
                    except subprocess.TimeoutExpired: helper.kill(); helper.wait()
                log.close()
            node.stop()
            for stream in streams:
                stream.close()
            for log in node.root.glob('*.log'):
                shutil.copy2(log, REPORT / ('notification-' + log.name))
    (REPORT / 'protocol-result.json').write_text(json.dumps(results, indent=2))
    print(json.dumps({'checks': results, 'count': len(results)}, indent=2))


if __name__ == '__main__':
    main()
