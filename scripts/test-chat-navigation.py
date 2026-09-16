#!/usr/bin/env python3
"""Creator navigation and explicit task contexts on isolated real Mesh nodes."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('channels', ROOT / 'scripts/test-chat-channels.py')
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)
f = c.fixture
REPORT = Path(os.environ.get('ZORK_TEST_ARTIFACT_DIR', ROOT / 'artifacts/chat-navigation-implementation/process'))


def received(node, session, message):
    status, value = c.request(node, 'GET', f'/sessions/{session}/messages?limit=200', agent=True)
    if status in (404, 503): return 0
    assert status == 200, (status, value)
    items = [json.loads(item['content']) for item in value['items']
        if item['role'] == 'mailbox' and item['content'].startswith('{')]
    return sum(item.get('source') == 'chat' and item.get('message', {}).get('message_id') == message for item in items)


def settled(node, session):
    status, value = c.request(node, 'GET', f'/sessions/{session}', agent=True)
    return status == 200 and value['status'] in ('idle', 'finished', 'cancelled')


def main():
    REPORT.mkdir(parents=True, exist_ok=True)
    checks, nodes = [], []

    def passed(label):
        checks.append(label)
        print('PASS: ' + label, flush=True)

    with tempfile.TemporaryDirectory(prefix='zork-navigation-') as directory:
        root = Path(directory)
        os.environ['ZORK_REGISTRY_DIR'] = str(root / 'registry')
        try:
            a, b = f.Node(root / 'owner'), f.Node(root / 'executor')
            nodes = [a, b]
            for node, other in ((a, b), (b, a)):
                node.pair(other)
                node.config['admin'] = {'token': c.TOKEN}
                node.config['mesh']['peers'][0].update(client=True, collaborate=True)
                (node.root / 'config.json').write_text(json.dumps(node.config))
                c.start(node)
            leader, home = c.make_caller(a, 'creator')
            observer, remote_home = c.make_caller(b, 'remote-observer')
            home_members = c.ok(a, 'GET', f'/v1/im/sessions/{home}/status')['items']
            partner = next(member for member in home_members if member['id'] == 'creator')
            assert partner['home'] and partner['session_id'] == leader
            assert c.operation(a, leader, 'chat.inspect', {'chat_id': home})['participants'][0]['author']['kind'] == 'user'

            created = c.operation(a, leader, 'chat.create', {'title': 'Created without participation'})
            assert created['creator']['id'] == 'creator'
            assert c.operation(a, leader, 'chat.inspect', {'chat_id': created['chat_id']})['participants'] == []
            remote_created = c.operation(b, observer, 'chat.create', {'target': a.origin, 'title': 'Remote creator'})
            assert remote_created['creator']['id'] == b.origin + '/remote-observer'
            posted = c.operation(b, observer, 'chat.send', {'target': a.origin, 'chat_id': created['chat_id'], 'text': 'Different first author'})
            summary = next(chat for chat in c.ok(a, 'GET', '/v1/node/chats')['items'] if chat['chat_id'] == created['chat_id'])
            assert summary['creator'] == created['creator'] and summary['last_message_at'] == posted['created_at']
            passed('authenticated local/remote creation survives different first authors and no subscription')

            default = c.operation(a, leader, 'agent.create', {'config': {'name': 'Definition only',
                'selection': {'profile_id': 'fixture', 'model': 'fixture-model', 'thinking': 'off'}}})['agent']
            assert default['role'] == 'worker' and default['allowed_leaders'] == ['creator']
            assert default['session_id'] is None

            config = {'name': 'Builder', 'avatar': 'chick', 'selection': {'profile_id': 'fixture', 'model': 'fixture-model', 'thinking': 'off'}, 'instructions': 'Complete the assigned work'}
            c.ok(a, 'POST', f'/v1/im/sessions/{home}/messages', {'request_id': 'review-worker',
                'content': json.dumps({'fake_tool': {'name': 'agent.create', 'input': {'config': config, 'review': True}}})})
            notice = f.wait(lambda: next((item for item in c.mailbox(a, leader)
                if item.get('kind') == 'user_action_required'), None), 'original create invocation requests review')
            proposal = c.operation(a, leader, 'chat.send', {'chat_id': home, 'text': 'Prepare a worker for the original work',
                'interaction': {'request_id': notice['request_id']}})
            c.ok(a, 'POST', f'/v1/node/chats/{home}/messages/{proposal["message_id"]}/agent-configuration',
                {'response_id': 'confirm-worker', 'accept': True, 'values': {}})
            completion = f.wait(lambda: next((item['event']['result']['data'] for item in
                c.ok(a, 'GET', f'/sessions/{leader}/history?limit=200', agent=True)['items']
                if item['event']['kind'] == 'tool_result' and item['event']['result']['invocation_id'] == notice['invocation_id']), None),
                'same create invocation resumes with the accepted configuration')
            worker = completion['agent']
            assert worker['role'] == 'worker' and worker['session_id'] is None
            assert not c.sql(a, 'SELECT 1 FROM chat_agent_home WHERE agent_id=?', (worker['id'],))
            passed('confirmed card creates only a worker definition and resumes the original long-term context')

            tasks = []
            for index in (1, 2):
                invocation = f'assign-{index}'
                args = {'worker_id': worker['id'], 'goal': f'Independent work {index}'}
                work = c.operation(a, leader, 'agent.assign', args, invocation)
                assert c.operation(a, leader, 'agent.assign', args, invocation) == work
                assert work['state'] == 'queued' and work['chat']['creator']['id'] == 'creator'
                chat = work['chat']['chat_id']
                runtime = f.wait(lambda: next((row[0] for row in c.sql(a, 'SELECT w.session_id FROM worker_tasks w JOIN chat_channels c ON c.session_key=w.session_key WHERE c.chat_id=?', (chat,))), None), 'task allocation')
                f.wait(lambda: settled(a, runtime), 'initial work settles')
                inputs = [item for item in c.mailbox(a, runtime) if item.get('source') == 'assignment']
                assert len(inputs) == 1 and inputs[0]['chat_id'] == chat, inputs
                members = c.ok(a, 'GET', f'/v1/im/sessions/{chat}/status')['items']
                executor = next(member for member in members if member['id'] == worker['id'])
                assert executor['assigned'] and executor['session_id'] == runtime
                authors = c.operation(a, leader, 'chat.inspect', {'chat_id': chat})['participants']
                assert all(p['author']['id'] != worker['id'] for p in authors)
                tasks.append((chat, runtime))
            assert tasks[0][1] != tasks[1][1]
            assert c.runtime(a, worker['id']) is None
            passed('two tasks reuse one worker definition with separate histories; initial goals are not duplicated')

            chat, runtime = tasks[0]
            c.sql(a, "UPDATE product_tasks SET state='completed' WHERE session_key=(SELECT session_key FROM chat_channels WHERE chat_id=?)", (chat,))
            sent = c.operation(a, leader, 'chat.send', {'chat_id': chat, 'text': 'Continue after the old completion flag'})
            f.wait(lambda: received(a, runtime, sent['message_id']) == 1, 'follow-up uses first task context')
            assert received(a, tasks[1][1], sent['message_id']) == 0
            reply = c.operation(a, runtime, 'chat.send', {'chat_id': chat, 'text': 'Task result'})
            f.wait(lambda: received(a, leader, reply['message_id']) == 1, 'task result keeps leader continuity')
            assert received(a, runtime, reply['message_id']) == 0
            assert c.runtime(a, 'creator') == leader
            passed('follow-ups retain their task context after legacy completion; leader continuity and no self echo hold')

            remote_worker = c.ok(b, 'POST', '/v1/node/agents', dict(config['selection'], id='remote-builder', name='Remote builder', role='worker', allowed_leaders=[a.origin + '/creator']))
            remote = c.operation(a, leader, 'agent.assign', {'worker_id': b.origin + '/remote-builder', 'goal': 'Remote independent work'})
            remote_chat = remote['chat']['chat_id']
            assignment = 'worker-' + remote_chat
            remote_runtime = f.wait(lambda: next((row[0] for row in c.sql(b, 'SELECT runtime_id FROM mesh_runtime_sessions WHERE assignment_id=?', (assignment,))), None), 'remote allocation')
            f.wait(lambda: settled(b, remote_runtime), 'remote initial work settles')
            members = c.ok(a, 'GET', f'/v1/im/sessions/{remote_chat}/status')['items']
            assert next(member for member in members if member['id'] == b.origin + '/remote-builder')['session_id'] == remote_chat
            sent = c.operation(a, leader, 'chat.send', {'chat_id': remote_chat, 'text': 'Continue remotely in the same context'})
            f.wait(lambda: received(b, remote_runtime, sent['message_id']) == 1, 'remote follow-up')
            assert c.runtime(b, 'remote-builder') is None
            assert len([item for item in c.mailbox(b, remote_runtime) if item.get('source') == 'assignment']) == 1
            history = c.ok(a, 'GET', f'/v1/im/sessions/{remote_chat}/history?limit=20')
            assert history['items'], history
            remote_before = c.sql(b, 'SELECT COUNT(*) FROM sessions')[0][0]
            for index in range(6):
                bulk = c.operation(a, leader, 'chat.send', {'chat_id': remote_chat, 'text': f'History segment {index}: ' + 'x' * 24000})
                f.wait(lambda: received(b, remote_runtime, bulk['message_id']) == 1, 'large remote history input')
            large_history = c.ok(a, 'GET', f'/v1/im/sessions/{remote_chat}/history?limit=200')
            assert len(json.dumps(large_history).encode()) > 128 * 1024
            assert c.sql(b, 'SELECT COUNT(*) FROM sessions')[0][0] == remote_before
            passed('Mesh follow-ups and owner-side history resolve the exact remote execution without a worker home')

            b.stop()
            backlog = c.operation(a, leader, 'chat.send', {'chat_id': remote_chat, 'text': 'Queued while executor is offline'})
            c.start(b)
            f.wait(lambda: received(b, remote_runtime, backlog['message_id']) == 1, 'offline execution continuation')
            assert received(b, remote_runtime, sent['message_id']) == 1
            assert c.runtime(b, 'remote-builder') is None
            a.stop()
            c.start(a)
            assert next(chat for chat in c.ok(a, 'GET', '/v1/node/chats')['items'] if chat['chat_id'] == created['chat_id'])['creator'] == created['creator']
            replay = c.operation(a, leader, 'agent.assign', {'worker_id': worker['id'], 'goal': 'Independent work 1'}, 'assign-1')
            assert replay['chat']['chat_id'] == tasks[0][0]
            assert any(chat['chat_id'] == tasks[0][0] for chat in c.ok(a, 'GET', '/v1/node/chats')['items'])
            passed('restart retains provenance, assignment receipts and exactly-once context delivery')
        finally:
            for node in nodes:
                node.stop()
                for name in ('bootstrap.log', 'supervisor.log'):
                    path = node.root / name
                    if path.exists(): shutil.copyfile(path, REPORT / (node.root.name + '-' + name))
            (REPORT / 'result.json').write_text(json.dumps({'checks': checks, 'data_removed': True,
                'fixture': 'two real local Mesh nodes, fake models, real channel and confirmation APIs'}, indent=2))
    print('PASS: creator navigation and task contexts; isolated data and nodes cleaned', flush=True)


if __name__ == '__main__':
    main()
