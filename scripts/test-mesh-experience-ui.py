#!/usr/bin/env python3
"""Native cross-device enrollment, offline messages, and launchd ownership.

Runs only isolated fake-model nodes and a dedicated automation window. The
client is killed to exercise real ownership rather than Rust destructors.
"""
import importlib.util
import json
import os
from pathlib import Path
import shlex
import socket
import sqlite3
import subprocess
import tempfile
import time
from urllib.request import Request, urlopen

spec = importlib.util.spec_from_file_location('ui', Path(__file__).parent / 'lib/native_gui_fixture.py')
ui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ui)
spec = importlib.util.spec_from_file_location('enrollment', Path(__file__).with_name('test-mesh-enrollment.py'))
enrollment = importlib.util.module_from_spec(spec)
spec.loader.exec_module(enrollment)


def main():
    root = Path(tempfile.mkdtemp(prefix='zexperience-', dir='/tmp'))
    client = root / 'client'
    screenshots = ui.fixture.ROOT / 'artifacts/mesh-experience'
    screenshots.mkdir(parents=True, exist_ok=True)
    if os.environ.get('ZORK_TEST_BIN_DIR'):
        ui.fixture.TARGET=Path(os.environ['ZORK_TEST_BIN_DIR'])
    b = ui.fixture.Node(root / 'mini2')
    b.config['admin'] = {'token': 'enrollment-fixture'}
    b.config['mesh']['name'] = 'mini2'
    (b.root / 'config.json').write_text(json.dumps(b.config))
    native = ui.Native(None, root)
    env = dict(os.environ, ZORK_CLIENT_DATA=str(client), ZORK_NODE_BINARY=str(ui.fixture.TARGET / 'zork'),
        ZORK_DESKTOP_FAKE_AGENT='1', ZORK_REGISTRY_DIR=str(root / 'registry'),
        ZORK_GUI_PREFERENCES_PATH=str(root / 'preferences.json'), ZORK_GUI_LOCALE='zh-CN', ZORK_GUI_TEST_WINDOW_SIZE='1280x800')

    def launch(size='1280x800'):
        native.process = subprocess.Popen([str(ui.fixture.TARGET / 'zork-gui'), '--dev', '--dev-port', native.url.rsplit(':', 1)[1],
            '--dev-token', 'mesh-native-fixture'], env=dict(env, ZORK_GUI_TEST_WINDOW_SIZE=size), stdout=native.log, stderr=native.log)
        ui.wait(lambda: native.ui('/health'), 'native client ready')

    def kill_gui():
        native.process.kill()
        native.process.wait(timeout=10)
        native.process = None

    def click(element):
        for _ in range(15):
            found = native.element(element, True)
            if found and found['visible_bounds']['height'] >= 10:
                native.click(element)
                return
            native.ui('/v1/actions', {'type': 'scroll', 'target': {'x': 720, 'y': 480}, 'delta_y': -220})
            time.sleep(.15)
        raise AssertionError('element unavailable: ' + element)

    def local_config():
        return json.loads((client / 'node/config.json').read_text())

    def saved_nodes():
        with sqlite3.connect(client / 'client.db') as db:
            return [json.loads(row[0]) for row in db.execute('SELECT value FROM nodes')]

    def outbox():
        with sqlite3.connect(client / 'client.db') as db:
            return [json.loads(row[0]) for row in db.execute('SELECT value FROM outbox')]

    def alive(pid):
        try:
            os.kill(pid, 0)
            return True
        except ProcessLookupError:
            return False

    def settings():
        path = client / 'node/service.json'
        return json.loads(path.read_text()) if path.exists() else {'enabled': False}

    local_settings_id = None

    def manage_local():
        click('desktop-manage')
        click(local_settings_id)

    print('isolated native mesh experience:', root, flush=True)
    try:
        b.start()
        ui.wait(lambda: b.request('GET', '/readyz')[0] == 200, 'remote Station ready')
        ui.wait(lambda: urlopen(b.agent_url + '/readyz', timeout=2).status == 200, 'remote Agent ready')
        selection = {'profile_id': 'fixture', 'model': 'fixture-model', 'thinking': 'off'}
        leader = enrollment.admin(b, 'POST', '/v1/node/agents', dict(selection, id='remote-leader', name='mini2 Leader', role='leader'))
        enrollment.admin(b, 'POST', '/v1/node/agents/remote-leader/open', {})
        workspace = Path(next(s for s in b.get('/v1/im/sessions')['items'] if s['session_id'] == leader['session_id'])['workspace'])
        context = b.get('/v1/tools/context?threadId=' + leader['session_id'])
        binding = {'sessionKey': context['sessionKey'], 'conversationId': context['conversationId'], 'rootMessageId': context['rootMessageId']}
        for index in range(130):
            assert b.request('POST', '/chat/post-message', dict(binding, kind='progress', text=f'Reading marker {index:03d} · A cached message on mini2.'))[0] == 200

        launch()
        ui.wait(lambda: native.element('local-node-toggle', True), 'zero-node start')
        assert not (client / 'node').exists()
        click('local-node-toggle')
        ui.wait(lambda: native.element('desktop-manage'), 'local node opened')
        config = local_config()
        # Public network fixture only; no production relay or model credentials.
        config['mesh']['offline'] = True
        config['mesh']['bind'] = '127.0.0.1:' + str(ui.port(True))
        config['mesh']['name'] = 'mini1'
        (client / 'node/config.json').write_text(json.dumps(config))
        pid = int((client / 'node/zork.pid').read_text())
        pids=','.join([str(pid),(client/'node/run/zork-station.pid').read_text().strip()])
        (root/'process-masks.txt').write_text(subprocess.check_output(['ps','-p',pids,'-o','pid,blocked,pending,stat,command'],text=True))
        click('desktop-manage')
        local_settings_id = next(e['id'] for e in json.loads(native.ui('/v1/elements'))['elements']
                                 if e['id'].startswith('settings-device-') and e['visible'])
        click(local_settings_id)
        click('device-connections')
        click('mesh-connect-device-tab')
        ui.wait(lambda: native.element('mesh-invite-create', True), 'invite entry')
        click('mesh-invite-create')
        ui.wait(lambda: native.element('mesh-invite-copy', True), 'join command generated', 90)
        native.screenshot(screenshots / 'join-command.png')
        click('mesh-invite-copy')
        command = shlex.split(subprocess.check_output(['pbpaste'], text=True))
        ticket = command[command.index('join') + 1]
        assert ticket.startswith('zj1_')
        result = subprocess.run([str(ui.fixture.TARGET / 'zork'), 'mesh', 'join', ticket, '--data', str(b.root)], capture_output=True, text=True, timeout=65)
        assert result.returncode == 0, result.stderr
        ui.wait(lambda: native.element('mesh-invite-status') and 'mini2 已加入' in native.element('mesh-invite-status')['label'], 'joined confirmation')
        native.screenshot(screenshots / 'device-joined.png')
        click('desktop-return')
        remote_id = 'leader-' + b.origin + '-remote-leader'
        ui.wait(lambda: native.element(remote_id, True), 'new device and Leader appear without manual identity exchange', 90)
        assert len(saved_nodes()) == 2, 'one physical local Station appeared twice in the sidebar'
        click(remote_id)
        ui.wait(lambda: native.element('conversation-device') and 'mini2' in native.element('conversation-device')['label'], 'execution location')
        native.screenshot(screenshots / 'remote-conversation.png')
        native.ui('/v1/actions', {'type': 'scroll', 'target': {'x': 720, 'y': 380}, 'delta_y': 550})
        def reading():
            with sqlite3.connect(client / 'client.db') as db:
                row = db.execute('SELECT value FROM cache WHERE key=?', ('reading:' + leader['session_id'],)).fetchone()
                return json.loads(row[0]) if row else None
        before_reading = ui.wait(lambda: reading() if reading() and not reading()['following'] and reading()['anchors'] else None, 'reading position saved')
        native.screenshot(screenshots / 'reading-before.png')
        assert int((client / 'node/zork.pid').read_text()) == pid
        assert not (client / 'node/run/zork-agent.pid').exists()
        print('PASS: native join command, device confirmation, automatic direct client access, execution location; no Agent restart', flush=True)

        b.stop()
        ui.wait(lambda: native.element('device-offline-banner'), 'offline state', 60)
        assert '不代表任务已停止' in native.element('device-offline-banner')['label']
        goal = json.dumps({'fake_tool': {'name': 'shell.run', 'input': {'command': "printf 'one\\n' >> offline-executions.txt"}}})
        click('composer-input')
        native.type(goal)
        native.ui('/v1/actions', {'type': 'key', 'keystroke': 'enter'})
        ui.wait(lambda: len(outbox()) == 1, 'offline message saved')
        queued = outbox()[0]
        assert not queued['attempted']
        ui.wait(lambda: native.element('cancel-queued-' + queued['request_id'], True), 'pending message can return to draft')
        click('cancel-queued-' + queued['request_id'])
        ui.wait(lambda: not outbox(), 'pending message withdrawn')
        with sqlite3.connect(client / 'client.db') as db:
            assert json.loads(db.execute('SELECT value FROM cache WHERE key=?', ('draft:' + leader['session_id'],)).fetchone()[0]) == goal
        click('composer-input')
        native.ui('/v1/actions', {'type': 'key', 'keystroke': 'enter'})
        ui.wait(lambda: len(outbox()) == 1, 'offline resend persisted')
        native.screenshot(screenshots / 'offline-pending.png')
        b.start()
        ui.wait(lambda: not outbox() and (workspace / 'offline-executions.txt').exists(), 'reconnect sends once', 90)
        assert (workspace / 'offline-executions.txt').read_text().splitlines() == ['one']
        assert sum(m['content'] == goal for m in b.get('/v1/im/sessions/' + leader['session_id'] + '/messages')['items']) == 1
        assert reading()['anchors'] == before_reading['anchors'] and not reading()['following']
        native.screenshot(screenshots / 'reading-after.png')
        print('PASS: offline state preserves execution uncertainty and reading position; unsent message returns to draft; reconnect delivers once', flush=True)

        manage_local()
        click('local-node-background')
        ui.wait(lambda: settings()['enabled'] and native.element('local-node-background', True), 'background service enabled')
        assert int((client / 'node/zork.pid').read_text()) == pid
        assert not (client / 'node/run/zork-agent.pid').exists()
        native.screenshot(screenshots / 'background-station.png')
        kill_gui()
        time.sleep(2)
        assert alive(pid)
        assert urlopen('http://' + local_config()['bind']['runtime'] + '/readyz', timeout=3).status == 200
        launch('900x600')
        ui.wait(lambda: native.element('desktop-manage'), 'client reattaches existing background Station', 60)
        assert int((client / 'node/zork.pid').read_text()) == pid
        manage_local()
        native.screenshot(screenshots / 'background-station-small.png')
        click('local-node-background')  # the run-mode switch turns background off
        ui.wait(lambda: not settings()['enabled'] and native.element('local-node-background', True), 'return ownership to client')
        assert alive(pid)
        kill_gui()
        ui.wait(lambda: not alive(pid), 'client lease closes Station after background mode disabled')
        print('PASS: launchd adopts existing PID, GUI crash leaves Station running, reattach at 900×600, client lease resumes without task restart', flush=True)
        (screenshots / 'result.json').write_text(json.dumps({'root': str(root), 'checks': ['native_invite', 'auto_devices', 'execution_location', 'offline_draft', 'cancel_pending', 'reconnect_once', 'background_adoption', 'client_crash', 'service_reattach', 'lease_cleanup', 'small_window']}, indent=2))
    finally:
        if native.process and native.process.poll() is None:
            (root/'last-elements.json').write_bytes(native.ui('/v1/elements'))
            native.screenshot(root / 'last-window.png')
        native.stop()
        native.log.close()
        if (client / 'node/config.json').exists():
            subprocess.run([str(ui.fixture.TARGET / 'zork'), 'stop', '--data', str(client / 'node')], env=env, capture_output=True, timeout=30)
        b.stop()


if __name__ == '__main__':
    main()
