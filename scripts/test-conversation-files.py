#!/usr/bin/env python3
"""Conversation -> remote Worker -> Leader copies, with real isolated Mesh nodes."""
import importlib.util
import json
import tempfile
from pathlib import Path
from urllib.error import HTTPError
from urllib.request import Request, urlopen

spec = importlib.util.spec_from_file_location('fixture', Path(__file__).with_name('test-mesh.py'))
f = importlib.util.module_from_spec(spec)
spec.loader.exec_module(f)
root = Path(tempfile.mkdtemp(prefix='zork-files-'))
a, b = f.Node(root / 'leader'), f.Node(root / 'worker')
a.pair(b)
b.pair(a)
for node in (a, b):
    node.config['admin'] = {'token': 'conversation-files-fixture'}
    (node.root / 'config.json').write_text(json.dumps(node.config))


def req(node, method, path, body=None, leader=None):
    headers = {'Content-Type': 'application/json', 'Authorization': 'Bearer conversation-files-fixture'}
    if leader:
        headers['x-zork-session-key'] = leader['session_key']
    try:
        response = urlopen(Request(node.url + path, method=method, data=None if body is None else json.dumps(body).encode(), headers=headers), timeout=20)
    except HTTPError as error:
        response = error
    with response:
        return response.status, json.load(response)


def ok(node, method, path, body=None, leader=None):
    status, value = req(node, method, path, body, leader)
    assert status in (200, 201, 202), (status, value)
    return value


def bytes_at(node, artifact):
    with urlopen(node.url + '/v1/artifacts/' + artifact['artifact_id'] + '/content') as response:
        return response.read()


try:
    for node in (a, b):
        node.start()
        f.wait(lambda: node.request('GET', '/readyz')[0] == 200, 'node ready')
    selection = {'profile_id': 'fixture', 'model': 'fixture-model', 'thinking': 'off'}
    leader = ok(a, 'POST', '/v1/node/agents', dict(selection, id='leader', name='Leader', role='leader'))
    ok(a, 'POST', '/v1/node/agents/leader/open', {})
    ok(b, 'POST', '/v1/node/agents', dict(selection, id='worker', name='Worker', role='worker', allowed_leaders=[a.origin + '/leader']))
    remote_id = b.origin + '/worker'
    f.wait(lambda: any(w['id'] == remote_id for w in ok(a, 'GET', '/v1/agent/workers', leader=leader)['items']), 'remote Worker')
    context = ok(a, 'GET', '/v1/tools/context?threadId=' + leader['session_id'])
    session = next(s for s in a.get('/v1/im/sessions')['items'] if s['session_id'] == leader['session_id'])
    payload = b'conversation snapshot\n' * 20000
    source = Path(session['workspace']) / 'source.txt'
    source.write_bytes(payload)
    post = {'sessionKey': context['sessionKey'], 'conversationId': context['conversationId'], 'rootMessageId': context['rootMessageId'], 'filePath': str(source)}
    initial = ok(a, 'POST', '/chat/post-file', post)['artifact']
    assert ok(a, 'POST', '/chat/post-file', post)['artifact']['artifact_id'] == initial['artifact_id']
    source.unlink()
    assignment = {'worker_id': remote_id, 'request_id': 'with-files', 'goal': 'Keep the attached input available.', 'attachment_ids': [initial['artifact_id']]}
    assigned = ok(a, 'POST', '/v1/agent/tasks', assignment, leader)
    assert ok(a, 'POST', '/v1/agent/tasks', assignment, leader)['session_id'] == assigned['session_id']
    assert req(a, 'POST', '/v1/agent/tasks', dict(assignment, attachment_ids=[]), leader)[0] == 409
    remote_task = f.wait(lambda: next(iter(b.get('/v1/tasks')['items']), None), 'remote task')
    remote_input = f.wait(lambda: next((v for v in b.get('/v1/artifacts')['items'] if v['session_id'] == remote_task['session_id']), None), 'remote input persisted')
    assert bytes_at(b, remote_input) == payload
    materialized = f.wait(lambda: next(iter((b.root / 'files/attachments').glob('*/source.txt')), None), 'Agent input uses the published immutable file')
    assert materialized.read_bytes() == payload
    f.wait(lambda: next(t for t in a.get('/v1/tasks')['items'] if t['task_id'] == assigned['task']['task_id'])['last_run_status'] == 'finished', 'input turn finished')
    current = next(t for t in a.get('/v1/tasks')['items'] if t['task_id'] == assigned['task']['task_id'])
    goal = json.dumps({'fake_tools': [{'name': 'chat.post_file', 'input': {'attachment_id': remote_input['artifact_id'], 'initial_comment': 'Returned input snapshot'}}, {'name': 'chat.post_message', 'input': {'kind': 'final', 'text': 'File returned'}}]})
    ok(a, 'POST', '/v1/agent/tasks/' + current['task_id'] + '/rework', {'request_id': 'return-file', 'goal': goal, 'expected_revision': current['revision']}, leader)
    returned = f.wait(lambda: next((v for v in a.get('/v1/artifacts')['items'] if v['artifact_id'].startswith('mesh-')), None), 'returned file imported')
    assert bytes_at(a, returned) == payload
    f.wait(lambda: any(t.get('result_text') == 'File returned' for t in a.get('/v1/tasks')['items']), 'final follows file')
    b.stop()
    # An offline source node is no longer needed to forward its delivered file.
    goal = json.dumps({'fake_tools': [{'name': 'chat.post_file', 'input': {'attachment_id': returned['artifact_id'], 'source_task_id': current['task_id'], 'initial_comment': 'Saved in Leader conversation'}}]})
    ok(a, 'POST', '/v1/im/sessions/' + leader['session_id'] + '/messages', {'request_id': 'forward-file', 'content': goal})
    forwarded = f.wait(lambda: next((v for v in a.get('/v1/artifacts')['items'] if v['session_id'] == leader['session_id'] and v['artifact_id'] != initial['artifact_id']), None), 'Leader owns independent copy')
    assert bytes_at(a, forwarded) == payload
    assert forwarded['task_id'] is None
    a.stop()
    a.start()
    f.wait(lambda: a.request('GET', '/readyz')[0] == 200, 'restart')
    assert bytes_at(a, forwarded) == payload
    print('PASS: assigned input copied before execution; same-ID retry; actual Mesh return; Leader copy after Worker offline; restart persistence')
finally:
    a.stop()
    b.stop()
    print(root)
