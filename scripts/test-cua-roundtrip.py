#!/usr/bin/env python3
"""Isolated Station -> native cua IPC -> image -> model-provider HTTP roundtrip.

Default mode uses a fake desktop host and a local model-provider fixture. It never
captures the user's display or injects input. This proves transport and error
contracts, not real desktop permission or model capability acceptance.
"""
import argparse
import base64
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import plistlib
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]
PNG = 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jRZkAAAAASUVORK5CYII='
VERSION = '0.28.2'


def port():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]


def wait(predicate, label, seconds=60):
    deadline = time.monotonic() + seconds
    last = None
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                return value
        except (OSError, URLError, ValueError, KeyError) as error:
            last = str(error)
        time.sleep(.1)
    raise AssertionError(f'{label}: timed out ({last})')


def fixture_host(config):
    settings = json.loads(Path(config).read_text())
    if '--endpoint' in sys.argv:
        print(json.dumps(settings['endpoint']))
        return
    endpoint = settings['endpoint']
    marker = Path(settings['marker'])
    with socket.socket(socket.AF_UNIX) as server:
        server.bind(endpoint['socket'])
        os.chmod(endpoint['socket'], 0o600)
        Path(endpoint['status']).write_text(json.dumps({'driver_pid':os.getpid(), 'host_pid':os.getpid(), 'state':'started'}))
        server.listen()
        while True:
            connection, _ = server.accept()
            with connection, connection.makefile('rwb') as stream:
                while line := stream.readline():
                    request = json.loads(line)
                    method = request['method']
                    if method == 'metadata':
                        response = {'ok':True, 'result':{'driver_version':VERSION,'embedded':True,
                                    'host_bundle_id':endpoint['bundle_id'], 'pid':os.getpid()}}
                    elif method == 'call':
                        name, arguments = request['name'], request['args']
                        assert request['session_id'].startswith('zork-')
                        assert not any(key.startswith('_') for key in arguments)
                        if name == 'get_window_state':
                            response = {'ok':True,'result':{'isError':False,'content':[
                                {'type':'image','mimeType':'image/png','data':PNG},
                                {'type':'text','text':'Isolated fixture button at x=20,y=20'}]}}
                        elif name == 'click':
                            assert arguments == {'pid':1,'window_id':1,'x':20,'y':20}
                            marker.write_text('clicked')
                            response = {'ok':True,'result':{'isError':False,'content':[{'type':'text','text':'fixture clicked'}]}}
                        else:
                            response = {'ok':True,'result':{'isError':True,'content':[{'type':'text','text':'fixture_denied'}]}}
                    else:
                        response = {'ok':False,'error':'fixture_unknown_method'}
                    stream.write(json.dumps(response).encode()+b'\n')
                    stream.flush()


