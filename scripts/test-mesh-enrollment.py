#!/usr/bin/env python3
"""Three real Stations, one-command enrollment, membership and task ownership."""
import base64
import zlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import socket
import tempfile
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen

spec = importlib.util.spec_from_file_location('fixture', Path(__file__).with_name('test-mesh.py'))
f = importlib.util.module_from_spec(spec)
spec.loader.exec_module(f)


def request(node, method, path, body=None, leader=None):
    headers = {'Content-Type': 'application/json', 'Authorization': 'Bearer enrollment-fixture'}
    if leader:
        headers['x-zork-session-key'] = leader['session_key']
    try:
        response = urlopen(Request(node.url + path, method=method, headers=headers,
            data=None if body is None else json.dumps(body).encode()), timeout=60)
    except HTTPError as error:
        response = error
    with response:
        data = response.read()
        return response.status, json.loads(data) if data else None


def admin(node, method, path, body=None):
    status, value = request(node, method, path, body)
    assert status in (200, 201, 202), (status, value)
    return value


def main():
    root = Path(tempfile.mkdtemp(prefix='zenroll-', dir='/tmp'))
    os.environ['ZORK_REGISTRY_DIR'] = str(root / 'registry')
    nodes = [f.Node(root / name) for name in ('mini1', 'mini2', 'mini3')]
    a, b, c = nodes
    print(f'isolated mesh enrollment: {root}', flush=True)
    for node in nodes:
        node.config['admin'] = {'token': 'enrollment-fixture'}
        node.config['mesh']['name'] = node.root.name
        if os.environ.get('ZORK_TEST_RELAY'):
            node.config['mesh'].update(offline=False,
                relay_urls=[os.environ['ZORK_TEST_RELAY']],
                discovery_url=os.environ.get('ZORK_TEST_DISCOVERY'))
        (node.root / 'config.json').write_text(json.dumps(node.config))
    # Reusing an already-enabled, unpaired Station must import the invite's
    # endpoints, including clearing stale overrides. Offline isolates this test.
    b.config['mesh']['offline'] = True
    b.config['mesh']['relay_urls'] = ['http://127.0.0.1:9']
    b.config['mesh']['discovery_url'] = 'http://127.0.0.1:9/pkarr'
    (b.root / 'config.json').write_text(json.dumps(b.config))
    def pids():
        return [(node.root / 'run/zork-station.pid').read_text() for node in nodes]
    def join(node, invitation):
        result = subprocess.run([str(f.TARGET / 'zork'), 'mesh', 'join', invitation['invitation'], '--data', str(node.root), '--json'],
            capture_output=True, text=True, timeout=65)
        assert result.returncode == 0, result.stderr
        return json.loads(result.stdout)
    try:
        for node in nodes:
            node.start()
        for node in nodes:
            f.wait(lambda: node.request('GET', '/readyz')[0] == 200, 'Station ready')
            f.wait(lambda: urlopen(node.agent_url + '/readyz', timeout=2).status == 200, 'Agent ready')
            f.wait(lambda: node.get('/v1/mesh').get('origin') == node.origin, 'Mesh enrollment ready')
        before = pids()
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as observer:
            observer.settimeout(5)
            observer.connect(str(a.root / 'run/sup.sock'))
            observer.sendall(b'observe\n')
            status = json.loads(observer.makefile('rb').readline())
            assert status['protocol'] == 1
        assert pids() == before and admin(a, 'GET', '/v1/node/info'), 'read-only observer acquired a lifecycle lease'
        invite = admin(a, 'POST', '/v1/node/mesh/invites')
        assert 'releases/download/v' in invite['command'] and 'install.sh | sh -s -- --version' in invite['command'] and 'mesh join' in invite['command']
        assert not (a.root / 'mesh/invites' / (invite['id'] + '.json')).exists(), 'pending invitation leaked to disk'
        assert join(b, invite)['joined']
        applied = json.loads((b.root / 'config.json').read_text())['mesh']
        assert applied.get('relay_urls') == a.config['mesh'].get('relay_urls')
        assert applied.get('discovery_url') == a.config['mesh'].get('discovery_url')
        assert applied['offline'] == a.config['mesh']['offline']
        assert not (b.root / 'run/zork-agent.pid').exists()
        assert admin(b, 'GET', '/v1/node/mesh')['origin'] == b.origin
        after = pids()
        assert after[0] == before[0] and after[2] == before[2]
        before = after
        print('PASS: enabled unpaired Station adopts invitation transport and preserves identity', flush=True)
        # Simulate a committed intake whose reply never reached the joining node.
        # The transport identity remains durable, while its local membership was not saved.
        lost = json.loads((b.root / 'config.json').read_text())
        lost['mesh']['group'] = None
        lost['mesh']['peers'] = []
        (b.root / 'config.json').write_text(json.dumps(lost))
        record_path = a.root / 'mesh/invites' / (invite['id'] + '.json')
        record = json.loads(record_path.read_text())
        record['expires_at'] = int(time.time()) - 1
        record_path.write_text(json.dumps(record))
        assert join(b, invite)['joined'], 'same-device receipt did not recover a lost acknowledgement'
        assert join(b, invite)['already_joined']
        assert len(admin(a, 'GET', '/v1/node/mesh')['config']['group']['members']) == 2
        status, rejected = request(c, 'POST', '/v1/node/mesh/join', {'invitation': invite['invitation']})
        assert status == 409 and 'already_used' in rejected['error'], (status, rejected)
        next_invite = admin(b, 'POST', '/v1/node/mesh/invites')
        assert next_invite['invitation'].startswith('zj1_')
        assert join(c, next_invite)['joined']
        f.wait(lambda: len(admin(b, 'GET', '/v1/node/mesh')['config']['group']['members']) == 3, 'third member reaches existing peer')
        assert pids() == before, 'enrollment restarted a Station or Agent'
        print('PASS: CLI reuses running Stations; one-use invitation, idempotent retry, third-device membership; no process restart', flush=True)

        # Rename a member through its own administrator API; authority propagates it.
        identities = [node.origin for node in nodes]
        renamed = admin(b, 'PUT', '/v1/node/name', {'name': '  工作室小熊  '})
        assert renamed['name'] == '工作室小熊'
        def member_name(node, origin):
            group = admin(node, 'GET', '/v1/node/mesh')['config']['group']
            return next(m['name'] for m in group['members'] if m['origin'] == origin)
        for node in nodes:
            f.wait(lambda node=node: member_name(node, b.origin) == '工作室小熊', 'renamed member reaches all devices')
        assert json.loads((b.root / 'config.json').read_text())['mesh']['name'] == '工作室小熊'
        assert admin(b, 'GET', '/v1/node/info')['name'] == '工作室小熊'
        for invalid in ['', '   ', 'x' * 65, 'bad\nname']:
            status, _ = request(b, 'PUT', '/v1/node/name', {'name': invalid})
            assert status >= 400, (invalid, status)
        assert b.request('PUT', '/v1/node/name', {'name': 'unauthorized'})[0] == 401
        admin(a, 'PUT', '/v1/node/name', {'name': '主设备'})
        for node in nodes:
            f.wait(lambda node=node: member_name(node, a.origin) == '主设备', 'authority rename reaches all devices')
        b.restart_station()
        f.wait(lambda: admin(b, 'GET', '/v1/node/info')['name'] == '工作室小熊', 'rename survives restart')
        assert [node.origin for node in nodes] == identities
        before = pids()
        print('PASS: Unicode device rename synchronizes through authority, rejects invalid/unauthorized writes, and survives restart', flush=True)

        # An invitation changes no membership rows. Its authority notification
        # must still reach settings opened through a different Station.
        f.wait(lambda: all(peer['online'] for peer in admin(b, 'GET', '/v1/mesh')['peers']), 'peer subscriptions online')
        time.sleep(.5)
        membership_revision = admin(b, 'GET', '/v1/node/mesh')['config']['group']['revision']
        token = admin(b, 'GET', '/v1/mesh')['change_token']
        phone_invite = admin(a, 'POST', '/v1/node/mesh/client-invites')
        f.wait(lambda: admin(b, 'GET', '/v1/mesh')['change_token'] != token, 'authority invitation push reaches peer')
        assert admin(b, 'GET', '/v1/node/mesh')['config']['group']['revision'] == membership_revision
        assert any(i['id'] == phone_invite['id'] for i in admin(b, 'GET', '/v1/node/mesh/invites')['items'])
        token = admin(b, 'GET', '/v1/mesh')['change_token']
        admin(a, 'DELETE', '/v1/node/mesh/invites/' + phone_invite['id'])
        f.wait(lambda: admin(b, 'GET', '/v1/mesh')['change_token'] != token, 'authority revocation push reaches peer')
        assert next(i for i in admin(b, 'GET', '/v1/node/mesh/invites')['items'] if i['id'] == phone_invite['id'])['status'] == 'revoked'
        print('PASS: read-only supervisor observation and invitation-only authority changes use push subscriptions', flush=True)

        selection = {'profile_id': 'fixture', 'model': 'fixture-model', 'thinking': 'off'}
        leader = admin(a, 'POST', '/v1/node/agents', dict(selection, id='leader', name='Leader', role='leader'))
        admin(a, 'POST', '/v1/node/agents/leader/open', {})
        admin(a, 'POST', '/v1/node/agents', dict(selection, id='local-worker', name='Local Worker', role='worker', allowed_leaders=[]))
        admin(b, 'POST', '/v1/node/agents', dict(selection, id='worker', name='Worker', role='worker', allowed_leaders=[]))
        remote = b.origin + '/worker'
        f.wait(lambda: any(w['id'] == remote for w in request(a, 'GET', '/v1/agent/workers', leader=leader)[1]['items']), 'Worker available without a separate grant')
        assert any(w['id'] == 'local-worker' for w in request(a, 'GET', '/v1/agent/workers', leader=leader)[1]['items'])
        goal = json.dumps({'fake_tools': [
            {'name': 'shell.run', 'input': {'command': "printf 'one\\n' >> enrollment-executions.txt"}},
            {'name': 'chat.post_message', 'input': {'kind': 'final', 'text': 'Executed on mini2'}},
        ]})
        body = {'request_id': 'enrollment-task', 'worker_id': remote, 'goal': goal}
        status, assigned = request(a, 'POST', '/v1/agent/tasks', body, leader)
        assert status == 200, (status, assigned)
        assert request(a, 'POST', '/v1/agent/tasks', body, leader)[1]['session_id'] == assigned['session_id']
        f.wait(lambda: any(t['result_text'] == 'Executed on mini2' for t in a.get('/v1/tasks')['items']), 'remote result', 90)
        task = b.get('/v1/tasks')['items'][0]
        assert (Path(task['workspace']) / 'enrollment-executions.txt').read_text().splitlines() == ['one']
        assert not (a.workspace / 'enrollment-executions.txt').exists()
        print('PASS: default personal-mesh Worker collaboration executes once on the chosen device', flush=True)

        expired = admin(a, 'POST', '/v1/node/mesh/invites')
        admin(a, 'DELETE', '/v1/node/mesh/invites/' + expired['id'])
        assert next(i for i in admin(a, 'GET', '/v1/node/mesh/invites')['items'] if i['id'] == expired['id'])['status'] == 'revoked'
        admin(a, 'POST', '/v1/node/mesh/members/remove', {'origin': b.origin})
        f.wait(lambda: not any(p['origin'] == b.origin for p in admin(c, 'GET', '/v1/node/mesh')['config']['peers']), 'removal reaches remaining devices')
        status, rejected = request(b, 'POST', '/v1/node/mesh/join', {'invitation': invite['invitation']})
        assert status == 409, (status, rejected)
        assert [w['id'] for w in request(a, 'GET', '/v1/agent/workers', leader=leader)[1]['items']] == ['local-worker']
        assert pids() == before, 'membership removal restarted tasks'
        print('PASS: invitation revocation and device removal converge; an old command cannot rejoin a removed device', flush=True)
        output = Path(os.environ.get('ZORK_TEST_ARTIFACT_DIR', str(f.ROOT / 'artifacts/mesh-enrollment')))
        output.mkdir(parents=True, exist_ok=True)
        (output / 'result.json').write_text(json.dumps({'root': str(root), 'checks': ['cli_join', 'single_use', 'retry', 'three_nodes', 'no_restart', 'default_worker_grant', 'single_execution', 'revoke', 'supervisor_observe', 'invitation_authority_push']}, indent=2))
    finally:
        for node in nodes:
            node.stop()


if __name__ == '__main__':
    main()
