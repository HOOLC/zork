#!/usr/bin/env python3
"""The seven public MCP tools over real isolated Station/Mesh and a fake model."""
import importlib.util
import json
import os
from pathlib import Path
import shlex
import shutil
import tempfile
import threading
from http.server import ThreadingHTTPServer

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('mcp_fixture', ROOT / 'scripts/test-mcp.py')
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
f = m.f


def main():
    root = Path(tempfile.mkdtemp(prefix='zork-mcp-management-'))
    print('fixture:', root, flush=True)
    nodes = []; http = None; success = False; checks = []
    try:
        a, b = f.Node(root / 'a'), f.Node(root / 'b'); nodes = [a, b]
        a.pair(b); b.pair(a)
        for node in nodes:
            node.config['admin'] = {'token': 'mcp-fixture'}
            node.config['mesh']['peers'][0].update(client=False, collaborate=False, execute=[])
            (node.root / 'config.json').write_text(json.dumps(node.config))
            node.request = lambda method, path, body=None, n=node: m.request(n, method, path, body)
            node.start()
        for node in nodes:
            f.wait(lambda n=node: n.request('GET', '/readyz')[0] == 200, 'ready')
            f.wait(lambda n=node: n.get('/v1/mesh').get('origin') == n.origin, 'Mesh identity')
        session = a.new_task()['session_id']; counter = 0

        def ok(response):
            assert response[0] in (200, 201, 202), response
            return response[1]

        def events():
            result = []
            for segment in sorted((a.root / 'shared-files/sessions' / session / 'segments').glob('*.jsonl')):
                for line in segment.read_text().splitlines():
                    try: result.append(json.loads(line)['event'])
                    except json.JSONDecodeError: pass
            return result

        def agent(op, *, failed=False, **arguments):
            nonlocal counter
            counter += 1
            name = 'mcp.' + op
            before = {e['result']['invocation_id'] for e in events() if e['kind'] == 'tool_result'}
            ok(m.request(a, 'POST', f'/v1/im/sessions/{session}/messages', {
                'content': json.dumps({'fake_tools': [{'name': name, 'input': arguments}]}),
                'request_id': f'manage-{counter}'}))
            def completed():
                for event in events():
                    if event['kind'] != 'tool_result': continue
                    result = event['result']
                    if result['tool'] == name and result['invocation_id'] not in before:
                        assert result['outcome'] == ('failed' if failed else 'succeeded'), result
                        return result
            return f.wait(completed, name)

        def raw(op, invocation, **arguments):
            return m.request(a, 'POST', '/v1/mcp', {
                'session_id': session, 'invocation_id': invocation,
                'request': {'op': op, **arguments}})

        assert not agent('list', target=b.origin)['data']['items']
        http = ThreadingHTTPServer(('127.0.0.1', 0), m.HttpMcp)
        threading.Thread(target=http.serve_forever, daemon=True).start()
        config = {'name': 'agent-installed', 'description': 'Keep this description',
                  'tool_allowlist': ['echo'], 'transport': {'kind': 'http',
                  'url': f'http://127.0.0.1:{http.server_port}/mcp'}}
        installed_event = agent('install', target=b.origin, config=config)
        installed = installed_event['data']; server_id = installed['server_id']
        target = {'target': b.origin, 'server_id': server_id}
        reference = {'owner_origin': b.origin, 'server_id': server_id}
        inspected = agent('inspect', **target)['data']
        assert inspected['availability'] == 'ready' and inspected['items'][0]['name'] == 'echo', inspected
        assert inspected['config']['description'] == config['description']
        initial_config = inspected['config']
        definition = agent('inspect', **target, tool='echo')['data']['items'][0]
        called = agent('call', **target, tool='echo', binding_revision=definition['binding_revision'], arguments={})['data']
        assert called['result']['content'][0]['text'] == 'http-sse-ok', called
        assert agent('search', target=b.origin, query='agent-installed')['data']['items']
        checks.append('named_install_list_search_inspect_and_call_across_mutually_trusted_mesh')

        disabled = agent('update', **target, expected_revision=inspected['config_revision'], config={'enabled': False})['data']
        current = agent('inspect', **target)['data']
        assert current['availability'] == 'disabled' and current['config']['enabled'] is False
        assert current['config'] == dict(initial_config, enabled=False), current
        assert any(row['server']['server_id'] == server_id for row in agent('list', target=b.origin)['data']['items'])
        assert not agent('search', target=b.origin, query='agent-installed')['data']['items']
        stale = agent('update', **target, expected_revision=inspected['config_revision'], config={'enabled': True}, failed=True)['data']
        assert stale['state'] == 'not_dispatched' and 'mcp_revision_conflict' in stale['error'], stale
        changed = agent('update', **target, expected_revision=disabled['config_revision'], config={'description': 'Updated while disabled'})['data']
        current = agent('inspect', **target)['data']
        assert current['config']['enabled'] is False and current['config']['description'] == 'Updated while disabled'
        assert current['config']['transport'] == initial_config['transport'] and current['config']['tool_allowlist'] == ['echo']
        for patch in ({}, {'enabled': None}, {'unknown_option': True}):
            rejected = agent('update', **target, expected_revision=changed['config_revision'], config=patch, failed=True)['data']
            assert rejected['state'] == 'not_dispatched' and 'mcp_invalid_config' in rejected['error'], rejected
        enabled = agent('update', **target, expected_revision=changed['config_revision'], config={'enabled': True, 'tool_allowlist': None})['data']
        current = agent('inspect', **target)['data']
        assert current['config']['tool_allowlist'] is None and current['availability'] == 'ready'
        assert current['config']['description'] == 'Updated while disabled'
        checks.append('partial_update_preserves_fields_clears_allowlist_and_checks_revision_and_types')

        # Replay actual invocation IDs through the private transport, including
        # after a restart and after deletion. None may repeat its old mutation.
        replay = ok(raw('install', installed_event['invocation_id'], owner=b.origin, config=config))
        assert replay['config_revision'] == installed['config_revision']
        a.restart_station(); b.restart_station()
        for node in nodes: f.wait(lambda n=node: n.get('/v1/mesh').get('origin') == n.origin, 'Mesh restored')
        current = agent('inspect', **target)['data']
        assert current['config_revision'] == enabled['config_revision']
        assert current['config']['description'] == 'Updated while disabled'
        removed_event = agent('uninstall', **target, expected_revision=current['config_revision'])
        assert removed_event['data']['availability'] == 'removed'
        replay = ok(raw('uninstall', removed_event['invocation_id'], owner=b.origin, server_ref=reference, expected_revision=current['config_revision']))
        assert replay['availability'] == 'removed'
        assert ok(raw('install', installed_event['invocation_id'], owner=b.origin, config=config))['config_revision'] == installed['config_revision']
        assert not agent('list', target=b.origin)['data']['items']
        checks.append('same_invocation_receipts_survive_restart_without_duplicate_install_or_resurrection')

        script = root / 'fixture.py'; script.write_text(m.FIXTURE)
        wrapper = root / 'fixture-python'; wrapper.write_text('#!/bin/sh\nexec ' + shlex.quote(m.sys.executable) + ' "$@"\n'); wrapper.chmod(0o700)
        stdio = agent('install', target=b.origin, config={'name': 'stdio-agent', 'transport': {
            'kind': 'stdio', 'command': str(wrapper), 'args': [str(script), str(root / 'calls.jsonl')]}})['data']
        stdio_target = {'target': b.origin, 'server_id': stdio['server_id']}
        current = agent('inspect', **stdio_target)['data']
        assert current['availability'] == 'ready' and str(b.root) in current['config']['transport']['cwd']
        wrapper.unlink()
        disabled = agent('update', **stdio_target, expected_revision=current['config_revision'], config={'enabled': False})['data']
        enabled = agent('update', **stdio_target, expected_revision=disabled['config_revision'], config={'enabled': True})['data']
        broken = agent('inspect', **stdio_target)['data']
        assert broken['error'] and broken['availability'] == 'failed' and broken['config_revision'] == enabled['config_revision'], broken
        transport = broken['config']['transport']; transport['command'] = 'python3'
        repaired = agent('update', **stdio_target, expected_revision=broken['config_revision'], config={'transport': transport})['data']
        current = agent('inspect', **stdio_target)['data']
        assert current['availability'] == 'ready' and Path(current['config']['transport']['command']).is_absolute()
        checks.append('broken_installations_remain_inspectable_and_can_be_disabled_or_repaired')

        mesh = ok(m.request(b, 'GET', '/v1/node/mesh'))['config']; mesh['peers'] = []
        ok(m.request(b, 'PUT', '/v1/node/mesh', mesh))
        denied = agent('inspect', **stdio_target, failed=True)['data']
        assert 'error' in denied and 'config' not in denied, denied
        checks.append('removing_mesh_membership_revokes_access')

        catalog = next(event['tools'] for event in events() if event['kind'] == 'session_created')
        names = sorted(tool['name'] for tool in catalog)
        assert [name for name in names if name.startswith('mcp.')] == [
            'mcp.call', 'mcp.inspect', 'mcp.install', 'mcp.list', 'mcp.search', 'mcp.uninstall', 'mcp.update']
        report = {'checks': checks, 'tool_count': len(names), 'tool_catalog': names, 'fake_model': True}
        if directory := os.environ.get('ZORK_TEST_ARTIFACT_DIR'):
            output = Path(directory); output.mkdir(parents=True, exist_ok=True)
            (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        print(json.dumps(report, indent=2), flush=True)
        success = True
    finally:
        for node in reversed(nodes): node.stop()
        if http: http.shutdown(); http.server_close()
        if success: shutil.rmtree(root)


if __name__ == '__main__':
    main()
