#!/usr/bin/env python3
"""Real Station processes + the phone core; dynamic Mesh membership and recovery.

Uses online endpoints with an unavailable local cloud service and automatic
addresses. No user data, fixed peer routes, device installs or OS network changes.
"""
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

ROOT = Path(__file__).resolve().parents[1]
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


class Phone:
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


def phone_invite(phone, node, switch_from=None):
    invite = admin(node, 'POST', '/v1/node/mesh/client-invites')
    result = phone.command('begin_invitation', ticket=invite['invitation'], name='Directory phone', switch_from=switch_from)
    return invite, result


def approve(phone, node, invite):
    phone.wait('invitation', lambda value: (value.get('invitation') or {}).get('id') == invite['id']
        and value['invitation'].get('status') == 'awaiting_approval')
    claim = next(i for i in admin(node, 'GET', '/v1/node/mesh/invites')['items'] if i['id'] == invite['id'])
    admin(node, 'POST', '/v1/node/mesh/invites/' + invite['id'] + '/approve',
        {'origin': claim['device']['origin'], 'claim_id': claim['claim_id']})
    return phone.wait('invitation', lambda value: value.get('done') and value.get('invitation') is None)


def main():
    root = Path(tempfile.mkdtemp(prefix='zmob-', dir='/tmp'))
    print('Isolated online mobile Mesh:', root, flush=True)
    os.environ['ZORK_REGISTRY_DIR'] = str(root / 'registry')
    nodes, phone, checks = [], None, []
    def passed(name):
        checks.append(name)
        print('PASS:', name, flush=True)
    try:
        for name in ('a', 'b', 'c', 'switch-target'):
            node = f.Node(root / name)
            nodes.append(node)
            node.config['admin'] = {'token': TOKEN}
            node.config['mesh'].update(name=name, offline=False, bind=None,
                relay_urls=['http://127.0.0.1:9'], discovery_url='http://127.0.0.1:9/pkarr', quic_discovery_urls=[])
            (node.root / 'config.json').write_text(json.dumps(node.config))
            node.start()
            ready(node)
        a, b, c, d = nodes
        join(b, a)
        phone = Phone(root / 'phone')
        invite, _ = phone_invite(phone, a)
        accepted = approve(phone, a, invite)
        identity = accepted['identity']
        phone.nodes([a.origin, b.origin])
        phone.command('draft', peer=b.origin, session='preserved-draft', content='draft before switch')
        for peer in (a, b):
            assert phone.read(peer.origin)['name'] == peer.root.name
        passed('approved phone discovers and uses every existing member through core subscriptions')

        join(c, b)
        phone.nodes([a.origin, b.origin, c.origin])
        assert phone.read(c.origin)['name'] == 'c'
        passed('a member added later appears without opening the inviter or restarting the phone')

        admin(a, 'POST', '/v1/node/mesh/members/remove', {'origin': c.origin})
        phone.nodes([a.origin, b.origin])
        phone.command('read', peer=c.origin, path='/v1/node/info', expect_error=True)
        passed('membership removal revokes the phone connection and removes the directory entry')

        a.stop()
        old = admin(b, 'GET', '/v1/node/mesh')['address']
        b.restart_station()
        f.wait(lambda: admin(b, 'GET', '/v1/node/mesh').get('origin') == b.origin, 'member restart')
        assert admin(b, 'GET', '/v1/node/mesh')['address'] != old, 'fixture failed to rotate ephemeral endpoint'
        phone.command('pause')
        phone.command('resume')
        phone.nodes([a.origin, b.origin])
        assert phone.read(b.origin)['name'] == 'b'
        phone.close()
        phone = Phone(root / 'phone')
        assert phone.command('resume')['identity'] == identity
        phone.nodes([a.origin, b.origin])
        assert phone.read(b.origin)['name'] == 'b'
        passed('inviter offline, member port change and phone process restart preserve identity and usable peers')

        invite, preview = phone_invite(phone, d)
        expected = preview['switch_confirmation']['expected']
        assert {n['id'] for n in preview['nodes']} == {a.origin, b.origin}
        phone.command('cancel_invitation')
        phone.nodes([a.origin, b.origin])
        phone.command('begin_invitation', ticket=invite['invitation'], name='Directory phone', switch_from=expected)
        phone.wait('invitation', lambda value: (value.get('invitation') or {}).get('status') == 'awaiting_approval')
        phone.command('cancel_invitation')
        phone.nodes([a.origin, b.origin])
        assert phone.read(b.origin)['name'] == 'b'
        admin(d, 'DELETE', '/v1/node/mesh/invites/' + invite['id'])
        phone.command('begin_invitation', ticket=invite['invitation'], name='Directory phone', switch_from=expected, expect_error=True)
        phone.nodes([a.origin, b.origin])
        passed('switch preview, cancel and rejected invitation keep the current Mesh usable')

        invite, _ = phone_invite(phone, d, expected)
        approve(phone, d, invite)
        phone.nodes([d.origin])
        assert phone.read(d.origin)['name'] == 'switch-target'
        phone.close()
        phone = None
        with sqlite3.connect(root / 'phone/client.db') as db:
            draft = db.execute("SELECT value FROM cache WHERE node=? AND key='draft:preserved-draft'", (b.origin,)).fetchone()
            assert draft and 'draft before switch' in draft[0]
        passed('confirmed switch replaces only that Mesh directory and preserves local drafts')
        report = Path(os.environ.get('ZORK_TEST_ARTIFACT_DIR', ROOT / 'artifacts/mobile-mesh-directory'))
        report.mkdir(parents=True, exist_ok=True)
        (report / 'result.json').write_text(json.dumps({'fixture': str(root), 'checks': checks}, indent=2))
    finally:
        if phone:
            phone.close()
        for node in nodes:
            node.stop()


if __name__ == '__main__':
    main()
