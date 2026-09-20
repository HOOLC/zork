#!/usr/bin/env python3
"""Real supervisors, isolated identities/history, and fault recovery through a rescue Agent."""
import argparse
from contextlib import ExitStack
import http.server
import importlib.util
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/lib'))
import deployment
from deployment import atomic_json, copy_tree, digest, manifest
from deployment_build import build
from deployment_health import NodeRuntime, control


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


recovery = load('recovery', ROOT / 'scripts/dev/recovery.py')


def port(udp=False):
    with socket.socket(type=socket.SOCK_DGRAM if udp else socket.SOCK_STREAM) as reserve:
        reserve.bind(('127.0.0.1', 0))
        return reserve.getsockname()[1]


class Provider:
    """External HTTP provider; all Agent tools, chat transport and storage are real."""
    def __enter__(self):
        self.chat_id = ''
        self.rescue = None
        self.turns = {}
        model = self
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                body = self.rfile.read(int(self.headers['Content-Length'])).decode()
                challenges = re.findall(r'reply to chat ([A-Za-z0-9_-]+) with exactly: (zork-health-[0-9a-f]{32})', body)
                chat_id, nonce = challenges[-1] if challenges else (model.chat_id, '')
                turn = model.turns.get(nonce, 0)
                model.turns[nonce] = turn + 1
                calls = [{'tool': 'tool.help', 'arguments': {'tool': 'chat.post_message'}}]
                if model.rescue:
                    calls += [{'tool': 'tool.help', 'arguments': {'tool': 'shell.run'}},
                              {'tool': 'shell.run', 'arguments': {'command': model.rescue}}]
                calls += [{'tool': 'chat.post_message', 'arguments': {'chat_id': chat_id, 'text': nonce}}]
                message = {'role': 'assistant', 'content': 'Finished.'}
                finish = 'stop'
                if nonce and turn < len(calls):
                    call = dict(calls[turn], action='Isolated deployment acceptance')
                    message = {'role': 'assistant', 'content': None, 'tool_calls': [{
                        'id': 'recovery-' + str(turn), 'type': 'function',
                        'function': {'name': 'call', 'arguments': json.dumps(call)}}]}
                    finish = 'tool_calls'
                response = json.dumps({'id': 'recovery-provider', 'object': 'chat.completion',
                    'model': 'recovery-model', 'choices': [{'index': 0, 'message': message, 'finish_reason': finish}],
                    'usage': {'prompt_tokens': 1, 'completion_tokens': 1, 'total_tokens': 2}}).encode()
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(response)))
                self.end_headers()
                self.wfile.write(response)
            def log_message(self, *_):
                pass
        self.server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.worker = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.worker.start()
        return self

    def configure(self, data):
        (data / 'profiles').mkdir(parents=True)
        atomic_json(data / 'profiles/recovery-fixture.json', {
            'provider': 'openai', 'billing': 'usage', 'base_url': f'http://127.0.0.1:{self.server.server_port}/v1',
            'auth': {'type': 'api_key', 'key': 'isolated-recovery-fixture'},
            'models': [{'id': 'recovery-model', 'api': 'openai-completions', 'streaming': False,
                'thinking': ['off'], 'default_thinking': 'off', 'default': True,
                'capabilities': {'input': ['text']}, 'limits': {'context_window_tokens': 100000, 'max_output_tokens': 1000}}]})

    def __exit__(self, *_):
        self.server.shutdown()
        self.server.server_close()
        self.worker.join()


