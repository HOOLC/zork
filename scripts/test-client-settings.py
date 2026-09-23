#!/usr/bin/env python3
"""Native client settings checks using isolated local fixtures."""
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import time
from urllib.request import Request, urlopen

spec = importlib.util.spec_from_file_location('ui', Path(__file__).parent / 'lib/native_gui_fixture.py')
ui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ui)
f = ui.fixture
f.TARGET = Path(os.environ.get('CARGO_TARGET_DIR', f.ROOT / 'target')) / 'debug'


def main():
    root = Path(tempfile.mkdtemp(prefix='zork-client-settings-', dir='/tmp'))
    client = root / 'client'
    client.mkdir()
    nodes = [f.Node(root / name) for name in ('private-device-a', 'private-device-b')]
    native = ui.Native(None, root)
    size = os.environ.get('ZORK_GUI_TEST_WINDOW_SIZE', '1280x800')
    art = Path(os.environ.get('ZORK_GUI_SCREENSHOT_DIR', f.ROOT / 'artifacts/client-settings-pc/native-final'))
    art.mkdir(parents=True, exist_ok=True)

    def ready(id):
        return f.wait(lambda: native.element(id, True), id)

    def click(id):
        ready(id)
        native.click(id)

    def capture(name):
        time.sleep(0.35)  # Capture settled navigation and modal animation endpoints.
        native.screenshot(art / f'{name}-{size}.png')
        (art / f'{name}-{size}.json').write_bytes(native.ui('/v1/elements?include_hidden=true'))

    def request(method, path, body=None, extra=None):
        return json.load(urlopen(Request(nodes[0].url + path, method=method,
            data=None if body is None else json.dumps(body).encode(),
            headers={'Content-Type': 'application/json', 'Authorization': 'Bearer settings-fixture-secret', **(extra or {})})))

    try:
        for node in nodes:
            node.config['mesh']['enabled'] = False
            node.config['admin'] = {'token': 'settings-fixture-secret'}
            (node.root / 'config.json').write_text(json.dumps(node.config))
            node.start()
            f.wait(lambda: node.request('GET', '/readyz')[0] == 200, 'fixture ready')
            f.wait(lambda: urlopen(node.agent_url + '/readyz', timeout=2).status == 200, 'fixture Agent ready')
        selection = dict(profile_id='fixture', model='fixture-model', thinking='off')
        leader = request('POST', '/v1/node/agents', dict(selection, id='leader', name='产品领队', role='leader', avatar='fox'))
        request('POST', '/v1/node/agents/leader/open', {})
        request('POST', '/v1/node/agents', dict(selection, id='worker', name='测试队员', role='worker', avatar='dog', allowed_leaders=['leader']))
        task = request('POST', '/v1/agent/tasks', dict(worker_id='worker', request_id='settings-arrow-fixture',
            goal='检查设置页面'), {'x-zork-session-key': leader['session_key']})['task']
        with sqlite3.connect(client / 'client.db') as db:
            db.executescript('CREATE TABLE nodes(id TEXT PRIMARY KEY,value TEXT NOT NULL);'
                'CREATE TABLE cache(node TEXT,key TEXT,value TEXT,PRIMARY KEY(node,key));')
            db.execute('INSERT INTO cache VALUES (?,?,?)', ('device', 'local-node-enabled', 'false'))
            db.execute('INSERT INTO cache VALUES (?,?,?)', ('device', 'navigation-collapsed', json.dumps(['node-0/leader'])))
            for i, node in enumerate(nodes):
                saved = dict(id=f'node-{i}', name=node.root.name, url=node.url,
                             token='settings-fixture-secret', local=False)
                db.execute('INSERT INTO nodes VALUES (?,?)', (saved['id'], json.dumps(saved)))
        env = dict(os.environ, ZORK_CLIENT_DATA=str(client), ZORK_GUI_LOCALE='zh-CN',
            ZORK_GUI_PREFERENCES_PATH=str(root / 'preferences.json'), ZORK_GUI_TEST_WINDOW_SIZE=size)
        native.process = subprocess.Popen([str(f.TARGET / 'zork-gui'), '--dev', '--dev-port',
            native.url.rsplit(':', 1)[1], '--dev-token', 'mesh-native-fixture'], env=env,
            stdout=native.log, stderr=native.log)
        f.wait(lambda: native.ui('/health'), 'native GUI')
        f.wait(lambda: native.element('client_notifications') or native.element('desktop-manage'), 'navigation')
        if not native.element('client_notifications'):
            click('desktop-manage')
        click('client_notifications')
        assert ready('client_notifications')['bounds']['x'] < 240, 'client tabs must be in the sidebar'
        for removed in ('client_diagnostics', 'client_about', 'client-check-connections', 'client-copy-diagnostics', 'client-license'):
            assert not native.element(removed), f'removed client setting remains: {removed}'
        capture('client-settings')
        click('settings-device-node-0')
        click('settings-node-0-1')
        ready('agent-settings-leader')
        capture('agents-no-arrows')
        click('agent-settings-leader')
        ready('agent-editor-dialog')
        time.sleep(0.3)  # Wait for the native compositor's entry animation.
        capture('agent-modal')
        if tool := os.environ.get('ZORK_COMPOSITOR_CAPTURE'):
            result = subprocess.run([tool, str(native.process.pid), str(art / f'agent-modal-composited-{size}.png')], capture_output=True, text=True)
            print(f'Native compositor capture: {result.returncode}', flush=True)
        native.ui('/v1/actions', {'type': 'key', 'keystroke': 'escape'})
        click('settings-node-0-0')
        ready('profile-detail-fixture')
        capture('models-no-arrows')
        click('desktop-return')
        ready('leader-node-0-leader')
        ready(f"leader-task-node-0-{task['task_id']}")
        assert not native.element('leader-toggle-node-0/leader')
        capture('tasks-no-arrows')
        (art / f'verification-{size}.json').write_text(json.dumps(dict(passed=True, size=size,
            checks=['client settings sidebar tabs', 'diagnostics and About removed',
                    'arrowless agent/model lists', 'tasks visible without fold arrow']), indent=2) + '\n')
        print(f'PASS native client settings at {size}: {art}', flush=True)
    finally:
        native.stop()
        native.log.close()
        for node in nodes:
            node.stop()
        print(root, flush=True)


if __name__ == '__main__':
    main()
