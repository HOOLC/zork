#!/usr/bin/env python3
"""Approved native GUI against isolated real Stations and fake model runtimes.

Covers scoped drafts/comments, a single explicit send, ownership-preserving Task
comments, device folds, full-width/resizable navigation, settings and reconnect.
Never starts the user's local node or generates an enrollment invitation.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import time
import sys
from urllib.request import Request, urlopen

spec = importlib.util.spec_from_file_location('ui', Path(__file__).parent / 'lib/native_gui_fixture.py')
ui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ui)
f = ui.fixture
TARGET_ROOT = Path(os.environ.get('CARGO_TARGET_DIR', f.ROOT / 'target'))
f.TARGET = TARGET_ROOT / 'debug'



def capture_brand_motion():
    """Verify actual native frames; no model calls or user preference mutations."""
    root = Path(tempfile.mkdtemp(prefix='zork-brand-gui-', dir='/tmp'))
    client = root / 'client'
    client.mkdir()
    node = f.Node(root / 'node')
    node.config['mesh']['enabled'] = False
    node.config['admin'] = {'token': 'sidebar-fixture'}
    (node.root / 'config.json').write_text(json.dumps(node.config))
    native = ui.Native(None, root)
    reduced = os.environ.get('ZORK_GUI_TEST_REDUCE_MOTION') == '1'
    art = Path(os.environ.get('ZORK_GUI_SCREENSHOT_DIR', f.ROOT / 'artifacts/gui-approved-design')) / ('brand-reduced' if reduced else 'brand-motion')
    art.mkdir(parents=True, exist_ok=True)
    evidence = {}
    morph_asset = f.ROOT / 'crates/zork-gui/assets/motion/morph-points.json'
    morph_bytes = morph_asset.read_bytes()
    morph_data = json.loads(morph_bytes)

    def request(method, path, body=None):
        return json.load(urlopen(Request(node.url + path, method=method,
            data=None if body is None else json.dumps(body).encode(),
            headers={'Content-Type': 'application/json', 'Authorization': 'Bearer sidebar-fixture'})))

    def ready(id):
        return f.wait(lambda: native.element(id, True), id)

    def click(id):
        ready(id)
        native.click(id)

    def frame(id, phase):
        # The normal screenshot helper waits for a new frame, which can skip a
        # short decorative motion. Sample the current GPU frame directly here.
        pixels = native.ui('/v1/screenshot')
        (art / f'{id}-{phase}.png').write_bytes(pixels)
        return pixels

    def motion(id, duration):
        time.sleep(1.5)  # Finish incidental hover caused by entering this page.
        native.ui('/v1/actions', {'type': 'move', 'target': {'x': 890, 'y': 40}})
        time.sleep(.1)
        frames = {'start': frame(id, 'start')}
        ready(id)
        native.ui('/v1/actions', {'type': 'move', 'target': {'element_id': id}})
        started = time.monotonic()
        schedule = ([('early', duration * .16), ('mid', duration * .4), ('unfold', duration * .575), ('late', duration * .65), ('settle', duration * .9)]
            if id == 'brand-morph' else [('early', .12), ('mid', .32), ('late', .52)])
        for phase, at in schedule + [('end', duration + 1)]:
            time.sleep(max(0, at - (time.monotonic() - started)))
            frames[phase] = frame(id, phase)
        if reduced:
            assert all(frames[phase] == frames['end'] for phase, _ in schedule), (id, 'reduced motion changed frames')
        else:
            assert any(frames[phase] != frames['end'] for phase, _ in schedule), (id, 'motion never progressed')
        if id == 'brand-morph':
            assert frames['start'] != frames['end'], 'morph never reached the wordmark endpoint'
        else:
            assert frames['start'] == frames['end'], (id, 'motion did not return to its rest state')
        evidence[id] = {phase: hashlib.sha256(data).hexdigest() for phase, data in frames.items()}

    try:
        node.start()
        f.wait(lambda: node.request('GET', '/readyz')[0] == 200, 'Station')
        f.wait(lambda: urlopen(node.agent_url + '/readyz').status == 200, 'Agent')
        with sqlite3.connect(client / 'client.db') as db:
            db.executescript('CREATE TABLE nodes(id TEXT PRIMARY KEY,value TEXT NOT NULL);'
                'CREATE TABLE cache(node TEXT,key TEXT,value TEXT,PRIMARY KEY(node,key));')
            db.execute('INSERT INTO nodes VALUES (?,?)', ('node-0', json.dumps(dict(id='node-0',
                name='mini1', url=node.url, token='sidebar-fixture', local=False))))
            db.execute('INSERT INTO cache VALUES (?,?,?)', ('device', 'local-node-enabled', 'false'))
        env = dict(os.environ, ZORK_CLIENT_DATA=str(client), ZORK_GUI_LOCALE='zh-CN',
            ZORK_GUI_PREFERENCES_PATH=str(root / 'preferences.json'), ZORK_GUI_TEST_WINDOW_SIZE='900x600')
        native.process = subprocess.Popen([str(f.TARGET / 'zork-gui'), '--dev', '--dev-port',
            native.url.rsplit(':', 1)[1], '--dev-token', 'mesh-native-fixture'], env=env,
            stdout=native.log, stderr=native.log)
        f.wait(lambda: native.ui('/health'), 'GUI')
        click('settings-device-node-0')
        click('desktop-return')
        ready('brand-morph')
        motion('brand-morph', morph_data['durationMs'] / 1000)
        request('POST', '/v1/node/agents', dict(id='leader', name='产品 Leader', role='leader',
            avatar='fox', profile_id='fixture', model='fixture-model', thinking='off'))
        ready('brand-icon')
        motion('brand-icon', .680)
        motion('brand-linked', .805)
        click('desktop-manage')
        ready('brand-wordmark')
        motion('brand-wordmark', .675)
        (art / 'frames.json').write_text(json.dumps({'reduced_motion': reduced, 'geometry_mode': morph_data['mode'],
            'duration_ms': morph_data['durationMs'], 'geometry_sha256': hashlib.sha256(morph_bytes).hexdigest(),
            'sha256': evidence}, indent=2) + '\n')
        print(f'PASS native four-motion pixel checks (reduced_motion={reduced})', flush=True)
    finally:
        native.stop()
        native.log.close()
        node.stop()
        print(root, flush=True)

def main():
    root = Path(tempfile.mkdtemp(prefix='zork-approved-gui-', dir='/tmp'))
    client = root / 'client'
    client.mkdir()
    nodes = [f.Node(root / name) for name in ('mini1', 'mini2')]
    native = ui.Native(None, root)
    art = Path(os.environ.get('ZORK_GUI_SCREENSHOT_DIR', f.ROOT / 'artifacts/gui-approved-design'))
    art.mkdir(parents=True, exist_ok=True)
    size = os.environ.get('ZORK_GUI_TEST_WINDOW_SIZE', '1280x800')
    for node in nodes:
        node.config['mesh']['enabled'] = False
        node.config['admin'] = {'token': 'sidebar-fixture'}
        (node.root / 'config.json').write_text(json.dumps(node.config))

    def request(node, method, path, body=None, extra=None):
        return json.load(urlopen(Request(node.url + path, method=method,
            data=None if body is None else json.dumps(body).encode(),
            headers={'Content-Type': 'application/json', 'Authorization': 'Bearer sidebar-fixture', **(extra or {})}), timeout=15))

    def ready(id):
        def lookup():
            element = native.element(id, True)
            if element:
                return element
            snapshot = json.loads(native.ui('/v1/elements?include_hidden=true'))
            hidden = next((e for e in snapshot['elements'] if e['id'] == id and e['enabled']), None)
            if hidden and not hidden['visible']:
                bounds = hidden['bounds']
                width = snapshot['viewport']['width']
                height = snapshot['viewport']['height']
                native.ui('/v1/actions', {'type': 'scroll', 'target': {'x': min(max(bounds['x'] + 10, 10), width - 20), 'y': height / 2}, 'delta_y': 160 if bounds['y'] < 0 else -160})
            return None
        return f.wait(lookup, id)

    def click(id):
        ready(id)
        native.click(id)

    def key(keystroke):
        native.ui('/v1/actions', {'type': 'key', 'keystroke': keystroke})

    def fill(id, text):
        click(id)
        key('cmd-a')
        if text:
            native.type(text)
        else:
            key('backspace')

    def capture(name):
        native.screenshot(art / f'{name}-{size}.png')
        (art / f'{name}-{size}.json').write_bytes(native.ui('/v1/elements?include_hidden=true'))

    def cache(node, key):
        with sqlite3.connect(client / 'client.db') as db:
            row = db.execute('SELECT value FROM cache WHERE node=? AND key=?', (node, key)).fetchone()
            return json.loads(row[0]) if row else None

    def messages(node, session):
        return request(node, 'GET', f'/v1/im/sessions/{session}/messages')['items']

    def deliver(node, session, text):
        context = request(node, 'GET', '/v1/tools/context?threadId=' + session)
        request(node, 'POST', '/chat/post-message', {
            'sessionKey': context['sessionKey'], 'conversationId': context['conversationId'],
            'rootMessageId': context['rootMessageId'], 'kind': 'final', 'text': text,
        })

    def start_gui():
        env = dict(os.environ, ZORK_CLIENT_DATA=str(client), ZORK_GUI_LOCALE='zh-CN',
                   ZORK_GUI_PREFERENCES_PATH=str(root / 'preferences.json'), ZORK_GUI_TEST_WINDOW_SIZE=size)
        native.process = subprocess.Popen([str(f.TARGET / 'zork-gui'), '--dev', '--dev-port',
            native.url.rsplit(':', 1)[1], '--dev-token', 'mesh-native-fixture'], env=env,
            stdout=native.log, stderr=native.log)
        f.wait(lambda: native.ui('/health'), 'native window')
        # /health precedes the first frame and asynchronous cached-node restore.
        # Wait for an actual navigation surface before choosing its branch.
        f.wait(lambda: native.element('leader-node-0-leader') or
               native.element('settings-device-node-0'), 'restored navigation')
        if not native.element('leader-node-0-leader'):
            click('settings-device-node-0')
            click('desktop-return')
        ready('leader-node-0-leader')

    def select_passage(text, start=2, end=130):
        def find():
            return next((e for e in json.loads(native.ui('/v1/elements'))['elements']
                if e['visible'] and e['id'].endswith('-selection') and text in e['label']), None)
        element = f.wait(find, 'selectable message ' + text)
        bounds = element['visible_bounds']
        y = bounds['y'] + min(9, bounds['height'] / 2)
        native.ui('/v1/actions', {'type': 'drag',
            'from': {'x': bounds['x'] + start, 'y': y},
            'to': {'x': bounds['x'] + min(end, bounds['width'] - 2), 'y': y}})
        ready('comment-input')

    def assert_rows(ids):
        rows = [ready(id)['bounds'] for id in ids]
        assert all(abs(row['height'] - 32) < 0.6 for row in rows), rows
        assert max(row['width'] for row in rows) - min(row['width'] for row in rows) < 0.6, rows

    try:
        leaders = []
        for i, node in enumerate(nodes):
            node.start()
            f.wait(lambda: node.request('GET', '/readyz')[0] == 200, 'Station ready')
            f.wait(lambda: urlopen(node.agent_url + '/readyz', timeout=2).status == 200, 'Agent ready')
            selection = dict(profile_id='fixture', model='fixture-model', thinking='off')
            leaders.append(request(node, 'POST', '/v1/node/agents', dict(selection,
                id='leader', name='产品 Leader', role='leader', avatar='fox' if i == 0 else 'panda')))
            request(node, 'POST', '/v1/node/agents/leader/open', {})
            request(node, 'POST', '/v1/node/agents', dict(selection,
                id='worker', name='交付检查', role='worker', avatar='dog', allowed_leaders=['leader']))
        with sqlite3.connect(client / 'client.db') as db:
            db.executescript('CREATE TABLE nodes(id TEXT PRIMARY KEY,value TEXT NOT NULL);'
                'CREATE TABLE cache(node TEXT,key TEXT,value TEXT,PRIMARY KEY(node,key));')
            db.execute('INSERT INTO cache VALUES (?,?,?)', ('device', 'local-node-enabled', 'false'))
            for i, node in enumerate(nodes):
                saved = dict(id=f'node-{i}', name=node.root.name, url=node.url, token='sidebar-fixture', local=False)
                db.execute('INSERT INTO nodes VALUES (?,?)', (saved['id'], json.dumps(saved)))
        a, b = nodes
        task = request(a, 'POST', '/v1/agent/tasks', dict(worker_id='worker', request_id='sidebar-task',
            goal='检查安装说明和交付文件，整理需要补充的内容。'), {'x-zork-session-key': leaders[0]['session_key']})['task']
        first_text = '请先统一**图标与头像**，并保留原生界面的紧凑感。'
        rendered_text = first_text.replace('**', '')
        deliver(a, leaders[0]['session_id'], first_text)
        leader_workspace = next(s['workspace'] for s in request(a, 'GET', '/v1/im/sessions')['items'] if s['session_id'] == leaders[0]['session_id'])
        attachment = Path(leader_workspace) / '交付说明.md'
        attachment.write_text('# 已提交的文件\n\n这是隔离测试中的真实产物。\n')
        context = request(a, 'GET', '/v1/tools/context?threadId=' + leaders[0]['session_id'])
        artifact = request(a, 'POST', '/chat/post-file', dict(sessionKey=context['sessionKey'],
            conversationId=context['conversationId'], rootMessageId=context['rootMessageId'],
            filePath=str(attachment), initialComment='设计交付文件'))['artifact']
        start_gui()
        task_id = f"leader-task-node-0-{task['task_id']}"
        ready(task_id)
        assert_rows(['device-node-0', 'leader-node-0-leader', task_id, 'device-add', 'desktop-manage'])
        native.ui('/v1/actions', {'type': 'move', 'target': {'element_id': 'brand-linked'}})
        capture('brand-linked')
        click('leader-node-0-leader')
        ready('composer-input')
        assert abs(ready('conversation-header')['bounds']['height'] - 44) < .6
        assert not native.element('window-search'), 'legacy global search leaked into native shell'
        assert not native.element('composer-voice'), 'legacy voice placeholder leaked into composer'
        click('conversation-artifact-' + artifact['artifact_id'])
        ready('drive-preview')
        ready('drive-save')
        capture('conversation-file-preview')
        click('drive-close-preview')
        base_height = ready('composer-input')['bounds']['height']
        fill('composer-input', '\n'.join('自动撑开第 %s 行' % i for i in range(8)))
        f.wait(lambda: native.element('composer-input')['bounds']['height'] > base_height + 40, 'composer auto grows')
        fill('composer-input', 'mini1 的草稿只属于 mini1')
        click('leader-node-1-leader')
        fill('composer-input', 'mini2 的草稿只属于 mini2')
        click('leader-node-0-leader')
        for i, leader in enumerate(leaders):
            assert cache(f'node-{i}', 'draft:' + leader['session_id']) == f'mini{i+1} 的草稿只属于 mini{i+1}'
        before_count = len(messages(a, leaders[0]['session_id']))
        select_passage(rendered_text)
        native.type('图标先处理这一段')
        key('enter')
        f.wait(lambda: len(cache('node-0', 'draft-comments:' + leaders[0]['session_id']) or []) == 1, 'first comment queued')
        assert len(messages(a, leaders[0]['session_id'])) == before_count, 'popup Enter sent a message'
        select_passage(rendered_text, 132, 250)
        native.type('头像也保持一致')
        key('enter')
        comments = f.wait(lambda: (items if len(items := cache('node-0', 'draft-comments:' + leaders[0]['session_id']) or []) == 2 else None), 'second comment queued')
        original_id = next(m['id'] for m in messages(a, leaders[0]['session_id']) if m['content'] == first_text)
        assert all(c['source']['message_id'] == original_id and c['source']['quote'] in rendered_text for c in comments), comments
        click('comment-edit-' + comments[0]['id'])
        fill('comment-input', '先调整图标尺寸')
        key('enter')
        f.wait(lambda: cache('node-0', 'draft-comments:' + leaders[0]['session_id'])[0]['comment'] == '先调整图标尺寸', 'comment edit')
        fill('composer-input', '')
        ready('send-button')  # Comments-only submission is enabled without sending yet.
        fill('composer-input', 'mini1 的草稿只属于 mini1')
        click('comment-remove-' + comments[1]['id'])
        f.wait(lambda: len(cache('node-0', 'draft-comments:' + leaders[0]['session_id']) or []) == 1, 'comment removed')
        select_passage(rendered_text, 132, 250)
        native.type('头像也保持一致')
        key('enter')
        f.wait(lambda: len(cache('node-0', 'draft-comments:' + leaders[0]['session_id']) or []) == 2, 'second comment restored')
        click('leader-node-1-leader')
        assert not any(e['id'].startswith('comment-edit-') for e in json.loads(native.ui('/v1/elements'))['elements']), 'comment scope leaked'
        click('leader-node-0-leader')
        ready('comment-edit-' + comments[0]['id'])
        ready('send-button')
        capture('conversation-comments')
        # Persist an explicit pixel width; backgrounds retain their full width.
        old = native.element('device-node-0')['bounds']['width'] + 16
        native.ui('/v1/actions', {'type': 'drag', 'from': {'x': old - 2, 'y': 180}, 'to': {'x': old + 50, 'y': 180}})
        f.wait(lambda: (cache('device', 'sidebar-width') or 0) > old + 30, 'sidebar width persisted')
        assert_rows(['device-node-0', 'leader-node-0-leader', task_id, 'device-add', 'desktop-manage'])
        click('desktop-manage')
        click('client_notifications')
        ready('client_notifications')
        for removed in ('client_diagnostics', 'client_about', 'client-check-connections', 'client-copy-diagnostics', 'client-license'):
            assert not native.element(removed), f'removed client setting remains: {removed}'
        capture('client-settings')
        click('desktop-return')
        ready('composer-input')
        capture('conversation-default')
        click('desktop-manage')
        click('settings-device-node-0')
        assert_rows(['settings-device-node-0', 'settings-node-0-1', 'settings-node-0-0', 'settings-add-device'])
        capture('device-settings')
        click('settings-node-0-1')
        ready('agent-settings-leader')
        capture('agent-settings')
        click('settings-node-0-0')
        click('profile-add')
        ready('profile-access-true')
        assert not native.element('profile-model'), 'connection form still includes preset model editing'
        click('profile-access-false')
        click('profile-provider-select')
        options = json.loads(native.ui('/v1/elements'))['elements']
        compatible = next(e for e in options if e['id'].startswith('provider-option-') and 'compatible' in e['label'].lower())
        click(compatible['id'])
        fill('profile-id', 'manual-ui-connection')
        fill('profile-base-url', 'http://127.0.0.1:9/v1')
        fill('profile-key', 'fixture-only-not-a-real-key')
        capture('add-connection')
        click('profile-save')
        f.wait(lambda: any(p['profile_id'] == 'manual-ui-connection' for p in request(a, 'GET', '/v1/im/profiles')['items']), 'connection saved')
        profile = request(a, 'GET', '/v1/node/profiles/manual-ui-connection')
        assert profile['models'] == [], profile
        click('profile-detail-manual-ui-connection')
        click('profile-model-add')
        fill('profile-model', 'manual-test-model')
        fill('profile-context-limit', '100000')
        fill('profile-output-limit', '10000')
        capture('configure-model')
        click('profile-model-save')
        f.wait(lambda: len(request(a, 'GET', '/v1/node/profiles/manual-ui-connection')['models']) == 1, 'manual model saved')
        click('profile-detail-dialog-close')
        click('settings-node-0-1')
        click('agent-settings-leader')
        ready('agent-avatar-octopus')
        capture('agent-detail')
        def option(prefix, label):
            click('agent-edit-profile-select' if prefix == 'agent-edit-profile-' else 'agent-edit-model-select')
            return f.wait(lambda: next((e for e in json.loads(native.ui('/v1/elements?include_hidden=true'))['elements']
                if e['id'].startswith(prefix) and e['label'] == label), None), label)['id']
        click(option('agent-edit-profile-', 'manual-ui-connection'))
        click(option('agent-edit-model-', 'manual-test-model'))
        click('agent-avatar-save')
        f.wait(lambda: next(x for x in request(a, 'GET', '/v1/node/agents')['items'] if x['id'] == 'leader')['model'] == 'manual-test-model', 'Agent model binding saved')
        click('agent-settings-leader')
        click(option('agent-edit-profile-', 'fixture'))
        click(option('agent-edit-model-', 'fixture-model'))
        click('agent-avatar-save')
        f.wait(lambda: next(x for x in request(a, 'GET', '/v1/node/agents')['items'] if x['id'] == 'leader')['model'] == 'fixture-model', 'Agent model binding restored')
        click('desktop-return')
        click('device-add')
        ready('mesh-client-invite-create')
        assert not native.element('mesh-invite-create'), 'Station command must not appear on phone tab'
        capture('add-device')
        click('mesh-connect-device-tab')
        ready('mesh-invite-create')
        assert not native.element('mesh-client-invite-create'), 'Phone action must not appear on Station tab'
        click('add-device-dialog-close')
        # Device folds remain; task rows stay visible without a fold arrow.
        click('leader-node-0-leader')
        assert not native.element('leader-toggle-node-0/leader')
        ready(task_id)
        click('device-node-1')
        assert not native.element('leader-node-1-leader')
        click(task_id)
        ready('composer-input')  # Task comments are permitted by the owning Station.
        fill('composer-input', '请所属 Leader 检查这个任务的交付范围。')
        click('send-button')
        f.wait(lambda: any(m['content'] == '请所属 Leader 检查这个任务的交付范围。' for m in messages(a, task['session_id'])), 'Task human comment delivered once')
        capture('task-conversation')
        click('leader-node-0-leader')
        # Offline batch delivery remains exactly once and scoped to mini1.
        a.stop()
        f.wait(lambda: '离线' in native.element('device-node-0')['label'], 'mini1 offline', 45)
        ready('device-offline-banner')
        click('conversation-artifact-' + artifact['artifact_id'])
        ready('drive-save')  # Stored bytes remain readable while the owning node is offline.
        click('drive-close-preview')
        click('composer-input')
        key('enter')
        def queued():
            with sqlite3.connect(client / 'client.db') as db:
                return db.execute('SELECT count(*) FROM outbox WHERE node=?', ('node-0',)).fetchone()[0]
        f.wait(lambda: queued() == 1, 'one offline batch queued')
        assert cache('node-0', 'draft-comments:' + leaders[0]['session_id']) == []
        capture('offline-outbox')
        a.start()
        f.wait(lambda: '在线' in native.element('device-node-0')['label'], 'mini1 reconnects', 60)
        f.wait(lambda: queued() == 0, 'outbox delivered', 30)
        batches = [m['content'] for m in messages(a, leaders[0]['session_id']) if m['content'].startswith('<zork-message-comments version="1">')]
        assert len(batches) == 1, batches
        batch = json.loads(batches[0].split('\n', 1)[1].rsplit('\n', 1)[0])
        assert batch['text'] == 'mini1 的草稿只属于 mini1' and len(batch['comments']) == 2, batch
        capture('reconnected')
        saved_width = cache('device', 'sidebar-width')
        native.stop()
        start_gui()
        assert not native.element('leader-node-1-leader'), 'Device collapse not restored'
        assert cache('device', 'sidebar-width') == saved_width
        assert not (client / 'node/config.json').exists() and cache('device', 'local-node-enabled') is False, 'navigation started an unwanted local node'
        capture('restored')
        print(f'PASS ({size}): scoped drafts + 2 queued comments + single send, owner Task chat, settings/manual connection + model, resize/folds, offline/reconnect/restart', flush=True)
    finally:
        if native.process and native.process.poll() is None:
            native.screenshot(root / 'final.png')
        native.stop()
        native.log.close()
        for node in nodes:
            node.stop()
        print(root, flush=True)


if __name__ == '__main__':
    if '--brand-only' in sys.argv:
        capture_brand_motion()
    else:
        main()