def run(args):
    args.output.mkdir(parents=True, exist_ok=True)
    report = {'desktop':'real isolated blank window' if args.host_app else 'fixture', 'model':'local HTTP fixture', 'real_desktop_verified':False, 'checks':[]}
    window_process = None
    own_host = not bool(args.host_app)
    with tempfile.TemporaryDirectory(prefix='zcua-', dir='/tmp') as temporary:
        root = Path(temporary)
        os.chmod(root, 0o700)
        marker = root/'clicked'
        target = {'pid':1,'window_id':1}
        if args.host_app:
            app = args.host_app.resolve()
            endpoint = json.loads(subprocess.check_output([str(app/'Contents/MacOS/zork-cua-host'),'--endpoint']))
            own_host = not Path(endpoint['socket']).exists()
            subprocess.run(['open','-g','-a',str(app)],check=True)
            # Probe only permission status. Never request or click a system authorization dialog.
            deadline = time.monotonic()+8
            while time.monotonic()<deadline and not Path(endpoint['socket']).exists():
                time.sleep(.1)
            status = json.loads(Path(endpoint['status']).read_text()) if Path(endpoint['status']).exists() else {}
            report['permissions'] = status
            if not Path(endpoint['socket']).exists():
                report['passed'] = False
                report['blocked'] = 'Host requires user-granted Accessibility and Screen Recording, or failed to start'
                (args.output/'result.json').write_text(json.dumps(report,indent=2)+'\n')
                print(json.dumps(report,indent=2))
                raise SystemExit(77)
            fixture = root/'cua-window'
            subprocess.run(['clang','-fobjc-arc',str(ROOT/'scripts/test-fixtures/cua-window.m'),
                            '-framework','AppKit','-o',str(fixture)],check=True)
            window_info = root/'window.json'
            window_process = subprocess.Popen([str(fixture),str(marker),str(window_info)],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            target = wait(lambda:json.loads(window_info.read_text()),'isolated blank window')
            assert target['pid']==window_process.pid and target['window_id']>0
        else:
            app = root/'FixtureHost.app'
            mac = app/'Contents/MacOS'
            mac.mkdir(parents=True)
            endpoint = {'socket':str(root/'driver.sock'), 'status':str(root/'status.json'),
                        'bundle_id':'ing.zork.cua-roundtrip.fixture', 'driver_version':VERSION}
            config = root/'host.json'
            marker = root/'clicked'
            config.write_text(json.dumps({'endpoint':endpoint,'marker':str(marker)}))
            executable = mac/'zork-cua-host'
            # Exact paths are Python literals, not shell interpolation.
            executable.write_text('#!'+str(Path(sys.executable).resolve())+'\nimport runpy,sys\nsys.argv='+repr([str(Path(__file__).resolve()),'--fixture-host',str(config)])+'+sys.argv[1:]\nrunpy.run_path(sys.argv[0],run_name="__main__")\n')
            executable.chmod(0o755)
            (app/'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleIdentifier':endpoint['bundle_id'],
                'CFBundleExecutable':'zork-cua-host','CFBundleName':'Zork Cua Fixture','CFBundlePackageType':'APPL','LSUIElement':True}))
            subprocess.run(['codesign','--force','--sign','-',str(app)], check=True, capture_output=True)
        completed = threading.Event()
        errors, bodies = [], []

        class Model(BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                bodies.append(body)
                index = len(bodies)-1
                try:
                    assert self.path.endswith('/chat/completions'), self.path
                    if index == 0:
                        tool, arguments = 'computer.control', {'tool':'get_window_state','arguments':target}
                    elif index == 1:
                        encoded = json.dumps(body)
                        assert 'data:image/png;base64,' in encoded, 'model did not receive the capture'
                        if not args.host_app:
                            assert 'data:image/png;base64,'+PNG in encoded, 'fixture capture changed'
                        else:
                            for message in body['messages']:
                                if isinstance(message.get('content'),list):
                                    for part in message['content']:
                                        image = part.get('image_url',{}).get('url','')
                                        if image.startswith('data:image/png;base64,'):
                                            image_bytes = base64.b64decode(image.split(',',1)[1])
                                            (args.output/'isolated-window.png').write_bytes(image_bytes)
                                            report['png_sha256'] = hashlib.sha256(image_bytes).hexdigest()
                        assert 'zork-computer-' not in encoded, 'capture filesystem path leaked to model'
                        report['checks'].append('native capture reached the provider as an image')
                        if args.host_app:
                            def nodes(value):
                                if isinstance(value,dict):
                                    yield value
                                    for item in value.values(): yield from nodes(item)
                                elif isinstance(value,list):
                                    for item in value: yield from nodes(item)
                                elif isinstance(value,str):
                                    try: decoded = json.loads(value)
                                    except ValueError: return
                                    if decoded != value: yield from nodes(decoded)
                            button = next(v for v in nodes(body['messages']) if v.get('label')=='Cua fixture click' and 'element_index' in v)
                            click = dict(target,element_index=button['element_index'])
                        else:
                            click = dict(target,x=20,y=20)
                        tool, arguments = 'computer.control', {'tool':'click','arguments':click}
                    elif index == 2:
                        wait(lambda:marker.read_text()=='clicked','isolated button callback',seconds=5)
                        if not args.host_app: assert 'fixture clicked' in json.dumps(body)
                        report['checks'].append('model action returned through Station/native IPC')
                        tool, arguments = 'computer.control', {'tool':'fail_fixture','arguments':{}}
                    else:
                        if not args.host_app: assert 'fixture_denied' in json.dumps(body)
                        tool_messages = [m for m in body['messages'] if m['role']=='tool']
                        assert any('failed' in json.dumps(m) for m in tool_messages), 'desktop failure became success'
                        report['checks'].append('driver isError preserved as failed model tool result')
                        completed.set()
                        tool, arguments = 'end', {}
                    response = {'id':f'fixture-{index}','object':'chat.completion','created':1,'model':'fixture',
                        'choices':[{'index':0,'message':{'role':'assistant','content':None,'tool_calls':[{
                            'id':f'cua-{index}','type':'function','function':{'name':'call','arguments':json.dumps({
                                'tool':tool,'action':'Run isolated desktop fixture','arguments':arguments})}}]},'finish_reason':'tool_calls'}],
                        'usage':{'prompt_tokens':10,'completion_tokens':10,'total_tokens':20}}
                    payload = json.dumps(response).encode()
                    self.send_response(200)
                except Exception as error:
                    errors.append(repr(error)); completed.set()
                    payload = json.dumps({'error':str(error)}).encode()
                    self.send_response(500)
                self.send_header('Content-Type','application/json')
                self.send_header('Content-Length',str(len(payload)))
                self.end_headers(); self.wfile.write(payload)

        model = ThreadingHTTPServer(('127.0.0.1',0), Model)
        threading.Thread(target=model.serve_forever, daemon=True).start()
        node = root/'node'; (node/'profiles').mkdir(parents=True)
        (node/'profiles/fixture.json').write_text(json.dumps({'provider':'openai-compatible','billing':'usage',
            'base_url':f'http://127.0.0.1:{model.server_port}/v1','auth':{'type':'api_key','key':'fixture'},
            'models':[{'id':'fixture','api':'openai-completions','streaming':False,'thinking':['off'],'default_thinking':'off',
                       'capabilities':{'input':['text','image']},'limits':{'context_window_tokens':100000,'max_output_tokens':10000},'default':True}]}))
        bindings = {name:f'127.0.0.1:{port()}' for name in ('station','runtime','control','agent')}
        (node/'config.json').write_text(json.dumps({'bind':bindings,'admin':{'token':'fixture'},'mesh':{'enabled':False}}))
        base = 'http://'+bindings['runtime']

        def request(method, path, value=None, expected=200, headers=None, agent=False):
            url = ('http://'+bindings['agent']) if agent else base
            req = Request(url+path, method=method, data=None if value is None else json.dumps(value).encode(),
                          headers={'Content-Type':'application/json','Authorization':'Bearer fixture',**(headers or {})})
            try:
                response = urlopen(req, timeout=15)
            except HTTPError as error:
                response = error
            with response:
                data = json.loads(response.read())
                assert response.status == expected, (response.status,data)
                return data

        process = None
        try:
            with (args.output/'station.log').open('wb') as log:
                env = dict(os.environ, ZORK_CUA_HOST_APP=str(app))
                process = subprocess.Popen([str(args.bin_dir/'zork-station'),'--data',str(node)],
                                           env=env, stdout=log, stderr=log, start_new_session=True)
                wait(lambda: request('GET','/readyz'), 'Station ready')
                request('POST','/v1/computer/command',{'tool':'click'},expected=403)
                report['checks'].append('missing session rejected')
                request('POST','/v1/node/agents',{'id':'cua-fixture','name':'Cua fixture','role':'leader',
                         'profile_id':'fixture','model':'fixture','thinking':'off'})
                opened = request('POST','/v1/node/agents/cua-fixture/open',{})
                session = opened['session_id']
                execution = opened['agent']['session_id']
                request('POST',f'/v1/im/sessions/{session}/messages',{'content':'Exercise the isolated cua fixture.'},expected=202)
                assert completed.wait(90), 'model roundtrip timed out'
                assert not errors, errors
                wait(lambda:request('GET',f'/sessions/{execution}',agent=True)['status']=='finished','explicit model end')
                report['provider_requests'] = len(bodies)
                if not args.host_app: report['png_sha256'] = hashlib.sha256(base64.b64decode(PNG)).hexdigest()
                report['real_desktop_verified'] = bool(args.host_app)
                report['passed'] = True
        finally:
            if process:
                process.terminate()
                try: process.wait(15)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid,signal.SIGKILL); process.wait()
            if window_process:
                window_process.terminate(); window_process.wait(5)
            if own_host and Path(endpoint['status']).exists():
                pid = json.loads(Path(endpoint['status']).read_text())['host_pid']
                try: os.kill(pid,signal.SIGTERM)
                except ProcessLookupError: pass
            model.shutdown(); model.server_close()
            lsregister = '/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister'
            if not args.host_app: subprocess.run([lsregister,'-u',str(app)],capture_output=True)
            (args.output/'result.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))


def main():
    if len(sys.argv)>2 and sys.argv[1]=='--fixture-host':
        fixture_host(sys.argv[2]); return
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--host-app',type=Path,help='Explicit signed host for real capture/click on a new isolated blank window; never grants permissions')
    parser.add_argument('--bin-dir',type=Path,default=ROOT/'target/debug')
    parser.add_argument('--output',type=Path,required=True)
    args = parser.parse_args()
    run(args)


if __name__=='__main__':
    main()
