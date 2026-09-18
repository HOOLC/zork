#!/usr/bin/env python3
"""Bundled CEF + GPUI input + authenticated reverse browser RPC, isolated data."""
import json
import os
import sqlite3
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError
from urllib.request import Request, urlopen
from pathlib import Path
from native_automation import NativeAutomation


class Page(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b'''<!doctype html><meta charset=utf-8><title>Browser fixture</title>
        <style>body{font:16px sans-serif;background:#eef6ff}input{position:absolute;left:20px;top:20px;width:180px;height:30px}button{position:absolute;left:20px;top:70px;width:180px;height:30px}#inspect{position:absolute;left:20px;top:120px;width:200px;height:40px;background:#cdf}#result{position:absolute;top:190px}</style>
        <input id=input oninput="document.querySelector('#typed').textContent=this.value">
        <button id=button onclick="document.querySelector('#result').textContent='clicked'">Click me</button>
        <div id=inspect>Selected element</div><p id=result>ready</p><p id=typed></p>'''
        self.send_response(200)
        self.send_header('Content-Type', 'text/html; charset=utf-8')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


class BrowserTest(NativeAutomation):

    @classmethod
    def page_ready(cls):
        page = cls.element('browser-page')
        return page and any(text in page['label'] for text in ('已显示', 'displayed'))

    @classmethod
    def wait_until(cls, predicate, description, timeout=15):
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            try:
                if predicate():
                    return
            except Exception as error:
                last = error
            time.sleep(0.05)
        if directory := os.environ.get('ZORK_GUI_SCREENSHOT_DIR'):
            path = Path(directory)
            path.mkdir(parents=True, exist_ok=True)
            try:
                (path / 'failure.png').write_bytes(cls.ui('/v1/screenshot'))
                (path / 'failure-elements.json').write_bytes(cls.ui('/v1/elements'))
                cls.gui_log.flush()
                (path / 'failure-gui.log').write_bytes(Path(cls.gui_log.name).read_bytes())
            except Exception:
                pass
        raise AssertionError(f'Timed out waiting for {description}: {last}')


    def tool(self, session, action, request_id):
        client_id = 'missing'
        for path in self.root.rglob('browser.sqlite'):
            with sqlite3.connect(path) as db:
                row = db.execute("SELECT client FROM browser_clients WHERE session='clients' ORDER BY generation DESC LIMIT 1").fetchone()
                if row: client_id = row[0]
        body = {'session_id': session, 'client_id': 'local/' + client_id,
                'command': {'request_id': request_id, 'action': action}}
        request = Request(self.station_url + '/v1/browser/command',
                          data=json.dumps(body).encode(), headers={'Content-Type': 'application/json'})
        try:
            response = urlopen(request, timeout=25)
        except HTTPError as error:
            response = error
        with response:
            return response.status, json.load(response)

    def test_browser(self):
        server = ThreadingHTTPServer(('127.0.0.1', 0), Page)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        self.addCleanup(server.server_close)
        self.addCleanup(server.shutdown)
        url = f'http://127.0.0.1:{server.server_port}/'
        session = self.prepare_session()
        self.exercise_browser(url, session)


    def exercise_browser(self, url, session):
        self.click('conversation-browser')
        self.wait_until(lambda: self.element('browser-address'), 'address field')
        self.click('browser-address')
        self.ui('/v1/actions', {'type': 'type_text', 'text': url})
        self.ui('/v1/actions', {'type': 'key', 'keystroke': 'enter'})
        self.wait_until(self.page_ready, 'real browser frame', 30)
        self.screenshot('browser-page.png')
        # No browser capability before a user grants this conversation.
        self.assertIn('error', self.tool(session, {'op': 'list'}, 'no-grant')[1])
        self.click('browser-agent-grant')
        def tabs():
            return self.tool(session, {'op': 'list'}, 'list-' + os.urandom(8).hex())[1].get('tabs')
        self.wait_until(tabs, 'browser grant connected', 20)
        tab = tabs()[0]['id']
        self.assertEqual(self.tool(session, {'op': 'read', 'tab_id': tab}, 'read')[1]['page']['title'], 'Browser fixture')
        action = {'op': 'open', 'url': url + 'second'}
        opened = self.tool(session, action, 'open-once')
        self.assertEqual(opened, self.tool(session, action, 'open-once'))
        self.assertEqual(len(tabs()), 2)
        self.assertEqual(self.tool(session, {'op': 'open', 'url': url + 'different'}, 'open-once')[0], 400)
        self.tool(session, {'op': 'close', 'tab_id': opened[1]['tab']['id']}, 'close-second')
        self.click('browser-tab-' + tab)
        self.wait_until(self.page_ready, 'restored tab frame')
        page = self.tool(session, {'op': 'read', 'tab_id': tab}, 'dimensions')[1]['page']
        bounds = self.element('browser-page')['bounds']
        # Browser content is fit inside the native panel; map CSS to its image.
        width, height = page['viewport']['width'], page['viewport']['height']
        scale = min(bounds['width'] / width, bounds['height'] / height)
        left = bounds['x'] + (bounds['width'] - width * scale) / 2
        top = bounds['y'] + (bounds['height'] - height * scale) / 2
        def click_page(x, y):
            self.ui('/v1/actions', {'type': 'click', 'target': {'x': left + x * scale, 'y': top + y * scale}})
        click_page(60, 35)
        self.ui('/v1/actions', {'type': 'type_text', 'text': '中文 browser'})
        self.wait_until(lambda: '中文 browser' in self.tool(session, {'op': 'read', 'tab_id': tab}, 'typed-' + os.urandom(8).hex())[1]['page']['text'], 'native input delivered')
        click_page(60, 85)
        self.wait_until(lambda: 'clicked' in self.tool(session, {'op': 'read', 'tab_id': tab}, 'clicked-' + os.urandom(8).hex())[1]['page']['text'], 'native click delivered')
        self.click('browser-inspect')
        click_page(70, 140)
        self.wait_until(lambda: self.element('send-button', enabled=True), 'selection in composer')
        self.click('send-button')
        self.wait_until(lambda: any('Selected element' in json.dumps(message, ensure_ascii=False) and '#inspect' in json.dumps(message) for message in self.messages(session)), 'selected element delivered with message')
        self.screenshot('browser-inspection.png')
        self.click('conversation-browser')
        self.wait_until(lambda: not self.element('browser-page'), 'browser hidden')
        self.click('conversation-browser')
        self.wait_until(self.page_ready, 'browser restored')
        self.assertEqual(self.tool(session, {'op': 'read', 'tab_id': tab}, 'restored')[1]['page']['title'], 'Browser fixture')
        self.click('browser-agent-grant')
        self.wait_until(lambda: 'error' in self.tool(session, {'op': 'list'}, 'revoked-' + os.urandom(8).hex())[1], 'browser grant revoked', 25)
