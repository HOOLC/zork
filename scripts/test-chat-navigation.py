#!/usr/bin/env python3
"""First-send Chat creation and independent Sessions on real isolated Mesh nodes."""
import importlib.util
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import shutil
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('channels', ROOT / 'scripts/test-chat-channels.py')
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)
f = c.fixture
REPORT = Path(os.environ.get('ZORK_TEST_ARTIFACT_DIR', ROOT / 'artifacts/chat-first/process'))


def identifier():
    alphabet = '0123456789ABCDEFGHJKMNPQRSTVWXYZ'
    number = uuid.uuid4().int
    result = ''
    for _ in range(26):
        result = alphabet[number & 31] + result
        number >>= 5
    return result


def first_message(reply):
    return json.dumps({'fake_tool': {'name': 'chat.post_message', 'input': {'text': reply}}}, ensure_ascii=False)


def create(node, text, **selection):
    body = dict(request_id=identifier(), content=text, model='fixture-model', thinking='high', **selection)
    return c.ok(node, 'POST', '/v1/im/chats', body), body


def reply(node, chat, text):
    return next((m for m in c.ok(node, 'GET', f'/v1/im/sessions/{chat}/messages')['items']
                 if m['role'] == 'assistant' and m['content'] == text), None)


def main():
    REPORT.mkdir(parents=True, exist_ok=True)
    checks, nodes = [], []

    def passed(label):
        checks.append(label)
        print('PASS: ' + label, flush=True)

    with tempfile.TemporaryDirectory(prefix='zork-chat-first-') as directory:
        root = Path(directory)
        os.environ['ZORK_REGISTRY_DIR'] = str(root / 'registry')
        try:
            a, b = f.Node(root / 'a'), f.Node(root / 'b')
            nodes = [a, b]
            for node, other in ((a, b), (b, a)):
                path = node.root / 'profiles/fixture.json'
                profile = json.loads(path.read_text())
                profile['models'][0]['thinking'] = ['off', 'high']
                path.write_text(json.dumps(profile))
                alternate = json.loads(json.dumps(profile))
                alternate['models'][0]['id'] = 'fixture-fast'
                (node.root / 'profiles/alternate.json').write_text(json.dumps(alternate))
                node.pair(other)
                node.config['admin'] = {'token': c.TOKEN}
                node.config['mesh']['peers'][0].update(client=True, collaborate=True)
                (node.root / 'config.json').write_text(json.dumps(node.config))
                c.start(node)

            assert c.ok(a, 'GET', '/v1/node/agents')['items'] == []
            assert c.ok(a, 'GET', '/v1/node/chats')['items'] == []
            c.ok(a, 'GET', '/v1/im/profiles')
            assert c.ok(a, 'GET', '/v1/node/chats')['items'] == []
            invalid = dict(request_id=identifier(), content='work', model='missing', thinking='high')
            assert c.request(a, 'POST', '/v1/im/chats', invalid)[0] == 400
            invalid['model'] = 'fixture-model'; invalid['content'] = ' '
            assert c.request(a, 'POST', '/v1/im/chats', invalid)[0] == 400
            assert c.ok(a, 'GET', '/v1/node/chats')['items'] == []
            passed('reading creation choices and rejecting invalid first sends allocate no Chat or Session')

            chat, request = create(a, first_message('首条回复'), client_id=identifier())
            chat_id = chat['chat_id']
            assert chat_id == request['request_id']
            f.wait(lambda: reply(a, chat_id, '首条回复'), 'first message executes and replies to its own Chat')
            f.wait(lambda: c.settled(a, chat_id), 'first Session settles')
            session = c.ok(a, 'GET', f'/sessions/{chat_id}', agent=True)
            assert session['model'] == 'fixture-model' and session['thinking'] == 'high', session
            assert session['profile_id'] == 'auto', session
            members = c.ok(a, 'GET', f'/v1/im/sessions/{chat_id}/status')['items']
            own = next(m for m in members if m['session_id'] == chat_id)
            assert own['assigned'] and own['name'] == 'Session', own
            assert c.ok(a, 'GET', '/v1/node/agents')['items'] == []
            messages = c.ok(a, 'GET', f'/v1/im/sessions/{chat_id}/messages')['items']
            first = next(m for m in messages if m['role'] == 'user')
            assert first['id'] == f'client-{chat_id}-{request["request_id"]}'
            assert first['content'] == request['content']
            assert first['client_id'].endswith('/' + request['client_id']), first
            passed('first send binds one Session with selected model/depth and optional automatic Profile; reply returns through production tools')

            with ThreadPoolExecutor(max_workers=4) as pool:
                retries = list(pool.map(lambda _: c.ok(a, 'POST', '/v1/im/chats', request), range(4)))
            assert all(value == chat for value in retries)
            user_messages = [m for m in c.ok(a, 'GET', f'/v1/im/sessions/{chat_id}/messages')['items'] if m['role'] == 'user']
            assert len(user_messages) == 1
            assert c.received(a, chat_id, first['id']) == 1
            changed = dict(request, thinking='off')
            assert c.request(a, 'POST', '/v1/im/chats', changed)[0] == 409
            passed('concurrent retries return the original creation and never duplicate its first message or execution')

            second_request = dict(request_id=identifier(), content=first_message('另一条回复'), model='fixture-fast', thinking='off', profile_id='alternate')
            second = c.ok(a, 'POST', '/v1/im/chats', second_request)
            f.wait(lambda: reply(a, second['chat_id'], '另一条回复'), 'independent second Chat')
            second_session = c.ok(a, 'GET', f'/sessions/{second["chat_id"]}', agent=True)
            assert (second_session['model'], second_session['thinking'], second_session['profile_id']) == ('fixture-fast', 'off', 'alternate')
            original = c.ok(a, 'GET', f'/sessions/{chat_id}', agent=True)
            assert (original['model'], original['thinking']) == ('fixture-model', 'high')
            assert second['chat_id'] != chat_id
            passed('separate Chats keep independent Session histories and explicit Profile/model/depth selections')

            options = c.operation(a, chat_id, 'chat.options', {'target': b.origin})['items']
            assert any(o['model'] == 'fixture-model' and 'high' in o['thinking'] for o in options)
            args = {'target': b.origin, 'title': 'Remote work', 'text': first_message('remote-ready'), 'model': 'fixture-model', 'thinking': 'high'}
            child = c.operation(a, chat_id, 'chat.create', args, 'create-independent-remote-chat')
            assert child['session_id'] == child['chat_id']
            assert child['chat_id'] != chat_id
            assert c.operation(a, chat_id, 'chat.create', args, 'create-independent-remote-chat') == child
            answer = f.wait(lambda: reply(b, child['chat_id'], 'remote-ready'), 'remote Chat reply')
            f.wait(lambda: c.received(a, chat_id, answer['id']) == 1, 'creator Session receives remote child reply')
            assert c.ok(b, 'GET', '/v1/node/agents')['items'] == []
            assert child['creator']['id'].startswith(a.origin + '/session:')
            passed('collaboration creates another Chat on the selected device without Leader/Worker definitions or per-Agent grants')

            a.stop(); b.stop()
            for node in nodes: c.start(node)
            assert c.ok(a, 'POST', '/v1/im/chats', request) == chat
            assert c.received(a, chat_id, first['id']) == 1
            session = c.ok(a, 'GET', f'/sessions/{chat_id}', agent=True)
            assert (session['model'], session['thinking'], session['profile_id']) == ('fixture-model', 'high', 'auto')
            c.ok(a, 'POST', f'/v1/im/sessions/{chat_id}/messages', {'content': first_message('重启后的回复'), 'request_id': 'after-restart'})
            f.wait(lambda: reply(a, chat_id, '重启后的回复'), 'continuation uses the same Session after restart')
            assert c.received(a, chat_id, first['id']) == 1
            f.wait(lambda: not c.sql(a, 'SELECT 1 FROM chat_mailbox WHERE delivered=0'), 'all accepted notices settle')
            passed('restart preserves identity, configuration and source watermarks; subsequent messages continue the original Session')

            legacy, home = c.make_caller(a, 'legacy-partner')
            old_messages = c.ok(a, 'GET', f'/v1/im/sessions/{home}/messages')['items']
            assert old_messages
            assert any(item['chat_id'] == home for item in c.ok(a, 'GET', '/v1/node/chats')['items'])
            assert legacy != home
            passed('historical Agent homes remain readable as Chat records while new work requires no Agent catalog')
        finally:
            for node in nodes:
                node.stop()
                for path in node.root.glob('*.log'):
                    shutil.copyfile(path, REPORT / f'{node.root.name}-{path.name}')
            (REPORT / 'report.json').write_text(json.dumps({'checks': checks}, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    main()
