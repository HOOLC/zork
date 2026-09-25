#!/usr/bin/env python3
"""Real Stations, durable enrollment, membership and remote Chat tools."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import signal
import socket
import tempfile
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen

spec = importlib.util.spec_from_file_location('fixture', Path(__file__).with_name('test-mesh.py'))
f = importlib.util.module_from_spec(spec)
spec.loader.exec_module(f)
channel_spec = importlib.util.spec_from_file_location('channels', Path(__file__).with_name('test-chat-channels.py'))
channels = importlib.util.module_from_spec(channel_spec)
channel_spec.loader.exec_module(channels)
channels.TOKEN = 'enrollment-fixture'


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


def join_result(node, invitation):
    progress = admin(node, 'POST', '/v1/node/mesh/join', {'invitation': invitation['invitation']})
    def finished():
        value = admin(node, 'GET', '/v1/node/mesh/join/' + progress['id'])
        return value if value['finished'] else None
    return f.wait(finished, 'durable join operation', 65)


def main():
    root = Path(tempfile.mkdtemp(prefix='zenroll-', dir='/tmp'))
    os.environ['ZORK_REGISTRY_DIR'] = str(root / 'registry')
    nodes = [f.Node(root / name) for name in ('mini1', 'mini2', 'mini3', 'switch-target')]
    a, b, c, d = nodes
    print(f'isolated mesh enrollment: {root}', flush=True)
    for node in nodes:
        node.config['admin'] = {'token': 'enrollment-fixture'}
        node.config['mesh']['name'] = node.root.name
        if os.environ.get('ZORK_TEST_RELAY'):
            node.config['mesh'].update(offline=False,
                relay_urls=[os.environ['ZORK_TEST_RELAY']],
                discovery_url=os.environ.get('ZORK_TEST_DISCOVERY'))
        (node.root / 'config.json').write_text(json.dumps(node.config))
    # Joining preserves operator network choices, including an unreachable relay.
    # Native LAN/local discovery must not require replacing those settings.
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
        assert invite['command'].startswith("zork mesh join '") and invite['command'].endswith('--channel release')
        assert str(a.root) not in invite['command'] and 'curl' not in invite['command']
        # Unused invitations survive a Station restart: persisted privately, secret only as a hash.
        pending_record = a.root / 'mesh/invites' / (invite['id'] + '.json')
        assert pending_record.stat().st_mode & 0o777 == 0o600, oct(pending_record.stat().st_mode)
        stored = pending_record.read_text()
        assert 'secret_hash' in stored and invite['invitation'] not in stored and invite['command'] not in stored
        # Pause the invitation owner without losing its one-use capability.
        # Restarting the joining Station must resume the same persisted operation.
        authority_pid = int((a.root / 'run/zork-station.pid').read_text())
        os.kill(authority_pid, signal.SIGSTOP)
        try:
            pending = admin(b, 'POST', '/v1/node/mesh/join', {'invitation': invite['invitation']})
            assert not pending['finished']
            b.restart_station()
            f.wait(lambda: admin(b, 'GET', '/v1/node/mesh').get('origin') == b.origin, 'join owner restarted')
            resumed = admin(b, 'GET', '/v1/node/mesh/join/' + pending['id'])
            assert resumed['id'] == pending['id'] and not resumed['finished']
        finally:
            os.kill(authority_pid, signal.SIGCONT)
        assert join(b, invite)['joined']
        applied = json.loads((b.root / 'config.json').read_text())['mesh']
        assert applied.get('relay_urls') == b.config['mesh'].get('relay_urls')
        assert applied.get('discovery_url') == b.config['mesh'].get('discovery_url')
        assert applied['offline'] == b.config['mesh']['offline']
        assert not (b.root / 'run/zork-agent.pid').exists()
        assert admin(b, 'GET', '/v1/node/mesh')['origin'] == b.origin
        after = pids()
        assert after[0] == before[0] and after[2] == before[2]
        before = after
        print('PASS: in-flight join resumes the same operation after Station process death', flush=True)
        survivor = admin(a, 'POST', '/v1/node/mesh/invites')
        a.restart_station()
        f.wait(lambda: admin(a, 'GET', '/v1/node/mesh').get('origin') == a.origin, 'authority restarted')
        listed = next(i for i in admin(a, 'GET', '/v1/node/mesh/invites')['items'] if i['id'] == survivor['id'])
        assert listed['status'] == 'waiting', listed
        admin(a, 'DELETE', '/v1/node/mesh/invites/' + survivor['id'])
        before = pids()
        print('PASS: an unused invitation survives a restart of the inviting Station', flush=True)
        print('PASS: enabled unpaired Station preserves network settings and identity', flush=True)
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
        rejected = join_result(c, invite)
        assert 'already_used' in rejected['error'], rejected
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
        for node in (a, b, c):
            f.wait(lambda node=node: member_name(node, b.origin) == '工作室小熊', 'renamed member reaches all devices')
        assert json.loads((b.root / 'config.json').read_text())['mesh']['name'] == '工作室小熊'
        assert admin(b, 'GET', '/v1/node/info')['name'] == '工作室小熊'
        for invalid in ['', '   ', 'x' * 65, 'bad\nname']:
            status, _ = request(b, 'PUT', '/v1/node/name', {'name': invalid})
            assert status >= 400, (invalid, status)
        assert b.request('PUT', '/v1/node/name', {'name': 'unauthorized'})[0] == 401
        admin(a, 'PUT', '/v1/node/name', {'name': '主设备'})
        for node in (a, b, c):
            f.wait(lambda node=node: member_name(node, a.origin) == '主设备', 'authority rename reaches all devices')
        b.restart_station()
        f.wait(lambda: admin(b, 'GET', '/v1/node/info')['name'] == '工作室小熊', 'rename survives restart')
        assert [node.origin for node in nodes] == identities
        before = pids()
        print('PASS: Unicode device rename synchronizes through authority, rejects invalid/unauthorized writes, and survives restart', flush=True)

        # Mesh display names are separate from machine names: the authority
        # backfills A, B, C in join order, any member renames any device, every
        # Station converges, and duplicates are rejected by the authority.
        def display(node, origin):
            names = admin(node, 'GET', '/v1/node/mesh').get('names') or {}
            return next((d['name'] for d in names.get('devices', []) if d['origin'] == origin), None)
        for node in (a, b, c):
            f.wait(lambda node=node: [display(node, n.origin) for n in (a, b, c)] == ['A', 'B', 'C'],
                'default display letters reach every Station')
        renamed = admin(c, 'PUT', '/v1/node/mesh/names', {'origin': b.origin, 'name': ' 工作室 '})
        assert renamed['name'] == '工作室' and renamed['names']['authority'] == a.origin, renamed
        for node in (a, b, c):
            f.wait(lambda node=node: display(node, b.origin) == '工作室', 'display rename reaches every Station')
        assert admin(b, 'GET', '/v1/mesh')['names']['devices'][1]['name'] == '工作室'
        assert member_name(a, b.origin) == '工作室小熊', 'the machine name is kept'
        status, value = request(a, 'PUT', '/v1/node/mesh/names', {'origin': c.origin, 'name': '工作室'})
        assert status == 409 and '已被 Mesh 中的另一台设备使用' in value['error'], (status, value)
        for invalid in ['', '  ', 'x' * 25]:
            status, _ = request(b, 'PUT', '/v1/node/mesh/names', {'name': invalid})
            assert status >= 400, (invalid, status)
        assert b.request('PUT', '/v1/node/mesh/names', {'name': 'unauthorized'})[0] == 401
        # A Station renames itself when no origin is given.
        admin(b, 'PUT', '/v1/node/mesh/names', {'name': 'Studio'})
        for node in (a, b, c):
            f.wait(lambda node=node: display(node, b.origin) == 'Studio', 'self rename reaches every Station')
        c.restart_station()
        f.wait(lambda: display(c, b.origin) == 'Studio', 'display names survive restart')
        before = pids()
        print('PASS: Mesh display names backfill in join order, rename from any member, converge on every Station, reject duplicates and keep machine names', flush=True)

        # An invitation changes no membership rows. Its authority notification
        # must still reach settings opened through a different Station.
        f.wait(lambda: all(peer['online'] for peer in admin(b, 'GET', '/v1/mesh')['peers']), 'peer subscriptions online')
        time.sleep(.5)
        membership_revision = admin(b, 'GET', '/v1/node/mesh')['config']['group']['revision']
        token = admin(b, 'GET', '/v1/mesh')['change_token']
        extra_invite = admin(a, 'POST', '/v1/node/mesh/invites')
        f.wait(lambda: admin(b, 'GET', '/v1/mesh')['change_token'] != token, 'authority invitation push reaches peer')
        assert admin(b, 'GET', '/v1/node/mesh')['config']['group']['revision'] == membership_revision
        assert any(i['id'] == extra_invite['id'] for i in admin(b, 'GET', '/v1/node/mesh/invites')['items'])
        token = admin(b, 'GET', '/v1/mesh')['change_token']
        admin(a, 'DELETE', '/v1/node/mesh/invites/' + extra_invite['id'])
        f.wait(lambda: admin(b, 'GET', '/v1/mesh')['change_token'] != token, 'authority revocation push reaches peer')
        assert next(i for i in admin(b, 'GET', '/v1/node/mesh/invites')['items'] if i['id'] == extra_invite['id'])['status'] == 'revoked'
        print('PASS: read-only supervisor observation and invitation-only authority changes use push subscriptions', flush=True)

        caller, _ = channels.make_caller(a, 'leader')
        selection = {'profile_id': 'fixture', 'model': 'fixture-model', 'thinking': 'off'}
        admin(a, 'POST', '/v1/node/agents', dict(selection, id='local-worker', name='Local Worker', role='worker', allowed_leaders=[]))
        admin(b, 'POST', '/v1/node/agents', dict(selection, id='worker', name='Worker', role='worker', allowed_leaders=[]))
        remote = b.origin + '/worker'
        assigned = channels.operation(a, caller, 'agent.assign', {'worker_id': remote, 'goal': 'Wait for the next Chat instruction'}, 'enrollment-task')
        chat = assigned['chat']['chat_id']
        f.wait(lambda: channels.sql(b, 'SELECT runtime_id FROM mesh_runtime_sessions WHERE assignment_id=?', ('worker-' + chat,)), 'remote execution context')
        goal = json.dumps({'fake_tools': [
            {'name': 'shell.run', 'input': {'command': 'echo one >> enrollment-executions.txt'}},
            {'name': 'chat.post_message', 'input': {'target': a.origin, 'chat_id': chat, 'text': 'Executed on mini2'}},
        ]})
        channels.operation(a, caller, 'chat.post_message', {'chat_id': chat, 'text': goal})
        f.wait(lambda: any(m['text'] == 'Executed on mini2' for m in channels.operation(a, caller, 'chat.history', {'chat_id': chat})['items']), 'remote Chat result', 90)
        paths = list(b.root.rglob('enrollment-executions.txt'))
        assert len(paths) == 1 and paths[0].read_text().splitlines() == ['one']
        assert not list(a.root.rglob('enrollment-executions.txt'))
        print('PASS: personal-mesh Worker executes a real shell tool once on the chosen device and replies to its Chat', flush=True)

        previous_group = admin(b, 'GET', '/v1/node/mesh')['config']['group']
        bad_switch = admin(d, 'POST', '/v1/node/mesh/invites')
        admin(d, 'DELETE', '/v1/node/mesh/invites/' + bad_switch['id'])
        failed = subprocess.run([str(f.TARGET / 'zork'), 'mesh', 'switch', bad_switch['invitation'],
            '--data', str(b.root), '--yes', '--json'], capture_output=True, text=True, timeout=65)
        assert failed.returncode != 0
        assert admin(b, 'GET', '/v1/node/mesh')['config']['group'] == previous_group
        assert paths[0].read_text().splitlines() == ['one']
        print('PASS: failed confirmed switch preserves the current group and executed work', flush=True)

        expired = admin(a, 'POST', '/v1/node/mesh/invites')
        admin(a, 'DELETE', '/v1/node/mesh/invites/' + expired['id'])
        assert next(i for i in admin(a, 'GET', '/v1/node/mesh/invites')['items'] if i['id'] == expired['id'])['status'] == 'revoked'
        admin(a, 'POST', '/v1/node/mesh/members/remove', {'origin': b.origin})
        f.wait(lambda: not any(p['origin'] == b.origin for p in admin(c, 'GET', '/v1/node/mesh')['config']['peers']), 'removal reaches remaining devices')
        rejected = join_result(b, invite)
        assert any(reason in rejected['error'] for reason in ('removed', 'revoked')), rejected
        assert pids() == before, 'membership removal restarted tasks'
        print('PASS: invitation revocation and device removal converge; an old command cannot rejoin a removed device', flush=True)
        switch = admin(d, 'POST', '/v1/node/mesh/invites')
        declined = subprocess.run([str(f.TARGET / 'zork'), 'mesh', 'switch', switch['invitation'],
            '--data', str(b.root), '--json'], capture_output=True, text=True, timeout=65)
        assert declined.returncode != 0 and '--yes' in declined.stderr
        switched = subprocess.run([str(f.TARGET / 'zork'), 'mesh', 'switch', switch['invitation'],
            '--data', str(b.root), '--yes', '--json'], capture_output=True, text=True, timeout=65)
        assert switched.returncode == 0, switched.stderr
        assert json.loads(switched.stdout)['group']['authority'] == d.origin
        left = subprocess.run([str(f.TARGET / 'zork'), 'mesh', 'leave', '--data', str(b.root), '--yes', '--json'],
            capture_output=True, text=True, timeout=65)
        assert left.returncode == 0, left.stderr
        current = admin(b, 'GET', '/v1/node/mesh')
        assert current['origin'] == b.origin and current['config']['group'] is None
        assert (b.root / 'profiles/fixture.json').exists() and paths[0].read_text().splitlines() == ['one']
        assert len(admin(a, 'GET', '/v1/node/mesh')['config']['group']['members']) == 2
        print('PASS: switch requires explicit confirmation; switch and local leave preserve identity, profiles, files and other members', flush=True)
        assert json.loads(left.stdout)['manager_notified'] is True, left.stdout
        f.wait(lambda: all(m['origin'] != b.origin for m in admin(d, 'GET', '/v1/node/mesh')['config']['group']['members']),
               'manager dropped the departed member')
        print('PASS: leaving tells the managing device, which stops listing the member', flush=True)

        # A join stuck on an unreachable inviter can be replaced (explicitly) or cancelled.
        authority_pid = int((a.root / 'run/zork-station.pid').read_text())
        stuck_invite = admin(a, 'POST', '/v1/node/mesh/invites')
        other_invite = admin(d, 'POST', '/v1/node/mesh/invites')
        os.kill(authority_pid, signal.SIGSTOP)
        try:
            stuck = admin(b, 'POST', '/v1/node/mesh/join', {'invitation': stuck_invite['invitation']})
            assert not stuck['finished']
            status, refused = request(b, 'POST', '/v1/node/mesh/join', {'invitation': other_invite['invitation']})
            assert status == 409 and refused['error'] == 'another_join_is_in_progress', (status, refused)
            cli = subprocess.run([str(f.TARGET / 'zork'), 'mesh', 'join', other_invite['invitation'], '--data', str(b.root)],
                capture_output=True, text=True, timeout=65)
            assert cli.returncode != 0 and '--replace' in cli.stderr, cli.stderr
            replaced = admin(b, 'POST', '/v1/node/mesh/join', {'invitation': other_invite['invitation'], 'replace': True})
            assert replaced['id'] != stuck['id']
            def done():
                value = admin(b, 'GET', '/v1/node/mesh/join/' + replaced['id'])
                return value if value['finished'] else None
            assert f.wait(done, 'replacement join', 65)['phase'] == 'joined'
            left = subprocess.run([str(f.TARGET / 'zork'), 'mesh', 'leave', '--data', str(b.root), '--yes', '--json'],
                capture_output=True, text=True, timeout=65)
            assert left.returncode == 0, left.stderr
            cancel_invite = stuck_invite
            pending = admin(b, 'POST', '/v1/node/mesh/join', {'invitation': cancel_invite['invitation']})
            assert not pending['finished']
            cancelled = subprocess.run([str(f.TARGET / 'zork'), 'mesh', 'join', '--cancel', '--data', str(b.root), '--json'],
                capture_output=True, text=True, timeout=65)
            assert cancelled.returncode == 0, cancelled.stderr
            final = admin(b, 'GET', '/v1/node/mesh/join/' + pending['id'])
            assert final['finished'] and final['phase'] == 'cancelled' and final['error'] == 'join_cancelled', final
            assert admin(b, 'GET', '/v1/node/mesh')['config']['group'] is None
        finally:
            os.kill(authority_pid, signal.SIGCONT)
        print('PASS: a stuck join is replaced only on explicit confirmation, and can be cancelled', flush=True)

        output = Path(os.environ.get('ZORK_TEST_ARTIFACT_DIR', str(f.ROOT / 'artifacts/mesh-enrollment')))
        output.mkdir(parents=True, exist_ok=True)
        (output / 'result.json').write_text(json.dumps({'root': str(root), 'checks': ['cli_join', 'single_use', 'retry', 'three_nodes', 'no_restart', 'default_worker_grant', 'single_execution', 'revoke', 'supervisor_observe', 'invitation_authority_push', 'invite_survives_restart', 'leave_notifies_manager', 'join_replace_and_cancel']}, indent=2))
    finally:
        for node in nodes:
            node.stop()


if __name__ == '__main__':
    main()
