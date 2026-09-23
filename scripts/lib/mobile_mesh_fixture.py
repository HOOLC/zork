"""Isolated native client process and Station setup helpers for business tests."""
import importlib.util
import json
import os
from pathlib import Path
import queue
import sqlite3
import subprocess
import tempfile
import threading
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('fixture', ROOT / 'scripts/test-mesh.py')
f = importlib.util.module_from_spec(spec)
spec.loader.exec_module(f)
TOKEN = 'isolated-directory-fixture'


def admin(node, method, path, body=None):
    request = Request(node.url + path, method=method,
        headers={'Authorization': 'Bearer ' + TOKEN, 'Content-Type': 'application/json'},
        data=None if body is None else json.dumps(body).encode())
    try:
        response = urlopen(request, timeout=20)
    except HTTPError as error:
        response = error
    with response:
        value = json.loads(response.read())
        assert response.status in (200, 201, 202), (response.status, value)
        return value


def ready(node):
    f.wait(lambda: admin(node, 'GET', '/v1/node/mesh').get('origin') == node.origin, 'Station ready')


def join(node, inviter):
    invite = admin(inviter, 'POST', '/v1/node/mesh/invites')
    progress = admin(node, 'POST', '/v1/node/mesh/join', {'invitation': invite['invitation']})
    def finished():
        value = admin(node, 'GET', '/v1/node/mesh/join/' + progress['id'])
        return value if value['finished'] else None
    result = f.wait(finished, 'Station join')
    assert not result['error'] and result['result']['joined'], result


class NativeClient:
    def __init__(self, root):
        self.root, self.sequence, self.snapshots, self.responses = root, 0, {}, {}
        root.mkdir(exist_ok=True)
        self.log = (root / 'fixture.log').open('ab')
        self.process = subprocess.Popen([str(f.TARGET / 'examples/client-fixture'), str(root)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log, text=True, bufsize=1)
        self.inbox = queue.Queue()
        def read():
            for line in self.process.stdout:
                self.inbox.put(json.loads(line))
            self.inbox.put(None)
        self.reader = threading.Thread(target=read, daemon=True)
        self.reader.start()

    def receive(self, timeout):
        value = self.inbox.get(timeout=max(.01, timeout))
        assert value is not None, 'phone core exited; inspect fixture.log'
        if 'source' in value:
            self.snapshots[value['source']] = value['event']['snapshot']
        else:
            self.responses[value['id']] = value

    def wait(self, source, predicate, timeout=60):
        deadline = time.monotonic() + timeout
        while not predicate(self.snapshots.get(source, {})):
            assert time.monotonic() < deadline, (source, self.snapshots.get(source))
            self.receive(deadline - time.monotonic())
        return self.snapshots[source]

    def command(self, op, expect_error=False, **arguments):
        self.sequence += 1
        identity = self.sequence
        self.process.stdin.write(json.dumps({'id': identity, 'command': {'op': op, **arguments}}) + '\n')
        self.process.stdin.flush()
        deadline = time.monotonic() + 70
        while identity not in self.responses:
            self.receive(deadline - time.monotonic())
        response = self.responses.pop(identity)
        assert ('error' in response) == expect_error, response
        return response.get('error') if expect_error else response['result']

    def read(self, peer, path='/v1/node/info'):
        value = self.command('read', peer=peer, path=path)
        assert value.get('cached') is False and not value.get('error'), value
        return value['snapshot']['body']

    def nodes(self, origins, timeout=60):
        return self.wait('directory', lambda value:
            value.get('running') and {n['id'] for n in value.get('nodes', [])} == set(origins), timeout)

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            try:
                assert self.process.wait(timeout=15) == 0
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
                raise
        self.reader.join(timeout=2)
        self.log.close()

