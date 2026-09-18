#!/usr/bin/env python3
"""Production Agent tools wait for edited user input over two real Mesh nodes."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('channels', ROOT / 'scripts/test-chat-channels.py')
channels = importlib.util.module_from_spec(spec)
spec.loader.exec_module(channels)
fixture = channels.fixture
REPORT = Path(os.environ.get('ZORK_TEST_ARTIFACT_DIR', ROOT / 'artifacts/user-interactions-rework/process'))

def send_tool(node, home, name, arguments, identifier):
    return channels.ok(node, 'POST', f'/v1/im/sessions/{home}/messages', {
        'request_id': identifier, 'content': json.dumps({'fake_tool': {'name': name, 'input': arguments}})})

def tool_results(node, session, invocation):
    return [item['event']['result'] for item in channels.ok(node, 'GET', f'/sessions/{session}/history?limit=200', agent=True)['items']
        if item['event']['kind'] == 'tool_result' and item['event']['result']['invocation_id'] == invocation]

def next_event(response):
    name, data = '', []
    while True:
        line = response.readline()
        assert line, 'SSE stream closed before the business result'
        line = line.decode().rstrip('\r\n')
        if not line:
            if data:
                return name, json.loads('\n'.join(data))
        elif line.startswith('event:'):
            name = line[6:].strip()
        elif line.startswith('data:'):
            data.append(line[5:].lstrip())

def main():
    REPORT.mkdir(parents=True, exist_ok=True)
    checks, nodes = [], []
    def passed(label):
        checks.append(label)
        print('PASS: ' + label, flush=True)
    with tempfile.TemporaryDirectory(prefix='zork-business-interactions-') as directory:
        root = Path(directory)
        os.environ['ZORK_REGISTRY_DIR'] = str(root / 'registry')
        try:
            a, b = fixture.Node(root / 'a'), fixture.Node(root / 'b')
            nodes = [a, b]
            for node, other in ((a, b), (b, a)):
                node.pair(other)
                node.config['admin'] = {'token': channels.TOKEN}
                node.config['mesh']['peers'][0].update(client=True, collaborate=True)
                (node.root / 'config.json').write_text(json.dumps(node.config))
                channels.start(node)
            caller_a, _ = channels.make_caller(a, 'caller-a')
            caller_b, home_b = channels.make_caller(b, 'caller-b')
            chat_b = channels.operation(b, caller_b, 'chat.create', {'title': 'Business tool review'})['chat_id']
            config = {'name': 'Suggested worker', 'selection': {'profile_id': 'fixture', 'model': 'fixture-model', 'thinking': 'off'},
                'instructions': 'Original instructions', 'skill_paths': []}
            seen = set()
            def start(name, args, label):
                send_tool(b, home_b, name, dict(args, target=a.origin), label)
                notice = fixture.wait(lambda: next((m for m in channels.mailbox(b, caller_b)
                    if m.get('kind') == 'user_action_required' and m['request_id'] not in seen), None), label + ' notification')
                seen.add(notice['request_id'])
                return notice
            def oauth(profile_id, label):
                login_chat = channels.operation(a, caller_a, 'chat.create', {'title': 'Connect account'})['chat_id']
                before = {row[0] for row in channels.sql(a, 'SELECT request_id FROM provider_login_cards')}
                send_tool(b, home_b, 'chat.send', {'target': a.origin, 'chat_id': login_chat, 'oauth': {
                    'kind': 'profile', 'profile_id': profile_id, 'provider': 'anthropic', 'billing': 'subscription'}}, label)
                row = fixture.wait(lambda: next((row for row in channels.sql(a,
                    'SELECT p.request_id,r.invocation_id,b.message_id FROM provider_login_cards p JOIN interaction_registrations r USING(request_id) JOIN business_card_bindings b USING(request_id)')
                    if row[0] not in before), None), label + ' automatic card')
                request_id, invocation_id, message_id = row
                assert not any(m.get('kind') == 'user_action_required' and m.get('request_id') == request_id for m in channels.mailbox(b, caller_b))
                message = next(m for m in channels.ok(a, 'GET', f'/v1/im/sessions/{login_chat}/messages')['items'] if m.get('message_id', m.get('id')) == message_id)
                assert message['interaction']['request']['action'] == 'oauth', message
                return {'request_id': request_id, 'invocation_id': invocation_id}, f'/v1/node/chats/{login_chat}/messages/{message_id}/provider-login'
            def publish(notice, label):
                send_tool(b, home_b, 'chat.send', {'chat_id': chat_b, 'text': 'Review this pending business operation',
                    'interaction': {'request_id': notice['request_id']}}, label)
                card = fixture.wait(lambda: next(iter(channels.sql(b,
                    'SELECT message_id FROM business_card_bindings WHERE request_id=?', (notice['request_id'],))), None), 'publish card')[0]
                return f'/v1/node/chats/{chat_b}/messages/{card}/agent-configuration', card
            def state(notice):
                handler = channels.sql(a, 'SELECT handler FROM interaction_registrations WHERE request_id=?', (notice['request_id'],))[0][0]
                assert handler in ('agent.configuration', 'provider.login'), handler
                table = 'agent_configuration_cards' if handler == 'agent.configuration' else 'provider_login_cards'
                row = channels.sql(a, f'SELECT result FROM {table} WHERE request_id=?', (notice['request_id'],))[0][0]
                return json.loads(row) if row else None
            def final(notice):
                values = fixture.wait(lambda: tool_results(b, caller_b, notice['invocation_id']), 'original business invocation finishes')
                assert len(values) == 1, values
                return values[0]['data']
            def model_choice(notice, selection):
                form = json.loads(channels.sql(a, 'SELECT form FROM agent_configuration_cards WHERE request_id=?', (notice['request_id'],))[0][0])
                field = next(field for field in form['fields'] if field['id'] == '/selection')
                return next(option['value'] for option in field['options'] if json.loads(option['value']) == selection)

            notice = start('agent.create', {'config': config, 'review': True}, 'create-reviewed')
            assert notice['target'] == a.origin
            assert 'request' not in notice
            columns = {row[1] for row in channels.sql(a, 'PRAGMA table_info(interaction_registrations)')}
            assert columns == {'request_id', 'owner', 'invocation_id', 'request_key', 'handler', 'active', 'cleanup'}, columns
            assert not channels.sql(a, "SELECT id FROM node_agents WHERE json_extract(value,'$.name')=?", (config['name'],))
            assert not tool_results(b, caller_b, notice['invocation_id'])
            path, card = publish(notice, 'publish-create')
            assert not tool_results(b, caller_b, notice['invocation_id'])
            assert {'/name', '/selection', '/instructions', '/skill_paths', '/role'} <= {f['id'] for f in json.loads(channels.sql(a, 'SELECT form FROM agent_configuration_cards WHERE request_id=?', (notice['request_id'],))[0][0])['fields']}
            assert channels.request(b, 'POST', path.rsplit('/', 1)[0] + '/respond', {'response_id': 'retired', 'accept': False})[0] == 404
            assert channels.request(b, 'GET', path.rsplit('/', 1)[0] + '/private')[0] == 404
            assert channels.request(b, 'GET', path.rsplit('/', 1)[0] + '/provider-login')[0] == 400
            passed('registration stores only management metadata; the Agent configuration business owns its pending card and endpoint')

            assert channels.request(b, 'POST', path, {'response_id': 'empty-name', 'accept': True, 'values': {'/name': ''}})[0] == 400
            invalid = channels.request(b, 'POST', path, {'response_id': 'invalid-model', 'accept': True,
                'values': {'/selection': json.dumps({'profile_id': 'fixture', 'model': 'model-that-does-not-exist', 'thinking': 'off'}, sort_keys=True, separators=(',', ':'))}})
            assert invalid[0] == 400, invalid
            assert state(notice) is None
            assert not tool_results(b, caller_b, notice['invocation_id'])
            response = {'response_id': 'create-accepted', 'accept': True,
                'values': {'/name': 'User edited worker', '/instructions': 'User supplied instructions'}}
            # The requesting Agent has no Chat subscription. Its result still
            # has to wake connected clients that observe the durable source.
            with channels.urlopen(channels.Request(b.url + f'/v1/im/sessions/{chat_b}/events',
                    headers={'Authorization': 'Bearer ' + channels.TOKEN}), timeout=20) as events:
                assert next_event(events)[0] == 'snapshot'
                channels.ok(b, 'POST', path, response)
                created = final(notice)['agent']
                completed = fixture.wait(lambda: next((item for item in channels.ok(b, 'GET', f'/v1/im/sessions/{chat_b}/messages')['items']
                    if item.get('interaction', {}).get('result', {}).get('outcome') == 'completed'), None), 'published business result')
                deadline = time.monotonic() + 20
                while True:
                    assert time.monotonic() < deadline, 'No source notification for completed business result'
                    event, value = next_event(events)
                    if event == 'messages_changed' and value.get('through', 0) >= completed['source_sequence']:
                        break
            passed('SSE announces the completed source result without requiring Agent Chat membership or client polling')
            assert created['name'] == 'User edited worker' and created['instructions'] == 'User supplied instructions'
            assert len(channels.sql(a, "SELECT id FROM node_agents WHERE id=?", (created['id'],))) == 1
            settled = fixture.wait(lambda: state(notice) if state(notice) and state(notice)['outcome'] == 'completed' else None, 'business completion')
            assert settled['output']['agent']['id'] == created['id']
            assert channels.request(b, 'POST', path, dict(response, accept=False, values={}))[0] == 400
            channels.ok(b, 'POST', path, response)
            channels.ok(b, 'POST', path, dict(response, response_id='another-device'))
            assert len(channels.sql(a, "SELECT id FROM node_agents WHERE json_extract(value,'$.name')='User edited worker'")) == 1
            assert channels.received(b, caller_b, card) == 0
            passed('business validation rejects bad input without completing the call; edited values commit once independently of Chat subscriptions')

            inspected = channels.operation(b, caller_b, 'agent.inspect', {'target': a.origin, 'agent_id': created['id']})
            before = json.loads(channels.sql(a, 'SELECT value FROM node_agents WHERE id=?', (created['id'],))[0][0])
            update = start('agent.update', {'agent_id': created['id'], 'expected_revision': inspected['revision'],
                'changes': {'name': 'Suggested update'}, 'review': True}, 'update-reviewed')
            update_path, _ = publish(update, 'publish-update')
            channels.ok(b, 'POST', update_path, {'response_id': 'update-accepted', 'accept': True, 'values': {'/name': 'User updated name'}})
            updated = final(update)['agent']
            assert updated['id'] == created['id'] and updated['name'] == 'User updated name'
            after = json.loads(channels.sql(a, 'SELECT value FROM node_agents WHERE id=?', (created['id'],))[0][0])
            assert {k:v for k,v in before.items() if k != 'name'} == {k:v for k,v in after.items() if k != 'name'}
            passed('production agent.update retains every unspecified and unedited configuration field')

            partial = start('agent.create', {'config': {'name': 'Partially prefilled Agent'}}, 'create-partial')
            partial_path, _ = publish(partial, 'publish-partial')
            channels.ok(b, 'POST', partial_path, {'response_id': 'fill-missing', 'accept': True, 'values': {
                '/selection': model_choice(partial, config['selection']), '/role': 'leader'}})
            filled = final(partial)['agent']
            assert filled['name'] == 'Partially prefilled Agent' and filled['role'] == 'leader'
            passed('partial business parameters become the same editable form and retain the business tool role contract')

            declined = start('agent.create', {'config': dict(config, name='Declined worker'), 'review': True}, 'create-declined')
            decline_path, _ = publish(declined, 'publish-declined')
            channels.ok(b, 'POST', decline_path, {'response_id': 'declined', 'accept': False})
            assert final(declined)['status'] == 'rejected'
            assert not channels.sql(a, "SELECT id FROM node_agents WHERE json_extract(value,'$.name')='Declined worker'")
            passed('declining the card ends the original operation without creating an Agent')

            cancelled = start('agent.create', {'config': dict(config, name='Cancelled worker'), 'review': True}, 'create-cancelled')
            send_tool(b, home_b, 'tool.cancel', {'invocation_id': cancelled['invocation_id']}, 'cancel-business')
            fixture.wait(lambda: state(cancelled) and state(cancelled)['outcome'] == 'cancelled', 'cancel remote business wait')
            assert not channels.sql(a, "SELECT id FROM node_agents WHERE json_extract(value,'$.name')='Cancelled worker'")
            late = channels.operation(b, caller_b, 'chat.send', {'chat_id': chat_b, 'interaction': {'request_id': cancelled['request_id']}})
            assert late['interaction']['snapshot']['outcome'] == 'cancelled'
            passed('tool.cancel reaches the same invocation and a late card shows its settled state')

            # Anthropic's browser challenge is generated locally. A mismatched state
            # is rejected before its token exchange, so this exercises the real login
            # business call without contacting a provider or using an account.
            login, private_path = oauth('isolated-login', 'login-private')
            wrong_business = channels.request(a, 'POST', private_path.rsplit('/', 1)[0] + '/agent-configuration', {'response_id': 'wrong-business', 'accept': True, 'values': {}})
            assert wrong_business[0] == 400, wrong_business
            assert not tool_results(b, caller_b, login['invocation_id'])
            challenge = channels.ok(a, 'GET', private_path)['authorization']
            assert challenge['flow'] == 'browser_callback' and challenge['verification_url'].startswith('https://')
            assert not tool_results(b, caller_b, login['invocation_id'])
            bad_callback = 'isolated-code#intentionally-wrong-state'
            channels.ok(a, 'POST', private_path, {'callback': bad_callback})
            assert final(login)['status'] == 'rejected'
            for persisted in (channels.sql(a, 'SELECT form,submission,result FROM agent_configuration_cards'),
                              channels.sql(a, 'SELECT title,result FROM provider_login_cards'),
                              channels.sql(a, 'SELECT request_id,owner,invocation_id,origin FROM interaction_registration_notices'), channels.mailbox(b, caller_b)):
                encoded = json.dumps(persisted)
                assert bad_callback not in encoded and challenge['verification_url'] not in encoded
            assert 'authorization' not in channels.ok(a, 'GET', private_path)
            passed('chat.send OAuth card owns private callbacks and rejects a mismatched OAuth state without publishing private material')

            login_cancel, cancel_path = oauth('isolated-cancel-login', 'login-cancel')
            channels.ok(a, 'DELETE', cancel_path)
            assert final(login_cancel)['status'] == 'cancelled'
            passed('cancelling an external-action card stops the same production login invocation')

            restarting = start('agent.create', {'config': dict(config, name='Lost source worker'), 'review': True}, 'create-before-restart')
            b.restart_station()
            fixture.wait(lambda: state(restarting) and state(restarting)['outcome'] == 'cancelled', 'source recovery cancels the remote wait')
            assert not channels.sql(a, "SELECT id FROM node_agents WHERE json_extract(value,'$.name')='Lost source worker'")
            passed('source crash recovery cancels the remote pending business operation without replay')

            before_count = channels.sql(a, 'SELECT count(*) FROM interaction_registrations')[0][0]
            direct = channels.operation(a, caller_a, 'agent.create', {'config': dict(config, name='Authorized direct worker')})
            assert direct['agent']['name'] == 'Authorized direct worker'
            assert channels.sql(a, 'SELECT count(*) FROM interaction_registrations')[0][0] == before_count
            for removed in ('interaction.create', 'interaction.request'):
                assert channels.request(a, 'POST', '/v1/channels/tools', {'session_id': caller_a, 'invocation_id': removed,
                    'tool': removed, 'arguments': {'request': {'action': 'input', 'title': 'Removed', 'fields': []}}})[0] == 400
            assert channels.request(b, 'POST', '/v1/channels/tools', {'session_id': caller_b, 'invocation_id': 'inline-rejected',
                'tool': 'chat.send', 'arguments': {'chat_id': chat_b, 'interaction': {'action': 'agent.create', 'config': config}}})[0] == 400
            passed('authorized complete calls execute directly; standalone and inline business-form entry points are unavailable')
        finally:
            for node in nodes:
                node.stop()
                for name in ('bootstrap.log', 'supervisor.log'):
                    path = node.root / name
                    if path.exists():
                        shutil.copyfile(path, REPORT / (node.root.name + '-' + name))
            (REPORT / 'result.json').write_text(json.dumps({'checks': checks, 'data_removed': True,
                'fixture': 'two real isolated Mesh nodes; fake models invoke ordinary production business tools'}, indent=2))
    print('PASS: production user participation contract; isolated nodes cleaned', flush=True)

if __name__ == '__main__':
    main()
