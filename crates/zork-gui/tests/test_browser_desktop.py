#!/usr/bin/env python3
"""Current DesktopRoot + bundled CEF + remote Leader over real local Mesh."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError
from urllib.request import Request, urlopen

from test_browser import BrowserTest

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location('mesh_fixture', ROOT / 'scripts/test-mesh.py')
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


class DesktopBrowserTest(BrowserTest, unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        if os.environ.get('ZORK_BROWSER_TEST_PROFILE') and os.environ.get('ZORK_GUI_TEST_DRIVE_FRAMES') == '1':
            raise RuntimeError('Native frame measurements require the platform frame loop; disable test frame driving')
        cls.root = Path(tempfile.mkdtemp(prefix='zork-browser-desktop-'))
        cls.node = fixture.Node(cls.root / 'leader')
        cls.node.config['admin'] = {'token': 'browser-desktop-fixture'}
        client = cls.root / 'client'
        client.mkdir()
        transport = fixture.Node(client / 'transport')
        (transport.root / 'config.json').write_text(json.dumps({
            'mesh': {'offline': True, 'bind': f'127.0.0.1:{transport.udp}'}}))
        cls.node.config['mesh']['peers'].append({
            'origin': transport.origin, 'name': 'Browser client',
            'addr': f'127.0.0.1:{transport.udp}', 'client': True, 'execute': []})
        (cls.node.root / 'config.json').write_text(json.dumps(cls.node.config))
        cls.node.start()
        cls.station_url = cls.node.url
        cls.gui = None
        cls.gui_log = (cls.root / 'gui.log').open('wb')
        cls.ui_url = f'http://127.0.0.1:{fixture.port()}'
        cls.gui_env = dict(os.environ, ZORK_CLIENT_DATA=str(client),
                           ZORK_GUI_PREFERENCES_PATH=str(cls.root / 'preferences.json'),
                           ZORK_GUI_TEST_WINDOW_SIZE=os.environ.get('ZORK_GUI_TEST_WINDOW_SIZE', '1280x800'),
                           ZORK_NODE_BINARY=str(fixture.TARGET / 'zork'))
        cls.gui_env.pop('ZORK_GUI_LOCALE', None)
        if locale := os.environ.get('ZORK_BROWSER_TEST_LOCALE'):
            cls.gui_env['ZORK_GUI_LOCALE'] = locale
        if os.environ.get('ZORK_BROWSER_TEST_PROFILE'):
            cls.gui_env['ZORK_FRAME_REPORT'] = str(cls.root / 'browser-frames.json')
        try:
            fixture.wait(lambda: cls.node.request('GET', '/readyz')[0] == 200, 'Leader ready')
            fixture.wait(lambda: urlopen(cls.node.agent_url + '/readyz', timeout=2).status == 200, 'Agent ready')
            status, leader = cls.request('POST', '/v1/node/agents', {
                'id': 'leader', 'name': '研究助手', 'role': 'leader',
                'profile_id': 'fixture', 'model': 'fixture-model', 'thinking': 'off'})
            assert status in (200, 201), (status, leader)
            status, opened = cls.request('POST', '/v1/node/agents/leader/open', {})
            assert status == 200, (status, opened)
            cls.session = opened['session_id']
            cls.gui = subprocess.Popen([
                str(fixture.TARGET / 'zork-gui'), '--dev', '--dev-port',
                cls.ui_url.rsplit(':', 1)[1], '--dev-token', 'zork-shell-fixture'],
                env=cls.gui_env, stdout=cls.gui_log, stderr=cls.gui_log)
            cls.wait_until(lambda: cls.element('connect-existing-node', enabled=True), 'current desktop', 30)
            cls.click('connect-existing-node')
            cls.wait_until(lambda: cls.element('remote-name'), 'pair node')
            for field, value in [('remote-name', '浏览器测试节点'), ('remote-origin', cls.node.origin),
                                 ('remote-addr', f'127.0.0.1:{cls.node.udp}')]:
                cls.click(field)
                cls.ui('/v1/actions', {'type': 'type_text', 'text': value})
            cls.click('connect-remote')
            cls.leader_id = 'leader-' + cls.node.origin + '-leader'
            cls.wait_until(lambda: cls.element(cls.leader_id), 'remote Leader', 40)
            cls.click(cls.leader_id)
            cls.ui('/v1/actions', {'type': 'move', 'target': {'x': 650, 'y': 350}})
            last_open = time.monotonic()
            def conversation_ready():
                nonlocal last_open
                if cls.element('composer-input'):
                    return True
                # The transport may connect before the first message fetch.
                # Reopen through the same user action offered by its error UI.
                if time.monotonic() - last_open > 3:
                    cls.click(cls.leader_id)
                    last_open = time.monotonic()
                return False
            cls.wait_until(conversation_ready, 'current conversation', 60)
        except Exception:
            cls.tearDownClass()
            raise

    @classmethod
    def request(cls, method, path, body=None):
        request = Request(cls.node.url + path, method=method,
                          data=None if body is None else json.dumps(body).encode(),
                          headers={'Content-Type': 'application/json',
                                   'Authorization': 'Bearer browser-desktop-fixture'})
        try:
            response = urlopen(request, timeout=25)
        except HTTPError as error:
            response = error
        with response:
            return response.status, json.load(response)

    def prepare_session(self):
        self.assertIsNotNone(self.element('composer-input'))
        self.assertEqual(list((self.root / 'client/node/run').glob('*.pid')), [])
        return self.session

    def test_browser_panel_motion(self):
        from io import BytesIO
        from PIL import Image

        loads, states = [], []
        if directory := os.environ.get('ZORK_GUI_SCREENSHOT_DIR'):
            Path(directory).mkdir(parents=True, exist_ok=True)

        class MotionPage(BaseHTTPRequestHandler):
            def do_GET(handler):
                if handler.path != '/':
                    handler.send_error(404)
                    return
                loads.append(handler.path)
                body = b'''<!doctype html><title>Panel motion fixture</title>
                <style>html,body{margin:0;height:100%;overflow:hidden}body{font:24px sans-serif}</style>
                <h1>Retained browser document</h1><p>Resize without reloading.</p>
                <script>
                const marker = Math.random().toString(36);
                let hidden = 0;
                function report() {
                  // The pixels encode the document's layout width, so a retained
                  // or scaled old frame cannot pass as a newly reflowed page.
                  document.body.style.backgroundColor =
                    `rgb(${innerWidth >> 8},${innerWidth & 255},77)`;
                  fetch('/state', {method:'POST', body:JSON.stringify({
                  marker, hidden, width:innerWidth
                })}); }
                addEventListener('resize', report);
                document.addEventListener('visibilitychange', () => {
                  hidden += Number(document.hidden); report();
                });
                report();
                </script>'''
                handler.send_response(200)
                handler.send_header('Content-Type', 'text/html')
                handler.send_header('Content-Length', str(len(body)))
                handler.end_headers()
                handler.wfile.write(body)

            def do_POST(handler):
                states.append(json.loads(handler.rfile.read(int(handler.headers['Content-Length']))))
                handler.send_response(204)
                handler.end_headers()

            def log_message(self, *_):
                pass

        server = ThreadingHTTPServer(('127.0.0.1', 0), MotionPage)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        self.click('conversation-browser')
        self.wait_until(lambda: self.element('browser-address'), 'panel address')
        self.click('browser-address')
        self.ui('/v1/actions', {'type': 'type_text', 'text': f'http://127.0.0.1:{server.server_port}/'})
        self.ui('/v1/actions', {'type': 'key', 'keystroke': 'enter'})
        self.wait_until(self.page_ready, 'first browser frame', 30)
        self.wait_until(lambda: states, 'browser document state')
        original = self.element('browser-page')['bounds']['width']
        marker = states[-1]['marker']
        viewport_width = json.loads(self.ui('/v1/elements'))['viewport']['width']

        def painted_width(page, name=None):
            pixels = self.ui('/v1/screenshot')
            image = Image.open(BytesIO(pixels)).convert('RGB')
            scale = image.width / viewport_width
            bounds = page['bounds']
            color = image.getpixel((round((bounds['x'] + bounds['width'] / 2) * scale),
                                    round((bounds['y'] + bounds['height'] / 2) * scale)))
            edge = image.getpixel((image.width - round(4 * scale),
                                  round((bounds['y'] + bounds['height'] / 2) * scale)))
            if directory := os.environ.get('ZORK_GUI_SCREENSHOT_DIR'):
                if name:
                    (Path(directory) / f'{name}.png').write_bytes(pixels)
            return (color[0] << 8) | color[1] if color[2] == 77 and edge == color else None

        self.wait_until(lambda: painted_width(self.element('browser-page')) == original,
                        'first painted layout width')
        traces = []
        for name in ('expand', 'restore', 'expand-repeat', 'restore-repeat'):
            start = self.element('browser-page')['bounds']['width']
            self.click('browser-expand')
            samples = []
            painted = []
            started = time.monotonic()
            until = started + .5
            capture_at = .20 if name.endswith('repeat') else .12
            while time.monotonic() < until:
                page = self.element('browser-page')
                self.assertTrue(page and any(text in page['label'] for text in ('已显示', 'displayed')),
                                f'{name}: browser frame was cleared')
                samples.append(page['bounds']['width'])
                # PNG encoding blocks the native UI thread. Take one mid-motion
                # screenshot per run; repeated captures would stall the motion
                # we are trying to measure. The repeat samples a later point.
                if not painted and time.monotonic() - started >= capture_at:
                    painted.append(painted_width(page, f'{name}-intermediate'))
                time.sleep(.01)
            target = samples[-1]
            self.wait_until(lambda: abs(states[-1]['width'] - target) < 2, 'CEF viewport caught up')
            if name.startswith('restore'):
                self.assertAlmostEqual(target, original, delta=2)
            else:
                self.assertGreater(target, original + 100)
            self.assertEqual(len(loads), 1, 'panel toggle reloaded the document')
            self.assertEqual({state['marker'] for state in states}, {marker})
            self.assertEqual(max(state['hidden'] for state in states), 0,
                             'resizing hid the browser document')
            self.screenshot(f'panel-{name}.png')
            trace = {'case': name, 'page_widths': samples, 'painted_widths': painted,
                     'document_widths': [state['width'] for state in states]}
            traces.append(trace)
            if directory := os.environ.get('ZORK_GUI_SCREENSHOT_DIR'):
                (Path(directory) / f'{name}-frames.json').write_text(json.dumps(trace, indent=2))
            low, high = sorted((start, target))
            intermediate = {width for width in painted if width and low + 2 < width < high - 2}
            self.assertTrue(intermediate,
                            f'{name}: webpage did not repaint at an intermediate width: {painted}')
            self.wait_until(lambda: painted_width(self.element('browser-page')) == target,
                            f'{name}: final painted layout width')
        self.click('browser-expand')
        reversing = time.monotonic() + .05
        while time.monotonic() < reversing:
            self.element('browser-page')
            time.sleep(.005)
        self.click('browser-expand')
        self.wait_until(lambda: abs(self.element('browser-page')['bounds']['width'] - original) < 1,
                        'reversed panel settled')
        self.wait_until(lambda: painted_width(self.element('browser-page')) == original,
                        'reversed browser paint settled')
        self.assertEqual(len(loads), 1)
        self.assertEqual(max(state['hidden'] for state in states), 0)
        if directory := os.environ.get('ZORK_GUI_SCREENSHOT_DIR'):
            (Path(directory) / 'panel-motion.json').write_text(json.dumps({
                'loads': len(loads), 'states': states, 'cases': traces}, indent=2))
        self.click('conversation-browser')
        self.wait_until(lambda: not self.element('browser-page'), 'panel hidden after motion test')

    def test_browser_chrome_states(self):
        class Preview(BaseHTTPRequestHandler):
            def do_GET(handler):
                if handler.path == '/scroll':
                    body = ('<!doctype html><title>Scroll fixture</title><style>body{margin:24px;font:15px sans-serif}p{padding:12px;border-bottom:1px solid #ddd}</style>' + ''.join(f'<p>Browser scroll sample {i} · 中文网页内容</p>' for i in range(1000))).encode()
                else:
                    body = Path(__file__).with_name('fixtures').joinpath('browser-preview.html').read_bytes()
                handler.send_response(200)
                handler.send_header('Content-Type', 'text/html; charset=utf-8')
                handler.send_header('Content-Length', str(len(body)))
                handler.end_headers()
                handler.wfile.write(body)
            def log_message(self, *_):
                pass
        server = ThreadingHTTPServer(('127.0.0.1', 0), Preview)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        status, preview = self.request('POST', '/v1/node/agents', {
            'id': 'preview', 'name': '浏览器助手', 'role': 'leader',
            'profile_id': 'fixture', 'model': 'fixture-model', 'thinking': 'off'})
        self.assertIn(status, (200, 201))
        status, opened = self.request('POST', '/v1/node/agents/preview/open', {})
        self.assertEqual(status, 200, opened)
        self.session = opened['session_id']
        preview_id = 'leader-' + self.node.origin + '-preview'
        self.wait_until(lambda: self.element(preview_id), 'preview Leader')
        self.click(preview_id)
        self.wait_until(lambda: self.element('composer-input'), 'preview conversation')
        if not self.element('browser-address'):
            self.click('conversation-browser')
        self.click('browser-new-tab')
        self.wait_until(lambda: not self.element('browser-back')['enabled'], 'empty controls disabled')
        self.screenshot('browser-empty.png')
        self.click('browser-address')
        self.ui('/v1/actions', {'type': 'type_text', 'text': 'javascript:void(0)'})
        self.ui('/v1/actions', {'type': 'key', 'keystroke': 'enter'})
        self.wait_until(lambda: self.element('browser-error'), 'invalid URL feedback')
        self.screenshot('browser-error.png')
        self.click('browser-address')
        self.ui('/v1/actions', {'type': 'key', 'keystroke': 'cmd-a'})
        self.ui('/v1/actions', {'type': 'type_text', 'text': f'http://127.0.0.1:{server.server_port}/guide'})
        self.click('browser-go')
        self.wait_until(self.page_ready, 'guide ready')
        self.assertIsNone(self.element('browser-error'))
        if 'tabs' not in self.tool(self.session, {'op': 'list'}, 'guide-grant-check')[1]:
            self.click('browser-agent-grant')
        def guide():
            response = self.tool(self.session, {'op': 'list'}, 'guide-' + os.urandom(6).hex())[1]
            return next((t for t in response.get('tabs', []) if '/guide' in t['url']), None)
        self.wait_until(guide, 'guide grant connected')
        tab = guide()['id']
        self.wait_until(lambda: self.tool(self.session, {'op': 'read', 'tab_id': tab},
                                        'guide-read-' + os.urandom(6).hex())[1].get('page', {}).get('title') == '网页协作指南', 'guide content loaded')
        self.ui('/v1/actions', {'type': 'move', 'target': {'x': 650, 'y': 350}})
        self.screenshot('browser-workspace.png')
        self.exercise_page_workspace(tab)
        original_width = self.element('browser-page')['bounds']['width']
        self.click('browser-expand')
        self.wait_until(lambda: self.element('browser-page')['bounds']['width'] > original_width + 100, 'browser expanded')
        self.wait_until(self.page_ready, 'expanded frame ready')
        self.screenshot('browser-expanded.png')
        self.click('browser-expand')
        self.wait_until(lambda: abs(self.element('browser-page')['bounds']['width'] - original_width) < 2, 'split view restored')
        self.wait_until(self.page_ready, 'restored frame ready')
        self.click('browser-inspect')
        self.wait_until(lambda: self.element('browser-inspection-hint'), 'selection guidance')
        self.screenshot('browser-selecting.png')
        self.ui('/v1/actions', {'type': 'key', 'keystroke': 'escape'})
        self.wait_until(lambda: not self.element('browser-inspection-hint'), 'escape cancels inspection')
        page = self.element('browser-page')['bounds']
        composer = self.element('composer-input')['bounds']
        self.assertLessEqual(composer['x'] + composer['width'], page['x'])
        for control in ('browser-back', 'browser-forward', 'browser-reload', 'browser-address',
                        'browser-more', 'browser-downloads', 'browser-expand', 'conversation-browser'):
            element = self.element(control)
            self.assertGreaterEqual(element['visible_bounds']['width'], element['bounds']['width'] - 1, control)
            self.assertGreaterEqual(element['visible_bounds']['height'], element['bounds']['height'] - 1, control)
        self.click('browser-more')
        self.screenshot('browser-menu.png')
        self.ui('/v1/actions', {'type': 'key', 'keystroke': 'escape'})
        self.wait_until(lambda: not self.element('browser-menu'), 'escape closes menu')
        if os.environ.get('ZORK_BROWSER_TEST_PROFILE'):
            self.tool(self.session, {'op': 'navigate', 'tab_id': tab, 'url': f'http://127.0.0.1:{server.server_port}/scroll'}, 'scroll-navigation')
            self.wait_until(lambda: self.tool(self.session, {'op': 'read', 'tab_id': tab}, 'scroll-read-' + os.urandom(6).hex())[1].get('page', {}).get('title') == 'Scroll fixture', 'scroll fixture ready')
            self.wait_until(self.page_ready, 'scroll frame')
            report = self.root / 'browser-frames.json'
            self.ui('/v1/actions', {'type': 'scroll_measure', 'target': self.element('browser-page')['center'], 'duration_ms': 6000, 'pixels_per_second': 480, 'start_down': True})
            self.wait_until(report.exists, 'native browser frame measurement', 12)
            measurement = json.loads(report.read_text())
            output = Path(os.environ['ZORK_GUI_SCREENSHOT_DIR'])
            (output / 'browser-frames.json').write_text(json.dumps(measurement, indent=2))
            self.assertGreater(measurement['draws'], 30)
            self.assertGreater(measurement['input_events'], 60)
            self.assertLessEqual(measurement['p95_draw_ms'], 1000 / 120)
            print('Browser native draw p95/p99:', measurement['p95_draw_ms'], measurement['p99_draw_ms'], flush=True)
        self.click('browser-agent-grant')
        self.click('browser-close-' + tab)
        self.wait_until(lambda: not self.element('browser-back')['enabled'], 'last tab closed')
        self.wait_until(lambda: self.element('browser-close-blank') and not self.element('browser-error'), 'clean new tab after closing the last page')
        self.screenshot('browser-empty.png')
        self.click('browser-expand')
        self.wait_until(lambda: self.element('browser-page')['bounds']['width'] > original_width + 100, 'empty browser expanded')
        self.screenshot('browser-empty-expanded.png')
        self.click('browser-expand')
        self.click('conversation-browser')
        self.wait_until(lambda: not self.element('browser-page'), 'browser minimized')
        self.click('conversation-browser')
        self.wait_until(lambda: self.element('browser-address'), 'browser restored after minimize')

    @classmethod
    def screenshot(cls, name):
        page = cls.element('browser-page')
        menu = cls.element('browser-menu')
        if menu:
            bounds = menu['bounds']
            target = {'x': bounds['x'] + bounds['width'] - 6, 'y': bounds['y'] + bounds['height'] - 6}
        elif page:
            target = page['center']
        else:
            target = {'x': 650, 'y': 350}
        cls.ui('/v1/actions', {'type': 'move', 'target': target})
        if name.startswith('browser-empty'):
            cls.ui('/v1/actions', {'type': 'click', 'target': target})
        time.sleep(.25)
        super().screenshot(name)

    @classmethod
    def element(cls, element_id, *, enabled=False):
        result = super().element(element_id, enabled=enabled)
        if result is None:
            # A capture requests a full native paint, repopulating controls
            # omitted from the registry during a retained-region-only frame.
            cls.ui('/v1/screenshot')
            result = super().element(element_id, enabled=enabled)
        return result

    @classmethod
    def click(cls, element_id):
        if element_id in ('remote-name', 'remote-origin', 'remote-addr', 'connect-remote'):
            def visible_setting():
                element = cls.element(element_id, enabled=True)
                viewport = json.loads(cls.ui('/v1/elements'))['viewport']
                if element and element['bounds']['y'] + element['bounds']['height'] < viewport['height'] - 4:
                    return True
                cls.ui('/v1/actions', {'type': 'scroll', 'target': {'x': viewport['width'] - 40, 'y': viewport['height'] - 80}, 'delta_y': -150})
                return False
            cls.wait_until(visible_setting, element_id + ' visible')
        if element_id in ('browser-agent-grant', 'browser-inspect', 'browser-handoff') and not cls.element(element_id):
            cls.click('browser-more')
        cls.wait_until(lambda: cls.element(element_id, enabled=True), element_id + ' enabled')
        # Resolve at dispatch time: toolbar entries can move between an HTTP
        # geometry read and the click while the conversation updates.
        cls.ui('/v1/actions', {'type': 'click', 'target': {'element_id': element_id}})

    def exercise_page_workspace(self, tab):
        width = self.element('browser-page')['bounds']['width']
        self.click('composer-input')
        self.ui('/v1/actions', {'type': 'type_text', 'text': '验证网页与执行历史共用页面管理。'})
        self.click('send-button')
        self.wait_until(lambda: bool(self.messages(self.session)), 'history fixture input delivered')
        self.ui('/v1/screenshot')
        members = [e for e in json.loads(self.ui('/v1/elements'))['elements']
                   if e['id'].startswith('header-member-') and e['visible']]
        self.assertTrue(members, 'conversation participant entry')
        self.click(members[0]['id'])
        self.wait_until(lambda: self.element('page-tab-history') and self.element('history-page'), 'history in shared page workspace')
        self.assertIsNone(self.element('browser-address'))
        self.assertIsNone(self.element('browser-page'))
        history = self.element('history-page')['bounds']
        composer = self.element('composer-input')['bounds']
        self.assertLessEqual(composer['x'] + composer['width'], history['x'])
        self.assertLess(abs(history['width'] - width), 3)
        self.wait_until(lambda: any(e['id'].startswith('history-record') for e in json.loads(self.ui('/v1/elements'))['elements']), 'history records loaded')
        self.screenshot('workspace-history.png')
        self.ui('/v1/actions', {'type': 'drag', 'from': {'x': history['x'], 'y': history['y']+100},
                                'to': {'x': history['x']-24, 'y': history['y']+100}})
        self.wait_until(lambda: abs(self.element('history-page')['bounds']['width'] - (history['width'] + 24)) < 1, 'shared page width settled')
        width = self.element('history-page')['bounds']['width']
        self.click('browser-tab-' + tab)
        self.wait_until(self.page_ready, 'web page restored after history')
        self.assertIsNone(self.element('history-page'))
        self.wait_until(lambda: abs(self.element('browser-page')['bounds']['width'] - width) < 3, 'web page retains settled shared width')
        self.screenshot('workspace-browser-tabs.png')
        self.click('page-tab-history')
        self.click('conversation-browser')
        self.wait_until(lambda: not self.element('history-page'), 'shared panel hidden')
        self.click('conversation-browser')
        self.wait_until(lambda: self.element('history-page'), 'history restored with panel')
        self.click('browser-expand')
        self.wait_until(lambda: self.element('history-page')['bounds']['width'] > width + 100, 'history uses shared expand')
        self.screenshot('workspace-history-expanded.png')
        self.click('browser-expand')
        self.click('page-close-history')
        self.wait_until(self.page_ready, 'closing history returns to webpage')
        self.assertIsNone(self.element('page-tab-history'))

    @classmethod
    def tearDownClass(cls):
        if cls.gui:
            cls.stop_process(cls.gui)
        profile = str((cls.root / 'client/browser/cef').resolve())
        def browser_processes():
            return [int(line.strip().split(None, 1)[0])
                    for line in subprocess.check_output(['ps', '-axo', 'pid,args'], text=True).splitlines()
                    if profile in line and 'ZorkBrowser' in line]
        deadline = time.monotonic() + 12
        remaining = browser_processes()
        while remaining and time.monotonic() < deadline:
            time.sleep(.1)
            remaining = browser_processes()
        for pid in remaining:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        cls.gui_log.close()
        cls.node.stop()
        print('Desktop browser evidence:', cls.root)
        assert not remaining, f'Browser processes survived desktop shutdown: {remaining}'


if __name__ == '__main__':
    os.environ.setdefault('ZORK_GUI_SCREENSHOT_DIR', str(ROOT / 'artifacts/browser/desktop'))
    result = unittest.TextTestRunner(verbosity=2).run(
        unittest.TestSuite([DesktopBrowserTest('test_browser_panel_motion'),
                           DesktopBrowserTest('test_browser'), DesktopBrowserTest('test_browser_chrome_states')]))
    raise SystemExit(not result.wasSuccessful())
