#!/usr/bin/env python3
"""Opt-in native-client Chat and real-model remote-tool round trips, including restart."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import sqlite3
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('directory_fixture', ROOT / 'scripts/test-mobile-mesh-directory.py')
d = importlib.util.module_from_spec(spec)
spec.loader.exec_module(d)


class LiveNode(d.f.Node):
    def start(self):
        self.log = (self.root / 'supervisor.log').open('ab')
        self.process = subprocess.Popen([str(d.f.TARGET / 'zork'), 'start', '--data', str(self.root)],
            stdout=self.log, stderr=self.log, start_new_session=True)


def events(node, execution):
    values = []
    for path in sorted((node.root / 'shared-files/sessions' / execution / 'segments').glob('*.jsonl')):
        for line in path.read_text().splitlines():
            try: values.append(json.loads(line)['event'])
            except json.JSONDecodeError: pass
    return values


def execution_id(node):
    with sqlite3.connect(node.root / 'state/station.sqlite') as db:
        row = db.execute("SELECT value FROM node_agents WHERE id='mesh-live'").fetchone()
        return json.loads(row[0]).get('session_id') if row else None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--profile', type=Path, required=True)
    parser.add_argument('--model', required=True)
    parser.add_argument('--thinking', default='high')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--timeout', type=int, default=240)
    args = parser.parse_args()
    profile = json.loads(args.profile.read_text())
    selection = next(model for model in profile['models'] if model['id'] == args.model)
    assert selection.get('limits'), 'The selected private profile must specify model limits'
    if profile.get('auth', {}).get('type') == 'oauth':
        assert profile['auth'].get('expires', 0) > time.time() * 1000 + (args.timeout * 2 + 300) * 1000
        profile['auth'].pop('refresh', None)
    root = Path(tempfile.mkdtemp(prefix='zlive-mesh-', dir='/tmp'))
    os.environ['ZORK_REGISTRY_DIR'] = str(root / 'registry')
    args.output.mkdir(parents=True, exist_ok=True)
    nodes, phone, phases = [], None, []
    report = {'fixture': str(root), 'model': args.model, 'thinking': args.thinking,
        'fake_agent': False, 'topology': 'native access client and two isolated Stations on one host', 'phases': phases}
    print(json.dumps({'fixture': str(root), 'model': args.model}), flush=True)
    try:
        for name in ('coordinator', 'mesh-live-target'):
            node = LiveNode(root / name); nodes.append(node)
            node.config['admin'] = {'token': d.TOKEN}
            node.config['mesh'].update(name=name, offline=False, bind=None,
                relay_urls=['http://127.0.0.1:9'], discovery_url='http://127.0.0.1:9/pkarr', quic_discovery_urls=[])
            for workspace in node.config['mesh']['workspaces']:
                workspace.update(profile_id='live', model=args.model, thinking=args.thinking)
            (node.root / 'profiles/fixture.json').unlink()
            private = node.root / 'profiles/live.json'
            private.write_text(json.dumps(profile)); private.chmod(0o600)
            (node.root / 'config.json').write_text(json.dumps(node.config))
            node.start(); d.ready(node)
        a, b = nodes
        d.join(b, a)
        d.admin(a, 'POST', '/v1/node/agents', {'id': 'mesh-live', 'name': 'Mesh live acceptance', 'role': 'leader',
            'profile_id': 'live', 'model': args.model, 'thinking': args.thinking})
        phone = d.Phone(root / 'client')
        invite, _ = d.phone_invite(phone, a)
        accepted = d.approve(phone, a, invite)
        identity = accepted['identity']
        phone.nodes([a.origin, b.origin])
        opened = phone.command('request', peer=a.origin, method='POST', path='/v1/node/agents/mesh-live/open', body={})
        chat = opened['chat_id']
        for number in (1, 2):
            proof = 'mesh-live-' + secrets.token_hex(12)
            (b.workspace / 'seed.txt').write_text(proof + '\n')
            result_file = b.workspace / f'result-{number}.txt'
            execution = execution_id(a)
            before = len(events(a, execution)) if execution else 0
            prompt = (f'这是已授权的隔离 Mesh 验收。请用 device.list 找到 mesh-live-target，'
                f'通过 shell.run 的 target 在那台设备读取 {b.workspace / "seed.txt"}，'
                f'把读到的原样内容写入同一远端设备的 {result_file}。'
                '不要在本地执行远端文件操作，不读取 profiles、凭据或任何测试目录以外的文件。'
                '完成后必须用 chat.post_message 向当前 Chat 发布文件里完整的随机校验文本。'
                '不要猜测文本，也不要只写内部最终回复。')
            queued = phone.command('enqueue', peer=a.origin, session=chat, content=prompt)
            phone.command('flush', peer=a.origin)
            execution = d.f.wait(lambda: execution_id(a), 'real Agent context')
            started = time.monotonic()
            def complete():
                current = events(a, execution)[before:]
                terminal = next((event for event in reversed(current) if event['kind'] == 'turn_finished'), None)
                if terminal and terminal['outcome'] != 'finished':
                    raise AssertionError('real model turn failed: ' + terminal['outcome'])
                if not terminal: return None
                messages = phone.read(a.origin, f'/v1/im/sessions/{chat}/messages')['items']
                if not any(item.get('role') == 'assistant' and proof in item.get('content', '') for item in messages):
                    raise AssertionError('model finished without delivering the verified result to the native client')
                return current
            current = d.f.wait(complete, 'real model remote tool and visible Chat reply', args.timeout)
            assert result_file.read_text() == proof + '\n'
            assert not (a.workspace / result_file.name).exists()
            tools = [event['result']['tool'] for event in current if event['kind'] == 'tool_result' and event['result']['outcome'] == 'succeeded']
            assert {'shell.run', 'chat.post_message'} <= set(tools), tools
            assert any(event['kind'] == 'step_completed' and event.get('usage') for event in current), 'no real provider usage'
            phases.append({'phase': 'after_restart' if number == 2 else 'first_round_trip', 'seconds': round(time.monotonic() - started, 2),
                'request_id': queued['request_id'], 'tools': tools, 'file_sha256': hashlib.sha256(result_file.read_bytes()).hexdigest()})
            print('PASS: real model remote shell result reaches the native client' + (' after both Stations restart' if number == 2 else ''), flush=True)
            if number == 1:
                a.restart_station(); b.restart_station(); d.ready(a); d.ready(b)
                phone.command('pause'); assert phone.command('resume')['identity'] == identity
        report['passed'] = True
    finally:
        if phone: phone.close()
        for node in nodes:
            node.stop()
            (node.root / 'profiles/live.json').unlink(missing_ok=True)
        (args.output / 'result.json').write_text(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
