#!/usr/bin/env python3
"""Opt-in real Slack read-only smoke test; bot token is read from stdin only.

The rebuilt Station uses dummy credentials and a loopback safety proxy. Only the
proxy owns the real token, rejects writes and Socket Mode, and forwards bytes
unchanged. Model replies are controlled; no production bot process is started.
"""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError
from urllib.request import Request, urlopen

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('forward_fixture',ROOT/'scripts/test-slack-forwarding.py')
support=importlib.util.module_from_spec(spec);spec.loader.exec_module(support)
fixture=support.fixture
request=support.request
ALLOWED=frozenset(['auth.test','users.info','users.conversations','conversations.list','conversations.info','conversations.history','conversations.replies'])

class Guard:
    def __init__(self,token):
        self.records=[];self.blocked=[];self.lock=threading.Lock();self.total=0
        owner=self
        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                raw=self.rfile.read(int(self.headers.get('Content-Length','0')))
                method=self.path.removeprefix('/api/')
                if method not in ALLOWED:
                    owner.blocked.append(method);self.answer(403,b'{"ok":false,"error":"blocked_by_read_only_test"}');return
                if self.headers.get('Authorization')!='Bearer fixture-live-readonly':
                    self.answer(403,b'{"ok":false,"error":"unexpected_fixture_token"}');return
                with owner.lock:
                    owner.total+=1
                    permitted=owner.total<=24
                if not permitted:self.answer(429,b'{"ok":false,"error":"read_only_test_budget_exhausted"}');return
                started=time.monotonic()
                req=Request('https://slack.com/api/'+method,data=raw,headers={'Authorization':'Bearer '+token,'Content-Type':'application/x-www-form-urlencoded'},method='POST')
                try:
                    try:response=urlopen(req,timeout=20)
                    except HTTPError as error:response=error
                    with response:body=response.read(1024*1024+1);status=response.status;retry=response.headers.get('Retry-After')
                    if len(body)>1024*1024:self.answer(502,b'{"ok":false,"error":"response_too_large"}');return
                    parsed=json.loads(body)
                    owner.records.append({'method':method,'body':parsed,'status':status,'upstream_ms':round((time.monotonic()-started)*1000)})
                    self.answer(status,body,retry)
                except Exception:
                    self.answer(502,b'{"ok":false,"error":"live_read_failed"}')
            def answer(self,status,body,retry=None):
                self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)))
                if retry:self.send_header('Retry-After',retry)
                self.end_headers();self.wfile.write(body)
            def log_message(self,*args):pass
        self.server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
        self.url='http://127.0.0.1:'+str(self.server.server_port)+'/api'
        self.thread=threading.Thread(target=self.server.serve_forever,daemon=True);self.thread.start()
    def close(self):self.server.shutdown();self.server.server_close();self.thread.join()

