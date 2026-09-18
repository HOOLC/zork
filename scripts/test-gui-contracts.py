#!/usr/bin/env python3
"""Exercise GUI APIs against an isolated real Agent/Station with fake inference."""
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.request import Request, urlopen
from urllib.error import HTTPError

spec = importlib.util.spec_from_file_location('fixture', Path(__file__).with_name('test-mesh.py'))
f = importlib.util.module_from_spec(spec)
spec.loader.exec_module(f)
f.TARGET = Path(os.environ.get('CARGO_TARGET_DIR', f.ROOT / 'target')) / 'debug'


def main():
    root = Path(tempfile.mkdtemp(prefix='zork-gui-contracts-', dir='/tmp'))
    node = f.Node(root / 'node')
    node.config['mesh']['enabled'] = False
    node.config['admin'] = {'token': 'gui-fixture'}
    (node.root / 'config.json').write_text(json.dumps(node.config))
    requests = []

    class Models(BaseHTTPRequestHandler):
        def do_GET(self):
            requests.append((self.path, self.headers.get('Authorization')))
            body = json.dumps({'data': [{'id': 'provider-model', 'max_input_tokens': 32000, 'max_tokens': 4096, 'private': 'must-not-forward'}]}).encode()
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        def log_message(self, *_):
            pass

    provider = ThreadingHTTPServer(('127.0.0.1', 0), Models)
    threading.Thread(target=provider.serve_forever, daemon=True).start()

    def request(method, path, body=None, token=True, extra=None):
        headers = {'Content-Type': 'application/json', **(extra or {})}
        if token:
            headers['Authorization'] = 'Bearer gui-fixture'
        req = Request(node.url + path, method=method, headers=headers,
                      data=None if body is None else json.dumps(body).encode())
        try:
            with urlopen(req, timeout=25) as result:
                return result.status, json.load(result)
        except HTTPError as error:
            return error.code, json.load(error)

    def ok(method, path, body=None, extra=None):
        status, value = request(method, path, body, extra=extra)
        assert 200 <= status < 300, (method, path, status, value)
        return value

    try:
        node.start()
        f.wait(lambda: node.request('GET', '/readyz')[0] == 200, 'Station ready')
        f.wait(lambda: urlopen(node.agent_url + '/readyz', timeout=2).status == 200, 'Agent ready')
        for path in ['/v1/node/info', '/v1/node/conversations/read-markers', '/v1/node/profiles/fixture', '/v1/node/profiles/fixture/discovered-models']:
            assert request('GET', path, token=False)[0] == 401
        assert request('PUT', '/v1/node/profiles/fixture/models', {'models': []}, token=False)[0] == 401
        assert request('POST', '/v1/node/profiles/fixture/refresh', token=False)[0] == 401
        assert request('POST', '/v1/node/profiles/missing/refresh')[0] == 404
        refreshed = ok('POST', '/v1/node/profiles/fixture/refresh')
        assert refreshed['profile_id'] == 'fixture' and 'rateLimits' in refreshed
        assert 'sk-test' not in json.dumps(refreshed) and 'auth' not in refreshed
        info = ok('GET', '/v1/node/info')
        assert info['station']['running'] and info['update']['supported'] is False
        assert request('PUT', '/v1/node/profiles/fixture/name', {'name': 'renamed'}, token=False)[0] == 401
        assert request('PUT', '/v1/node/profiles/missing/name', {'name': 'renamed'})[0] == 404
        before_name = json.loads((node.root / 'profiles/fixture.json').read_text())
        public_before_name = ok('GET', '/v1/node/profiles/fixture')
        named = ok('PUT', '/v1/node/profiles/fixture/name', {'name': '主力连接'})
        assert named['profile_id'] == 'fixture' and named['name'] == '主力连接'
        after_name = json.loads((node.root / 'profiles/fixture.json').read_text())
        assert after_name['auth'] == before_name['auth']
        assert named['models'] == public_before_name['models']
        assert after_name['provider'] == before_name['provider']
        assert 'auth' not in named and 'secret' not in json.dumps(named)
        assert request('PUT', '/v1/node/profiles/fixture/name', {'name': ' '})[0] >= 400
        assert json.loads((node.root / 'profiles/fixture.json').read_text()) == after_name
        original = json.loads((node.root / 'profiles/fixture.json').read_text())
        public = ok('GET', '/v1/node/profiles/fixture')
        assert 'sk-test' not in json.dumps(public) and 'auth' not in public
        models = public['models']
        second = dict(models[0], id='second-model', default=False)
        updated = ok('PUT', '/v1/node/profiles/fixture/models', {'models': models + [second]})
        assert len(updated['models']) == 2
        stored = json.loads((node.root / 'profiles/fixture.json').read_text())
        assert stored['auth'] == original['auth'] and stored['base_url'] == original['base_url']
        invalid = dict(second, default_thinking='unsupported')
        assert request('PUT', '/v1/node/profiles/fixture/models', {'models': [invalid]})[0] >= 400
        assert json.loads((node.root / 'profiles/fixture.json').read_text()) == stored
        selection = dict(profile_id='fixture', model='fixture-model', thinking='off')
        leader = ok('POST', '/v1/node/agents', dict(selection, id='leader', name='Leader', role='leader'))
        ok('POST', '/v1/node/agents/leader/open')
        worker = ok('POST', '/v1/node/agents', dict(selection, id='worker', name='Worker', role='worker', allowed_leaders=['leader']))
        changed = ok('PATCH', '/v1/node/agents/leader/model', dict(profile_id='fixture', model='second-model'))
        assert changed['session_id'] == leader['session_id'] and changed['thinking'] == 'off'
        ok('POST', '/v1/node/agents/leader/open')
        summaries = ok('GET', '/v1/im/sessions')['items']
        assert next(s for s in summaries if s['session_id'] == leader['session_id'])['model'] == 'second-model'
        assert request('PATCH', '/v1/node/agents/leader/model', dict(profile_id='missing', model='m'))[0] == 400
        task = ok('POST', '/v1/agent/tasks', dict(worker_id='worker', request_id='assignment', goal='Original task goal'), extra={'x-zork-session-key': leader['session_key']})['task']
        f.wait(lambda: ok('GET', '/v1/tasks/' + task['task_id'])['task']['last_run_status'] == 'finished', 'Worker turn finished')
        before = ok('GET', '/v1/tasks/' + task['task_id'])['task']
        changed_worker = ok('PATCH', '/v1/node/agents/worker/model', dict(profile_id='fixture', model='second-model'))
        assert changed_worker['allowed_leaders'] == worker['allowed_leaders']
        summary = next(s for s in ok('GET', '/v1/im/sessions')['items'] if s['session_id'] == task['session_id'])
        assert summary['can_send'] and summary['model'] == 'fixture-model'
        path = '/v1/im/sessions/' + task['session_id'] + '/messages'
        comment = {'content': 'Please explain the chosen approach', 'request_id': 'comment-one'}
        first = ok('POST', path, comment)['message']
        duplicate = ok('POST', path, comment)['message']
        assert first['id'] == duplicate['id'] and first['created_at'] == duplicate['created_at']
        assert request('POST', path, dict(comment, content='different content'))[0] == 409
        history = ok('GET', path)['items']
        assignment = next(m for m in history if m['id'].startswith('assignment-'))
        assert assignment['author_agent_id'] == 'leader' and assignment['role'] == 'assistant'
        assert sum(m['id'] == first['id'] for m in history) == 1
        marker = next(m for m in ok('GET', '/v1/node/conversations/read-markers')['items'] if m['session_id'] == task['session_id'])
        assert marker['last_message_id'] == first['id']
        def delivered():
            with sqlite3.connect(node.root / 'state/station.sqlite') as db:
                return db.execute("SELECT delivered FROM leader_notifications WHERE id=?", ('comment-' + first['id'],)).fetchone()[0] == 1
        f.wait(delivered, 'Comment delivered to owning Leader')
        after = ok('GET', '/v1/tasks/' + task['task_id'])['task']
        assert (after['state'], after['revision'], after['run_count']) == (before['state'], before['revision'], before['run_count'])
        connection = dict(original, provider='openai-compatible', base_url=f'http://127.0.0.1:{provider.server_port}/v1', models=[])
        empty = ok('PUT', '/v1/node/profiles/manual.profile', connection)
        assert empty['auth_configured'] and not empty['models']
        discovery = ok('GET', '/v1/node/profiles/manual.profile/discovered-models')
        assert discovery['supported'] and discovery['items'] == [{'id': 'provider-model'}]
        assert requests == [('/v1/models', 'Bearer sk-test')]
        assert not ok('GET', '/v1/node/profiles/manual.profile')['models'], 'Discovery silently configured a model'
        assert request('POST', '/v1/node/profiles/manual.profile/models/refresh', token=False)[0] == 401
        refreshed_models = ok('POST', '/v1/node/profiles/manual.profile/models/refresh')
        assert refreshed_models['added'] == 1 and refreshed_models['profile']['models'][0]['enabled']
        assert refreshed_models['profile']['models'][0]['limits']['context_window_tokens'] == 32000
        assert 'must-not-forward' not in json.dumps(refreshed_models) and 'sk-test' not in json.dumps(refreshed_models)
        assert ok('POST', '/v1/node/profiles/manual.profile/models/refresh')['added'] == 0
        toggle = '/v1/node/profiles/manual.profile/models/enabled'
        assert request('PUT', toggle, {'model_id': 'provider-model', 'enabled': False}, token=False)[0] == 401
        disabled = ok('PUT', toggle, {'model_id': 'provider-model', 'enabled': False})
        assert disabled['models'][0]['enabled'] is False
        saved = json.loads((node.root / 'profiles/manual.profile.json').read_text())
        assert saved['models'][0]['enabled'] is False and saved['auth'] == connection['auth']
        blocked = dict(profile_id='manual.profile', model='provider-model', thinking='off', id='blocked-worker', name='Blocked', role='worker', allowed_leaders=['leader'])
        assert request('POST', '/v1/node/agents', blocked)[0] == 400
        assert request('PATCH', '/v1/node/agents/worker/model', dict(profile_id='manual.profile', model='provider-model'))[0] == 400
        assert not ok('POST', '/v1/node/profiles/manual.profile/models/refresh')['profile']['models'][0]['enabled']
        ok('PUT', toggle, {'model_id': 'provider-model', 'enabled': True})
        created = ok('POST', '/v1/node/agents', dict(blocked, id='enabled-worker', name='Enabled'))
        assert created['model'] == 'provider-model'
        automatic = dict(id='pool-worker', name='Pool worker', role='worker', model='fixture-model', effort='off', allowed_leaders=['leader'])
        pooled = ok('POST', '/v1/node/agents', automatic)
        assert pooled['profile_id'] == 'auto' and pooled['thinking'] == 'off'
        assert ok('POST', '/v1/node/agents', automatic)['id'] == pooled['id'], 'Auto create must be idempotent'
        changed = ok('PATCH', '/v1/node/agents/pool-worker/model', dict(model='second-model', effort='off'))
        assert changed['profile_id'] == 'auto' and changed['model'] == 'second-model'
        pooled_task = ok('POST', '/v1/agent/tasks', dict(worker_id='pool-worker', request_id='pool-assignment', goal='Use the account pool'), extra={'x-zork-session-key': leader['session_key']})['task']
        f.wait(lambda: ok('GET', '/v1/tasks/' + pooled_task['task_id'])['task']['last_run_status'] == 'finished', 'Automatic Worker turn finished')
        pooled_summary = next(item for item in ok('GET', '/v1/im/sessions')['items'] if item['session_id'] == pooled_task['session_id'])
        assert pooled_summary['profile_id'] == 'auto'
        local = ok('POST', '/v1/im/sessions', dict(model='fixture-model', effort='off', workspace=str(node.workspace)))
        assert local['profile_id'] == 'auto'
        local_id = local['session_id']
        fixed = ok('PUT', '/v1/im/sessions/' + local_id + '/selection', dict(profile_id='fixture', model='fixture-model', thinking='off'))
        assert fixed['profile_id'] == 'fixture'
        pooled_again = ok('PUT', '/v1/im/sessions/' + local_id + '/selection', dict(model='fixture-model', effort='off'))
        assert pooled_again['profile_id'] == 'auto'
        print(json.dumps({'ok': True, 'fixture': str(root), 'checks': ['node authorization', 'private model edits', 'Leader model change retains session', 'Worker model change retains Task selection', 'durable task comments to Leader', 'real unread markers', 'provider model enumeration', 'automatic account pool API and Worker execution']}, ensure_ascii=False))
    finally:
        node.stop()
        provider.shutdown()
        provider.server_close()


if __name__ == '__main__':
    main()
