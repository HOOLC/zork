#!/usr/bin/env python3
"""Resource placement and page delivery through real native/core/Gateway contracts."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import quote

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('mcp_fixture', ROOT / 'scripts/test-mcp.py')
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m); f = m.f
spec = importlib.util.spec_from_file_location('native_fixture', ROOT / 'scripts/lib/native_gui_fixture.py')
ui = importlib.util.module_from_spec(spec); spec.loader.exec_module(ui)

class Preview(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == '/redirect':
            self.send_response(302); self.send_header('Location', '/report'); self.end_headers(); return
        body = b'''<!doctype html><meta charset=utf-8><title>Resource report</title><style>body{font:16px sans-serif;padding:20px}input{display:block;padding:10px}</style><h1>Report</h1><input id=note oninput="document.querySelector('#saved').textContent=this.value"><p id=saved></p>'''
        self.send_response(200); self.send_header('Content-Type', 'text/html; charset=utf-8')
        self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, *_): pass

def main():
    root = Path(tempfile.mkdtemp(prefix='zork-resources-ui-'))
    client = root / 'client'; client.mkdir()
    art = ROOT / 'artifacts/resource-native-implementation/native'; art.mkdir(parents=True, exist_ok=True)
    native = ui.Native(None, root); nodes = []; success = False; checks = []
    server = ThreadingHTTPServer(('127.0.0.1', 0), Preview)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    def request(node, method, path, body=None, admin=False):
        status, value = m.request(node, method, path, body, admin=admin)
        assert status in (200, 201, 202), (method, path, status, value)
        return value
    def click(identifier):
        f.wait(lambda: native.element(identifier, True), identifier)
        native.click(identifier)
    def reveal(identifier, container='agent-editor-dialog'):
        for _ in range(8):
            if native.element(identifier, True): return click(identifier)
            bounds = native.element(container)['bounds']
            native.ui('/v1/actions', {'type':'scroll','target':{'x':bounds['x']+bounds['width']*.6,'y':bounds['y']+bounds['height']*.6},'delta_y':-300})
        raise AssertionError('Not visible after scrolling: ' + identifier)
    try:
        nodes = [f.Node(root / name) for name in ('studio', 'laptop')]
        nodes[0].pair(nodes[1]); nodes[1].pair(nodes[0])
        client_transport=f.Node(client/'transport')
        (client_transport.root/'config.json').write_text(json.dumps({'mesh':{'offline':True,'bind':f'127.0.0.1:{client_transport.udp}'}}))
        sessions = []; skills = []; mcp_ids = []
        script = root / 'mcp.py'; script.write_text(m.FIXTURE)
        for index, node in enumerate(nodes):
            node.config['admin'] = {'token':'mcp-fixture'}
            node.config['mesh']['peers'][0]['client'] = True
            node.config['mesh']['peers'].append({'origin':client_transport.origin,'name':'Resource client','addr':f'127.0.0.1:{client_transport.udp}','client':True,'execute':[]})
            (node.root / 'config.json').write_text(json.dumps(node.config))
            node.request = lambda method, path, body=None, n=node: m.request(n, method, path, body)
            node.start(); f.wait(lambda: node.request('GET', '/readyz')[0] == 200, 'node ready')
            f.wait(lambda: node.request('GET','/v1/mesh')[1].get('origin') == node.origin, 'Mesh identity ready')
            skill = node.root / 'skills/01ARZ3NDEKTSV4RRFFQ69G5FAV'
            (skill / 'references').mkdir(parents=True)
            (skill / 'SKILL.md').write_text('---\nname: research-guide\ndescription: 整理研究资料，撰写有来源的报告。\n---\n# 研究手册\n\n保留证据，核对事实。\n')
            (skill / 'references/template.md').write_text('# 报告模板\n\n列出结论与引用来源。\n')
            request(node, 'POST', '/v1/node/agents', {'id':'research','name':'Research','role':'leader','profile_id':'fixture','model':'fixture-model','thinking':'off','skill_paths':[str(skill.resolve())]})
            session = request(node, 'POST', '/v1/node/agents/research/open', {})['session_id']
            sessions.append(session)
            catalog = request(node, 'GET', '/v1/node/agents/research/skills/catalog')
            skill_id = next(s['id'] for s in catalog['skills'] if s['name'] == 'research-guide'); skills.append(skill_id)
            details = request(node, 'GET', f'/v1/node/agents/research/skills/{skill_id}')
            assert '保留证据' in details['document']['text']
            assert details['document']['text'].startswith('# 研究手册'), details['document']
            assert m.request(node, 'GET', f'/v1/node/agents/research/skills/{skill_id}?file=..%2Fconfig.json')[0] == 400
            assert m.request(node, 'GET', f'/v1/node/agents/research/skills/{skill_id}', auth=False)[0] == 401
            installed = request(node, 'POST', '/admin/api/mcp', {'name':f'团队知识库 {index+1}','description':'按需查询团队资料','grant':{'scope':'selected','subjects':[]},'transport':{'kind':'stdio','command':sys.executable,'args':[str(script),str(root / 'calls.log')],'cwd':str(node.workspace),'env':{'DO_NOT_DISPLAY':'private-fixture-value'}}}, admin=True)
            mcp_id = installed['server_ref']['server_id']; mcp_ids.append(mcp_id)
            detail = request(node, 'GET', f'/v1/node/resources/mcp/{mcp_id}')
            assert detail['tools'][0]['name'] == 'echo', detail
            inventory=request(node,'GET','/v1/node/resources')
            assert next(item for item in inventory['items'] if item['kind']=='mcp' and item['id']==mcp_id)['status']=='ready'
            assert 'private-fixture-value' not in json.dumps(detail)
        checks.append('authenticated_skill_body_files_and_real_mcp_schema')
        request(nodes[1], 'POST', '/v1/services', {'session_id':sessions[1],'action':'attach','request_id':'resource-service','name':'报告服务','port':server.server_port})
        url = f'http://127.0.0.1:{server.server_port}/redirect'
        request(nodes[0], 'POST', f'/v1/im/sessions/{sessions[0]}/messages', {'content':json.dumps({'fake_tools':[
            {'name':'chat.send','input':{'chat_id':sessions[0],'text':f'[项目研究报告]({url})'}},
            {'name':'service.attach','input':{'name':'团队看板','port':server.server_port}},
            {'name':'chat.send','input':{'chat_id':sessions[1],'target':nodes[1].origin,'text':f'[跨设备报告]({url}/remote)'}}]}),'request_id':'resource-pages'})
        catalog = f.wait(lambda: (c if len((c := request(nodes[0], 'GET', '/v1/node/pages'))['applications']) == 1 else None), 'service application')
        assert catalog['applications'][0]['page']['title'] == '团队看板'
        assert not catalog['references'], 'Markdown indexing belongs to client core'
        f.wait(lambda: url + '/remote' in json.dumps(request(nodes[1], 'GET', f'/v1/im/sessions/{sessions[1]}/messages')), 'Markdown delivery over Mesh')
        checks.append('ordinary_markdown_messages_and_registered_service_application')
        nodes[0].restart_gateway()
        assert request(nodes[0], 'GET', '/v1/node/pages') == catalog
        checks.append('service_application_survives_restart')
        with sqlite3.connect(client / 'client.db') as db:
            db.executescript('CREATE TABLE nodes(id TEXT PRIMARY KEY,value TEXT NOT NULL);CREATE TABLE cache(node TEXT,key TEXT,value TEXT,PRIMARY KEY(node,key));')
            db.execute('INSERT INTO cache VALUES (?,?,?)', ('device','local-node-enabled','false'))
            for i, node in enumerate(nodes):
                saved = {'id':f'node-{i}','name':('工作室' if i == 0 else '笔记本'),'url':node.url,'token':'mcp-fixture','local':False}
                if i==1: saved.update(url='',token=None,mesh={'origin':node.origin,'addr':f'127.0.0.1:{node.udp}'})
                db.execute('INSERT INTO nodes VALUES (?,?)', (saved['id'],json.dumps(saved)))
        env = dict(os.environ,ZORK_CLIENT_DATA=str(client),ZORK_GUI_LOCALE='zh-CN',ZORK_GUI_PREFERENCES_PATH=str(root / 'preferences.json'),ZORK_GUI_TEST_WINDOW_SIZE='1280x800')
        native.process = subprocess.Popen([str(f.TARGET / 'zork-gui'),'--dev','--dev-port',native.url.rsplit(':',1)[1],'--dev-token','mesh-native-fixture'],env=env,stdout=native.log,stderr=native.log)
        f.wait(lambda: native.ui('/health'), 'native ready')
        click('connect-node-node-1')
        f.wait(lambda: native.element('device-node-1') and '在线' in native.element('device-node-1')['label'],'native Mesh connection ready')
        entry = f.wait(lambda: next((id for id in ('settings-tool-connections','desktop-manage') if native.element(id,True)),None), 'settings entry')
        if entry == 'desktop-manage': click(entry)
        click('settings-tool-connections')
        f.wait(lambda: native.element('resource-row-1',True), 'connections from two devices')
        assert not native.element('mesh-resources')
        assert native.element('settings-device-node-0')
        native.screenshot(art / 'tool-connections.png')
        click('resource-row-1'); f.wait(lambda: native.element('resource-tool-echo',True), 'MCP tool detail over Mesh')
        click('resource-tool-echo'); native.screenshot(art / 'mcp-parameters.png')
        native.ui('/v1/actions', {'type':'key','keystroke':'escape'})
        f.wait(lambda: not native.element('resource-detail-modal-close'), 'MCP Escape')
        click('settings-node-1-1'); click('agent-settings-research')
        f.wait(lambda: native.element('agent-editor-dialog'), 'existing Agent editor')
        reveal('agent-skill-' + skills[1])
        assert not native.element('resource-detail-modal-close')
        native.screenshot(art / 'agent-skill.png')
        reveal('resource-file-references/template.md'); native.screenshot(art / 'agent-skill-file.png')
        click('agent-editor-dialog-close')
        click('settings-device-node-1'); f.wait(lambda: native.element('resource-row-0',True), 'device services')
        native.screenshot(art / 'device-services.png')
        click('resource-row-0'); f.wait(lambda: native.element('resource-file-stderr.log',True), 'service logs')
        click('resource-file-stderr.log'); native.screenshot(art / 'service-log.png')
        click('resource-detail-modal-close')
        checks.append('mesh_client_mcp_skill_service_reads_in_existing_settings')
        click('desktop-return'); click('leader-node-0-research')
        f.wait(lambda: native.element('conversation-files-button',True), 'conversation')
        click('conversation-files-button'); click('conversation-page-' + reference['id'])
        f.wait(lambda: native.element('browser-page') and any(word in native.element('browser-page')['label'] for word in ('已显示','displayed')), 'embedded report')
        click('browser-more'); click('browser-agent-grant')
        def browser(action, strict=True):
            status,value = m.request(nodes[0], 'POST', '/v1/browser/command', {'session_id':sessions[0],'device_id':None,'command':{'request_id':'resource-browser-' + os.urandom(8).hex(),'action':action}})
            if strict: assert status == 200, (status,value)
            return value
        tabs = f.wait(lambda: browser({'op':'list'},False).get('tabs'), 'browser grant')
        tab = next(t for t in tabs if t['url'].endswith('/report'))['id']
        browser({'op':'type','tab_id':tab,'selector':'#note','text':'保留这段输入'})
        f.wait(lambda: '保留这段输入' in browser({'op':'read','tab_id':tab})['page']['text'], 'report input')
        click('conversation-files-button'); click('conversation-page-' + reference['id'])
        assert len(browser({'op':'list'})['tabs']) == len(tabs), 'opening a redirect page duplicated its tab'
        assert '保留这段输入' in browser({'op':'read','tab_id':tab})['page']['text']
        click('conversation-browser')
        f.wait(lambda: not native.element('browser-page'), 'browser hidden with report state retained')
        link=native.element('message-1-0-selection')['bounds']
        native.ui('/v1/actions',{'type':'click','target':{'x':link['x']+8,'y':link['y']+link['height']/2}})
        f.wait(lambda: native.element('browser-page'), 'message link opens embedded browser directly')
        assert len(browser({'op':'list'})['tabs']) == len(tabs)
        assert '保留这段输入' in browser({'op':'read','tab_id':tab})['page']['text']
        checks.append('message_link_opens_existing_page_directly')
        click('browser-new-tab'); f.wait(lambda: native.element('application-' + app['page']['id'],True), 'global applications')
        native.screenshot(art / 'applications.png')
        click('application-' + app['page']['id'])
        f.wait(lambda: len(browser({'op':'list'})['tabs']) == len(tabs)+1, 'application tab')
        click('browser-tab-' + tab)
        assert '保留这段输入' in browser({'op':'read','tab_id':tab})['page']['text']
        native.screenshot(art / 'retained-report.png')
        assert len(browser({'op':'list'})['tabs']) == len(tabs)+1
        checks.append('delivered_link_redirect_reuse_and_tab_input_preservation')
        assert 'private-fixture-value' not in native.ui('/v1/elements').decode()
        success = True
        (art / 'report.json').write_text(json.dumps({'outcome':'passed','devices':2,'checks':checks},ensure_ascii=False,indent=2)+'\n')
        print('PASS: ' + ', '.join(checks), flush=True)
    finally:
        if native.process:
            try:
                (art / 'last-elements.json').write_bytes(native.ui('/v1/elements'))
                native.screenshot(art / 'last-screen.png')
            except Exception: pass
        native.stop(); native.log.close()
        for node in reversed(nodes): node.stop()
        server.shutdown(); server.server_close()
        if (root / 'gui.log').exists(): shutil.copyfile(root / 'gui.log', art / 'native-ui.log')
        if success: shutil.rmtree(root)
        else: print('retained fixture:', root, flush=True)

if __name__ == '__main__': main()
