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
from urllib.request import urlopen

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

    try:
        for node in nodes:
            node.config['mesh']['enabled'] = False
            node.config['admin'] = {'token': 'settings-fixture-secret'}
            (node.root / 'config.json').write_text(json.dumps(node.config))
            node.start()
            f.wait(lambda: node.request('GET', '/readyz')[0] == 200, 'fixture ready')
            f.wait(lambda: urlopen(node.agent_url + '/readyz', timeout=2).status == 200, 'fixture Agent ready')
        with sqlite3.connect(client / 'client.db') as db:
            db.executescript('CREATE TABLE nodes(id TEXT PRIMARY KEY,value TEXT NOT NULL);'
                'CREATE TABLE cache(node TEXT,key TEXT,value TEXT,PRIMARY KEY(node,key));')
            db.execute('INSERT INTO cache VALUES (?,?,?)', ('device', 'local-node-enabled', 'false'))
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
        ready('device-refresh')
        capture('device-settings')
        click('settings-models')
        ready('models-add')
        capture('model-connections')
        click('desktop-return')
        assert not native.element('agent-editor-dialog'), 'obsolete Agent editor is visible'
        (art / f'verification-{size}.json').write_text(json.dumps(dict(passed=True, size=size,
            checks=['appearance sidebar tab', 'diagnostics and About removed',
                    'device settings', 'model connections', 'obsolete editor absent']), indent=2) + '\n')
        print(f'PASS native client settings at {size}: {art}', flush=True)
    finally:
        native.stop()
        native.log.close()
        for node in nodes:
            node.stop()
        print(root, flush=True)


if __name__ == '__main__':
    main()
