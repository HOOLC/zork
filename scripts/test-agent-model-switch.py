#!/usr/bin/env python3
"""Switch a Leader away from a removed model using isolated real processes."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
from urllib.error import HTTPError
from urllib.request import Request, urlopen

spec = importlib.util.spec_from_file_location('fixture', Path(__file__).with_name('test-mesh.py'))
f = importlib.util.module_from_spec(spec)
spec.loader.exec_module(f)


def main():
    with tempfile.TemporaryDirectory(prefix='zork-model-switch-') as directory:
        root = Path(directory)
        (root / 'profiles').mkdir()
        old = {'id': 'old-model', 'api': 'openai-completions', 'streaming': False,
               'thinking': ['off'], 'default_thinking': 'off',
               'capabilities': {'input': ['text']},
               'limits': {'context_window_tokens': 100000, 'max_output_tokens': 10000},
               'default': True}
        new = dict(old, id='new-model')
        (root / 'profiles/fixture.json').write_text(json.dumps({
            'provider': 'openai', 'billing': 'usage', 'base_url': 'http://127.0.0.1:9/v1',
            'auth': {'type': 'api_key', 'key': 'sk-fixture'}, 'models': [old],
        }))
        bindings = {name: f'127.0.0.1:{f.port()}'
                    for name in ('station', 'runtime', 'control', 'agent')}
        (root / 'config.json').write_text(json.dumps({
            'bind': bindings, 'admin': {'token': 'isolated-test'},
        }))
        process = None

        def request(method, path, body=None, expected=200, agent=False, headers=None):
            base = 'http://' + bindings['agent' if agent else 'runtime']
            req = Request(base + path, method=method,
                          data=None if body is None else json.dumps(body).encode(),
                          headers={'Content-Type': 'application/json',
                                   'Authorization': 'Bearer isolated-test', **(headers or {})})
            try:
                response = urlopen(req, timeout=15)
            except HTTPError as error:
                response = error
            with response:
                raw = response.read()
                value = json.loads(raw) if raw else None
                assert response.status == expected, (response.status, value)
                return value

        def start(log):
            nonlocal process
            process = subprocess.Popen(
                [str(f.TARGET / 'zork'), 'start', '--data', str(root), '--fake-agent'],
                stdout=log, stderr=log, start_new_session=True)
            f.wait(lambda: request('GET', '/readyz'), 'Station ready')

        def stop():
            nonlocal process
            if process:
                process.terminate()
                try:
                    process.wait(timeout=12)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
                process = None

        with (root / 'process.log').open('ab') as log:
            try:
                start(log)
                selection = {'profile_id': 'fixture', 'model': 'old-model', 'thinking': 'off'}
                leader = request('POST', '/v1/node/agents',
                                 dict(selection, id='leader', name='Leader', role='leader'))
                opened = request('POST', '/v1/node/agents/leader/open', {})
                session_id = opened['session_id']
                request('POST', f'/v1/im/sessions/{session_id}/messages',
                        {'content': json.dumps({'fake_tools': [{
                            'name': 'chat.post_message',
                            'input': {'text': 'Preserve this conversation'},
                        }]})}, 202)
                def messages():
                    return request('GET', f'/v1/im/sessions/{session_id}/messages')['items']
                f.wait(lambda: len(messages()) >= 2, 'fake model reply')
                before = messages()
                request('POST', '/v1/node/agents', dict(
                    selection, id='worker', name='Worker', role='worker', allowed_leaders=['leader']))
                task = request('POST', '/v1/agent/tasks', {
                    'request_id': 'model-switch-task', 'worker_id': 'worker', 'goal': 'Wait for input',
                }, headers={'x-zork-session-key': leader['session_key']})
                worker_session = task['session_id']
                f.wait(lambda: request('GET', f'/sessions/{worker_session}', agent=True)['status']
                       == 'finished', 'Worker completed first turn on old model')
                request('PUT', '/v1/node/profiles/fixture/models', {'models': [new]})
                request('PATCH', '/v1/node/agents/leader/model', selection, 400)
                replacement = dict(selection, model='new-model')
                updated = request('PATCH', '/v1/node/agents/leader/model', replacement)
                assert updated['model'] == 'new-model', updated
                assert updated['session_id'] == leader['session_id'] == session_id
                assert messages() == before, 'switch changed conversation history'
                request('PATCH', '/v1/node/agents/worker/model', replacement)
                request('POST', f'/v1/im/sessions/{worker_session}/messages',
                        {'content': 'Use the updated model'}, 202)
                f.wait(lambda: request('GET', f'/sessions/{worker_session}', agent=True)['model']
                       == 'new-model', 'existing Worker uses updated model')
                stop()
                start(log)
                reopened = request('POST', '/v1/node/agents/leader/open', {})
                assert reopened['session_id'] == session_id, reopened
                runtime = request('GET', f'/sessions/{session_id}', agent=True)
                assert runtime['model'] == 'new-model', runtime
                assert request('GET', f'/sessions/{worker_session}', agent=True)['model'] == 'new-model'
                assert messages() == before, 'restart changed conversation history'
                print('PASS: removed model rejected; Leader and existing Worker switch; session/history survive restart')
            except Exception:
                print((root / 'process.log').read_text(errors='replace')[-12000:])
                raise
            finally:
                stop()


if __name__ == '__main__':
    main()
