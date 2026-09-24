#!/usr/bin/env python3
"""Agent Mesh invitation tools against real Stations: mint, join, list, revoke."""
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('channels', ROOT / 'scripts/test-chat-channels.py')
channels = importlib.util.module_from_spec(spec)
spec.loader.exec_module(channels)
f = channels.fixture


def tool(node, session, name, args, invocation):
    return channels.request(node, 'POST', '/v1/mesh/invite-tools',
        {'session_id': session, 'invocation_id': invocation, 'tool': name, 'arguments': args})


def main():
    with tempfile.TemporaryDirectory(prefix='zork-invite-tools-') as directory:
        root = Path(directory)
        os.environ['ZORK_REGISTRY_DIR'] = str(root / 'registry')
        a, b = f.Node(root / 'office'), f.Node(root / 'laptop')
        try:
            for node in (a, b):
                node.config['admin'] = {'token': channels.TOKEN}
                node.config['mesh']['name'] = node.root.name
                (node.root / 'config.json').write_text(json.dumps(node.config))
                channels.start(node)
            session, _ = channels.make_caller(a, 'leader')

            status, invite = tool(a, session, 'mesh.invite', {'label': 'New laptop'}, 'invite-1')
            assert status == 200, (status, invite)
            assert re.fullmatch(r'[0-9a-f]{32}', invite['id']), invite
            assert invite['command'].startswith("zork mesh join '") and invite['single_use'] is True
            assert 0 < invite['expires_in_seconds'] <= 900 and 'mesh.revoke' in invite['security']
            ticket = invite['command'].split("'")[1]
            ledger = (a.root / 'mesh/agent-invites.json').read_text()
            assert ticket not in ledger and 'command' not in ledger
            status, replay = tool(a, session, 'mesh.invite', {'label': 'New laptop'}, 'invite-1')
            assert status == 200 and replay['id'] == invite['id'] and replay['command_shown'] is False
            assert ticket not in json.dumps(replay)
            print('PASS: mesh.invite returns a one-time command once; replay and ledger carry no secret', flush=True)

            status, listed = tool(a, session, 'mesh.invites', {}, 'list-1')
            assert status == 200, listed
            item = next(i for i in listed['items'] if i['id'] == invite['id'])
            assert item['state'] == 'created' and item['requested_in_this_chat'] and item['label'] == 'New laptop'
            assert ticket not in json.dumps(listed) and 'zork mesh join' not in json.dumps(listed)

            joined = subprocess.run([str(f.TARGET / 'zork'), 'mesh', 'join', ticket, '--data', str(b.root), '--json'],
                capture_output=True, text=True, timeout=65)
            assert joined.returncode == 0, joined.stderr
            def used():
                items = tool(a, session, 'mesh.invites', {}, 'list-2')[1]['items']
                return next((i for i in items if i['id'] == invite['id'] and i['state'] == 'used'), None)
            item = f.wait(used, 'invitation reported as used')
            assert item['device'] == {'id': b.origin, 'name': 'laptop'}, item
            status, rejected = tool(a, session, 'mesh.revoke', {'id': invite['id']}, 'revoke-used')
            assert status == 400 and rejected['error'] == 'invite_already_used_remove_device_instead', rejected
            print('PASS: joined device appears on the used invitation; used invitations cannot be revoked', flush=True)

            status, second = tool(a, session, 'mesh.invite', {}, 'invite-2')
            assert status == 200, second
            status, revoked = tool(a, session, 'mesh.revoke', {'id': second['id']}, 'revoke-2')
            assert status == 200 and revoked == {'id': second['id'], 'state': 'revoked'}, revoked
            item = next(i for i in tool(a, session, 'mesh.invites', {}, 'list-3')[1]['items'] if i['id'] == second['id'])
            assert item['state'] == 'revoked'
            refused = subprocess.run([str(f.TARGET / 'zork'), 'mesh', 'join', second['command'].split("'")[1],
                '--data', str(b.root), '--json'], capture_output=True, text=True, timeout=65)
            assert refused.returncode != 0 or json.loads(refused.stdout).get('already_joined'), refused
            status, bad = tool(a, session, 'mesh.revoke', {'id': 'nope'}, 'revoke-bad')
            assert status == 400 and bad['error'] == 'invalid_invite_id'
            status, bad = tool(a, session, 'mesh.invite', {'command': 'x'}, 'invite-bad')
            assert status == 400 and bad['error'] == 'invalid_arguments'
            status, unknown = tool(a, 'no-such-session', 'mesh.invite', {}, 'invite-unknown')
            assert status == 400 and unknown['error'] == 'unknown_session'
            print('PASS: revoke transitions created -> revoked; invalid arguments and unknown Sessions are refused', flush=True)

            cli = subprocess.run([str(f.TARGET / 'zork'), 'mesh', 'invite', '--data', str(a.root), '--json'],
                capture_output=True, text=True, timeout=65)
            assert cli.returncode == 0, cli.stderr
            value = json.loads(cli.stdout)
            assert set(value) >= {'id', 'command', 'expires_at', 'expires_in_seconds', 'single_use'} and 'invitation' not in value
            text = subprocess.run([str(f.TARGET / 'zork'), 'mesh', 'invite', '--data', str(a.root)],
                capture_output=True, text=True, timeout=65)
            assert text.returncode == 0 and 'Run this command on the device to add' in text.stdout
            print('PASS: zork mesh invite --json is structured; text output unchanged', flush=True)
        finally:
            for node in (a, b):
                node.stop()


if __name__ == '__main__':
    main()