class Runtime(NodeRuntime):
    def request(self, path, body=None, agent=False):
        if body and 'content' in body and self.provider.rescue:
            body = dict(body, content=body['content'].replace(
                'Do not run shell commands, inspect files, or send to any other chat.',
                'First use shell.run to recover the isolated dev fixture with this exact command: ' + self.provider.rescue))
        result = super().request(path, body, agent)
        if path == '/v1/im/sessions' and body is not None:
            self.provider.chat_id = result['session_id']
        return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--candidate', type=Path, required=True, help='Fresh node or app candidate with captured Cargo binaries')
    parser.add_argument('--output', type=Path, default=ROOT / 'artifacts/release-dev-recovery')
    parser.add_argument('--live-profile', type=Path, help='Private profile copied only to isolated data, then deleted')
    parser.add_argument('--live-model')
    parser.add_argument('--live-thinking', default='off')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    candidate_record, payload = recovery.load_candidate(args.candidate)
    raw = payload if candidate_record['kind'] == 'node' else args.candidate.resolve() / 'bin'
    for name, expected in candidate_record['source']['binaries'].items():
        if digest(raw / name) != expected:
            raise RuntimeError('Drill binary differs from the captured candidate: ' + name)
    report = {'candidate': candidate_record['id'], 'source': candidate_record['source'], 'checks': [],
              'scope': 'isolated loopback Mesh; real supervisors, embedded Agents and tools; external deterministic HTTP model',
              'passed': False}
    root = Path(tempfile.mkdtemp(prefix='zrr-', dir='/tmp')).resolve()
    runtimes = {}
    def passed(name, evidence=None):
        report['checks'].append({'name': name, 'evidence': evidence})
        atomic_json(output / 'result.json', report)
        print('PASS ' + name, flush=True)
    try:
        with ExitStack() as stack:
            settings = {'repo': str(ROOT), 'channels': {}}
            for channel in ('release', 'dev'):
                data, binary = root / channel / 'data', root / channel / 'bin'
                data.mkdir(parents=True)
                binary.mkdir()
                for name in ('zork', 'zork-station', 'zork-agent', 'zork-gh'):
                    deployment.clone_file(raw / name, binary / name)
                provider = stack.enter_context(Provider())
                provider.configure(data)
                bindings = {name: f'127.0.0.1:{port()}' for name in ('station', 'runtime', 'control', 'agent')}
                atomic_json(data / 'config.json', {'bind': bindings, 'admin': {'token': os.urandom(24).hex()},
                    'mesh': {'enabled': True, 'offline': True, 'bind': f'127.0.0.1:{port(True)}', 'peers': [], 'workspaces': []}})
                health = {'profile': 'recovery-fixture', 'model': 'recovery-model', 'timeout': 30}
                settings['channels'][channel] = {'node': {'data': str(data), 'payload': str(binary), 'health': health}}
                runtime = Runtime(data, binary, output / (channel + '.log'), **health)
                runtime.provider = provider
                runtimes[channel] = runtime
                runtime.start(runtime.capture(), candidate=True)
            atomic_json(root / 'deployment-config.json', settings)
            baseline = {channel: runtime.health() for channel, runtime in runtimes.items()}
            self_release, dev = runtimes['release'], runtimes['dev']
            assert baseline['release']['origin'] != baseline['dev']['origin']
            config_before = json.loads((dev.data / 'config.json').read_text())
            chat_before = baseline['dev']['chat']
            last_epoch = dev.request('/v1/im/sessions/' + chat_before['chat_id'] + '/messages')['source_epoch']
            def preserved(rotated=False):
                nonlocal last_epoch
                current = dev.health(chat=False)
                assert current['origin'] == baseline['dev']['origin']
                assert json.loads((dev.data / 'config.json').read_text()) == config_before
                page = dev.request('/v1/im/sessions/' + chat_before['chat_id'] + '/messages')
                history = page['items']
                assert {chat_before['request_id'], chat_before['reply_id']}.issubset({m['id'] for m in history})
                assert (page['source_epoch'] != last_epoch) == rotated
                last_epoch = page['source_epoch']
                release_now = self_release.health(chat=False)
                assert release_now['station']['pid'] == baseline['release']['station']['pid']
                assert release_now['supervisor']['pid'] == baseline['release']['supervisor']['pid']
                return {'origin': current['origin'], 'retained_reply': chat_before['reply_id'],
                        'release_pid_unchanged': release_now['station']['pid'], 'chat_epoch_rotated': rotated}
            passed('release/dev run simultaneously with distinct identities and complete chat replies', baseline)

            bad_repo = root / 'invalid-build'
            bad_repo.mkdir()
            (bad_repo / 'Cargo.toml').write_text('[invalid toml\n')
            with (output / 'build-failure.log').open('w') as log:
                subprocess.run(['git', 'init', '-q', str(bad_repo)], check=True, stdout=log, stderr=log)
                subprocess.run(['git', 'add', 'Cargo.toml'], cwd=bad_repo, check=True, stdout=log, stderr=log)
                subprocess.run(['git', '-c', 'user.name=Recovery fixture', '-c', 'user.email=recovery@example.invalid',
                                'commit', '-qm', 'Invalid build fixture'], cwd=bad_repo, check=True, stdout=log, stderr=log)
            try:
                build(bad_repo, root / 'failed-builds')
                raise AssertionError('Invalid Cargo build succeeded')
            except RuntimeError as error:
                assert 'Cargo build failed' in str(error)
            for log in (root / 'failed-builds').glob('*/build.log'):
                shutil.copyfile(log, output / 'cargo-build-failure.log')
            assert control(dev.data)['pid'] == baseline['dev']['supervisor']['pid']
            passed('real Cargo build failure leaves both running versions unchanged', preserved())

            next_candidate = root / 'candidate'
            next_candidate.mkdir()
            copy_tree(dev.binaries, next_candidate / 'payload')
            next_record = manifest(next_candidate / 'payload', candidate_record['source'], 'dev', 'node')
            atomic_json(next_candidate / 'deployment.json', next_record)
            def fixture_runtime(settings, _, channel, kind):
                assert kind == 'node'
                return runtimes[channel]
            with patch.object(recovery, 'runtime_for', side_effect=fixture_runtime):
                real_copy = deployment.clone_file
                def failed_backup(source, target):
                    if 'snapshot-' in str(target):
                        raise OSError('injected full backup disk')
                    return real_copy(source, target)
                with patch.object(deployment, 'clone_file', side_effect=failed_backup):
                    try:
                        recovery.apply_candidate(root, 'dev', next_candidate)
                        raise AssertionError('Backup failure was ignored')
                    except OSError:
                        pass
                passed('backup failure aborts before configuration or binary mutation', preserved())

                def bad_migration(data):
                    value = json.loads((data / 'config.json').read_text())
                    value['bind']['runtime'] = 'injected-invalid-listen-address'
                    atomic_json(data / 'config.json', value)
                    (data / 'state/station.sqlite').rename(data / 'state/failed-migration.sqlite')
                    (data / 'fault-history.txt').write_text('history written by failed candidate')
                dev.timeout = 8
                with patch.object(recovery, 'migrate_config', side_effect=bad_migration):
                    try:
                        recovery.apply_candidate(root, 'dev', next_candidate)
                        raise AssertionError('Startup failure was ignored')
                    except RuntimeError as error:
                        assert 'acceptance' in str(error), str(error)
                dev.timeout = 30
                assert list((root / 'transactions').glob('*/failed-*/fault-history.txt'))
                passed('real startup failure restores full pre-migration storage and keeps failed-version history', preserved(rotated=True))

                original_replace = os.replace
                def fail_switch(source, destination):
                    if Path(destination) == dev.binaries and '-incoming-' in str(source):
                        raise OSError('injected cutover rename failure')
                    return original_replace(source, destination)
                with patch.object(os, 'replace', side_effect=fail_switch):
                    try:
                        recovery.apply_candidate(root, 'dev', next_candidate)
                        raise AssertionError('Switch failure was ignored')
                    except OSError:
                        pass
                passed('switch failure after removing the old binary directory restores the running version', preserved(rotated=True))

            # Exit the deployer abruptly after mutation. The rescue process remains up.
            script = f'''import importlib.util, os, sys
sys.path.insert(0, {str(ROOT / 'scripts/lib')!r})
from deployment_health import NodeRuntime
spec=importlib.util.spec_from_file_location('recovery', {str(ROOT / 'scripts/dev/recovery.py')!r})
m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
from pathlib import Path
NodeRuntime.start=lambda *a, **k: os._exit(86)
m.apply_candidate(Path({str(root)!r}), 'dev', Path({str(next_candidate)!r}))
'''
            crash = subprocess.run([sys.executable, '-c', script], capture_output=True, text=True)
            assert crash.returncode == 86, crash.stderr
            pending = json.loads((root / 'dev/node-pending.json').read_text())
            # A real shell tool in the independently running release Agent performs recovery.
            journal_path = Path(pending['transaction']) / 'journal.json'
            self_release.provider.rescue = shlex.join([sys.executable, str(ROOT / 'scripts/dev/recovery.py'),
                                                       '--root', str(root), 'recover', pending['transaction']])
            rescued = self_release.chat()
            self_release.provider.rescue = None
            assert json.loads(journal_path.read_text())['phase'] == 'rolled_back'
            passed('release Agent shell tool recovers dev after abrupt deployer exit',
                   {'release_reply': rescued, **preserved(rotated=True), 'dev_reply_after_recovery': dev.chat()})
            if args.live_profile:
                if not args.live_model:
                    raise RuntimeError('--live-model is required with --live-profile')
                shutil.copy2(args.live_profile, dev.data / 'profiles/recovery-live.json')
                live = NodeRuntime(dev.data, dev.binaries, output / 'dev.log',
                    profile='recovery-live', model=args.live_model, thinking=args.live_thinking, timeout=180)
                # Reload on restart so this also exercises provider configuration changes.
                before = dev.capture()
                dev.stop()
                dev.start(before)
                evidence = live.health()
                passed('real configured model replies after profile change and restart', evidence)
            report['passed'] = True
    finally:
        errors = []
        for runtime in runtimes.values():
            try:
                runtime.stop()
            except Exception as error:
                errors.append(str(error))
        report['cleanup_errors'] = errors
        if errors:
            report['retained_fixture'] = str(root)
            report['passed'] = False
        else:
            shutil.rmtree(root)
            report['isolated_data_removed'] = True
        atomic_json(output / 'result.json', report)
    if not report['passed']:
        raise SystemExit(1)
    print('PASS real release/dev recovery drill: ' + str(output / 'result.json'), flush=True)


if __name__ == '__main__':
    main()