def main():
    token=sys.stdin.readline().strip()
    if not token:raise SystemExit('Missing bot token on stdin; no tests performed')
    guard=Guard(token);del token
    report={'tests':[],'real_model':False,'writes_forwarded':0,'production_changed':False}
    try:
        with tempfile.TemporaryDirectory(prefix='zork-slack-readonly-') as temp:
            root=Path(temp);os.environ['ZORK_REGISTRY_DIR']=str(root/'registry')
            node=fixture.Node(root/'node')
            node.config['im_connections']=[{'id':'live-readonly','name':'Read-only test','provider':'slack','mode':'proactive','enabled':True,'app_token':'fixture-no-socket','bot_token':'fixture-live-readonly','api_base_url':guard.url}]
            (node.root/'config.json').write_text(json.dumps(node.config))
            binary=fixture.TARGET/'zork-station'
            with binary.open('rb') as f:report['binary_sha256']=hashlib.file_digest(f,'sha256').hexdigest()
            with (node.root/'live.log').open('wb') as log:
                process=subprocess.Popen([str(binary),'--data',str(node.root),'--fake-agent'],stdout=log,stderr=log)
                try:
                    fixture.wait(lambda:request(node.url,'GET','/readyz')[0]==200,'isolated Station ready')
                    status,created,_=request(node.agent_url,'POST','/sessions',{'profile_id':'fixture','model':'fixture-model','thinking':'off','workspace':str(node.workspace)})
                    assert status==201,'session creation failed'
                    sid=created['session_id'];taught=set()
                    def history():return request(node.agent_url,'GET',f'/sessions/{sid}/history?limit=200')[1]['items']
                    def tool(name,args):
                        seen={i['event_id'] for i in history()}
                        status,_,_=request(node.agent_url,'POST',f'/sessions/{sid}/mailbox',{'content':json.dumps({'fake_tool':{'name':name,'input':args}})})
                        assert status in (200,202),'input rejected'
                        def result():return next((i['event']['result'] for i in history() if i['event_id'] not in seen and i['event'].get('kind')=='tool_result' and i['event']['result']['tool']==name),None)
                        value=fixture.wait(result,'result '+name)
                        fixture.wait(lambda:request(node.agent_url,'GET',f'/sessions/{sid}')[1].get('status') in ('wait','finished','failed'),'turn finished')
                        return value
                    connections=tool('slack.connections',{})
                    assert connections['data']['connections'][0]['connect_id']=='live-readonly'
                    report['connection_selection']=True
                    def read(method,args):
                        name='slack.'+method
                        if method not in taught:
                            help_result=tool('tool.help',{'tool':name});assert help_result['outcome']=='succeeded','help failed';taught.add(method)
                        before=len(guard.records);started=time.monotonic()
                        result=tool(name,dict(args,connect_id='live-readonly'));body=result['data']
                        candidates=[r for r in guard.records[before:] if r['method']==method]
                        equal=any(r['body']==body for r in candidates)
                        row={'tool':name,'ok':body.get('ok',False),'error':body.get('error'),'tool_outcome':result['outcome'],'duration_ms':round((time.monotonic()-started)*1000),'raw_response_equal':equal}
                        if candidates:row.update(http_status=candidates[-1]['status'],upstream_ms=candidates[-1]['upstream_ms'])
                        for key in ('channels','messages','members'):
                            if isinstance(body.get(key),list):row[key+'_count']=len(body[key])
                        row['has_next_cursor']=bool(body.get('response_metadata',{}).get('next_cursor'))
                        report['tests'].append(row);print(json.dumps(row),flush=True)
                        assert equal,'Slack response changed or never reached real Slack'
                        assert (result['outcome']=='succeeded')==(body.get('ok') is True),'incorrect outcome'
                        return body
                    auth=read('auth.test',{})
                    if auth.get('ok'):
                        read('users.info',{'user':auth['user_id']})
                        page=read('conversations.list',{'limit':1,'exclude_archived':True})
                        cursor=page.get('response_metadata',{}).get('next_cursor')
                        if page.get('ok') and cursor:read('conversations.list',{'limit':1,'exclude_archived':True,'cursor':cursor});report['pagination_followed']=True
                        member=read('users.conversations',{'limit':5,'exclude_archived':True,'types':'public_channel,private_channel,im,mpim'})
                        channels=member.get('channels',[])
                        if channels:
                            channel=next((c for c in channels if c.get('is_im') or c.get('is_mpim')),channels[0])
                            read('conversations.info',{'channel':channel['id']})
                            history_page=read('conversations.history',{'channel':channel['id'],'limit':2})
                            messages=history_page.get('messages',[])
                            if messages:
                                message=messages[0]
                                read('conversations.replies',{'channel':channel['id'],'ts':message.get('thread_ts') or message['ts'],'limit':2})
                            else:report['thread_read_skipped']='no readable message in selected conversation'
                        else:report['thread_read_skipped']='no readable member conversation'
                    process.terminate();assert process.wait(timeout=30)==0,'Station shutdown failed'
                finally:
                    if process.poll() is None:process.kill();process.wait()
        report['upstream_read_requests']=guard.total
        report['socket_mode_blocked']=all(m=='apps.connections.open' for m in guard.blocked)
        output=ROOT/'.tmp/slack-live-readonly/result.json';output.parent.mkdir(parents=True,exist_ok=True);output.write_text(json.dumps(report,indent=2))
        print(json.dumps({'completed':True,'successes':sum(t['ok'] for t in report['tests']),'cases':len(report['tests']),'writes_forwarded':0,'report':str(output)}),flush=True)
    finally:guard.close()

if __name__=='__main__':main()
