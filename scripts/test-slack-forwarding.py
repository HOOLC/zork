#!/usr/bin/env python3
"""Rebuilt Station + fake model + two loopback Slack endpoints. No real Slack calls."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs
from urllib.request import Request, urlopen
from urllib.error import HTTPError

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('fixture',ROOT/'scripts/test-mesh.py')
fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)
fixture.TARGET=Path(os.environ.get('ZORK_TEST_BIN_DIR',str(Path(os.environ.get('CARGO_TARGET_DIR',ROOT/'target'))/'debug')))

class Slack:
    def __init__(self,name):
        self.name=name;self.calls=[]
        owner=self
        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                raw=self.rfile.read(int(self.headers.get('Content-Length','0')))
                fields={k:v[0] for k,v in parse_qs(raw.decode(),keep_blank_values=True).items()}
                method=self.path.rsplit('/',1)[-1]
                if method in ('auth.test','apps.connections.open'):
                    value={'ok':False,'error':'fixture_socket_offline'};status=200
                else:
                    owner.calls.append((method,fields,self.headers.get('Authorization')))
                    status=429 if method=='fixture.rateLimit' else 200
                    value=({'ok':False,'error':'ratelimited'} if status==429 else
                           {'ok':False,'error':'missing_scope','needed':'fixture:scope'} if method=='fixture.error' else
                           {'ok':True,'fixture':owner.name,'echo':fields,'ts':'123.456','has_more':True,'response_metadata':{'next_cursor':'opaque-next'},'unknown':{'nested':[1,False,None]}})
                body=json.dumps(value).encode();self.send_response(status)
                self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)))
                if status==429:self.send_header('Retry-After','37')
                self.end_headers();self.wfile.write(body)
            def log_message(self,*args):pass
        self.server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        self.url='http://127.0.0.1:'+str(self.server.server_port)+'/api'
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
    def close(self):self.server.shutdown();self.server.server_close();self.thread.join()

def request(base,method,path,body=None):
    req=Request(base+path,method=method,headers={'Content-Type':'application/json'},data=None if body is None else json.dumps(body).encode())
    try:r=urlopen(req,timeout=10)
    except HTTPError as e:r=e
    with r:
        raw=r.read()
        return r.status,json.loads(raw) if raw else None,dict(r.headers)

def main():
    a,b=Slack('a'),Slack('b')
    try:
        with tempfile.TemporaryDirectory(prefix='zork-slack-forward-') as temp:
            root=Path(temp);os.environ['ZORK_REGISTRY_DIR']=str(root/'registry')
            node=fixture.Node(root/'node')
            node.config['im_connections']=[dict(id=s.name,name=s.name,provider='slack',mode='proactive',enabled=True,app_token='fixture-app-'+s.name,bot_token='fixture-bot-'+s.name,api_base_url=s.url) for s in (a,b)]
            (node.root/'config.json').write_text(json.dumps(node.config))
            session_id=None
            for restart in range(2):
                with (node.root/'slack-test.log').open('ab') as log:
                    p=subprocess.Popen([str(fixture.TARGET/'zork-station'),'--data',str(node.root),'--fake-agent'],stdout=log,stderr=log)
                    try:
                        fixture.wait(lambda:request(node.url,'GET','/readyz')[0]==200,'Station ready')
                        if session_id is None:
                            status,session,_=request(node.agent_url,'POST','/sessions',{'profile_id':'fixture','model':'fixture-model','thinking':'off','workspace':str(node.workspace)})
                            assert status==201,(status,session);session_id=session['session_id']
                        def history():return request(node.agent_url,'GET',f'/sessions/{session_id}/history?limit=200')[1]['items']
                        def tool(name,args,outcome='succeeded'):
                            seen={x['event_id'] for x in history()}
                            status,_,_=request(node.agent_url,'POST',f'/sessions/{session_id}/mailbox',{'content':json.dumps({'fake_tool':{'name':name,'input':args}})})
                            assert status in (200,202),status
                            def result():
                                return next((i['event']['result'] for i in history() if i['event_id'] not in seen and i['event'].get('kind')=='tool_result' and i['event']['result']['tool']==name),None)
                            value=fixture.wait(result,'tool '+name)
                            assert value['outcome']==outcome,value
                            fixture.wait(lambda:request(node.agent_url,'GET',f'/sessions/{session_id}')[1].get('status') in ('wait','finished','failed'),'turn finished')
                            return value['data']
                        if restart==0:
                            connections=tool('slack.connections',{})['connections'];assert {c['connect_id'] for c in connections}=={'a','b'}
                            assert 'fixture-bot' not in json.dumps(connections)
                            for method in ['chat.postMessage','fixture.error','fixture.rateLimit']:
                                help_result=tool('tool.help',{'tool':'slack.'+method});assert 'connect_id' in help_result['parameters'];assert 'upload' in help_result['description']
                            for selected in ('a','b'):
                                args={'connect_id':selected,'channel':'C1','text':'中文 + & =','blocks':[{'type':'section','text':{'type':'plain_text','text':'nested'}}],'metadata':{'unknown':[True,None,3]},'cursor':'opaque-in','session_id':'not-the-caller'}
                                result=tool('slack.chat.postMessage',args)
                                assert result['fixture']==selected and result['ts']=='123.456' and result['response_metadata']['next_cursor']=='opaque-next'
                                assert result['echo']['text']==args['text'] and json.loads(result['echo']['blocks'])==args['blocks']
                                assert json.loads(result['echo']['metadata'])==args['metadata'];assert 'connect_id' not in result['echo']
                            assert a.calls[0][2]=='Bearer fixture-bot-a' and b.calls[0][2]=='Bearer fixture-bot-b'
                            expected={'ok':False,'error':'missing_scope','needed':'fixture:scope'}
                            assert tool('slack.fixture.error',{'connect_id':'a'},'failed')==expected
                            before=len(a.calls)+len(b.calls)
                            tool('slack.chat.postMessage',{},'failed');tool('slack.chat.postMessage',{'connect_id':'missing'},'failed')
                            for old in ['slack.history','slack.post_message','slack.post_file']:tool(old,{},'failed')
                            assert len(a.calls)+len(b.calls)==before
                            assert tool('slack.fixture.rateLimit',{'connect_id':'b'},'failed')=={'ok':False,'error':'ratelimited'}
                            status,body,headers=request(node.url,'POST','/v1/slack/forward',{'session_id':session_id,'connect_id':'a','method':'fixture.rateLimit','arguments':{}})
                            assert status==429 and body=={'ok':False,'error':'ratelimited'} and headers.get('retry-after',headers.get('Retry-After'))=='37'
                            for method,args in [('../evil',{}),('chat.postMessage',{'token':'override'})]:
                                before=len(a.calls)
                                status,_,_=request(node.url,'POST','/v1/slack/forward',{'session_id':session_id,'connect_id':'a','method':method,'arguments':args})
                                assert status==400 and len(a.calls)==before
                        else:
                            # Persisted exact-name knowledge must remain valid without teaching help again.
                            result=tool('slack.chat.postMessage',{'connect_id':'b','channel':'C2','text':'after restart'})
                            assert result['fixture']=='b' and result['echo']['text']=='after restart'
                        p.terminate();assert p.wait(timeout=30)==0
                    except Exception:
                        print((node.root/'slack-test.log').read_text()[-8000:]);raise
                    finally:
                        if p.poll() is None:p.kill();p.wait()
            assert len([c for c in b.calls if c[0]=='fixture.rateLimit'])==1,'write was retried'
            print('PASS: explicit A/B connection routing, native args/results/errors/cursors, exact-name help, missing connection and old-name rejection, no retries, restart, isolated real Station')
    finally:a.close();b.close()

if __name__=='__main__':main()
