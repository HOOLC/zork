#!/usr/bin/env python3
"""Isolated real Gateway/Agent/Mesh MCP contract; fake model, no user credentials."""
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.error import HTTPError
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts/lib"))
from build_env import build_environment
spec = importlib.util.spec_from_file_location("mesh_fixture", ROOT / "scripts/test-mesh.py")
f = importlib.util.module_from_spec(spec)
spec.loader.exec_module(f)
f.TARGET = Path(os.environ.get("ZORK_TEST_BIN_DIR", str(Path(build_environment().get("CARGO_TARGET_DIR", ROOT / "target")) / "debug")))

FIXTURE = r'''
import sys,json,time
counter=0
for line in sys.stdin:
 r=json.loads(line)
 if 'id' not in r: continue
 if r['method']=='initialize': result={'protocolVersion':'2025-11-25','capabilities':{'tools':{}}}
 elif r['method']=='tools/list': result={'tools':[{'name':'echo','description':'Echo fixture','inputSchema':{'type':'object','properties':{'text':{'type':'string'},'delay':{'type':'number'},'large':{'type':'boolean'}},'required':['text'],'additionalProperties':False}}]}
 elif r['method']=='tools/call':
  counter+=1
  p=r['params']['arguments']
  with open(sys.argv[1],'a') as log: log.write(json.dumps({'text':p['text'],'count':counter})+'\n')
  time.sleep(p.get('delay',0))
  result={'content':[{'type':'text','text':p['text']*100000 if p.get('large') else p['text']}],'structuredContent':{'count':counter}}
 else: result={}
 print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':result}),flush=True)
'''

class HttpMcp(BaseHTTPRequestHandler):
    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        if request.get('method') == 'initialize':
            result = {'protocolVersion': '2025-11-25', 'capabilities': {'tools': {}}}
        else:
            assert self.headers.get('MCP-Session-Id') == 'fixture-session'
            assert self.headers.get('MCP-Protocol-Version') == '2025-11-25'
            if 'id' not in request:
                self.send_response(202); self.end_headers(); return
            if request['method'] == 'tools/list':
                result = {'tools': [{'name': 'echo', 'inputSchema': {'type': 'object'}}]}
            else:
                result = {'content': [{'type': 'text', 'text': 'http-sse-ok'}]}
        message = json.dumps({'jsonrpc':'2.0','id':request['id'],'result':result})
        self.send_response(200)
        self.send_header('MCP-Session-Id', 'fixture-session')
        self.send_header('Content-Type', 'text/event-stream')
        self.end_headers()
        # CRLF framing and a notification before the response.
        self.wfile.write(b': comment\r\n\r\ndata: {"jsonrpc":"2.0","method":"notifications/tools/list_changed"}\r\n\r\n')
        self.wfile.write(('data: '+message+'\r\n\r\n').encode())
    def log_message(self, *_): pass


def request(node, method, path, body=None, admin=False, auth=True):
    base = 'http://' + node.config['bind']['control'] if admin else node.url
    req = Request(base+path, method=method, data=None if body is None else json.dumps(body).encode(), headers={'Content-Type':'application/json', **({'Authorization':'Bearer mcp-fixture'} if auth else {})})
    try: response = urlopen(req, timeout=25)
    except HTTPError as e: response = e
    with response:
        raw=response.read()
        return response.status, json.loads(raw) if raw else None


def main():
    root = Path(tempfile.mkdtemp(prefix='zork-mcp-'))
    print('fixture:',root,flush=True)
    nodes=[]; http=None; checks=[]; success=False
    try:
        a,b=f.Node(root/'a'),f.Node(root/'b'); nodes=[a,b]
        a.pair(b);b.pair(a)
        for node in nodes:
            node.config['admin']={'token':'mcp-fixture'}
            (node.root/'config.json').write_text(json.dumps(node.config))
            node.request=lambda method,path,body=None,n=node:request(n,method,path,body)
            node.start()
        for node in nodes:
            f.wait(lambda n=node:n.request('GET','/readyz')[0]==200,'node ready')
            f.wait(lambda n=node:n.get('/v1/mesh').get('origin')==n.origin,'stable Mesh identity')
        session=a.new_task()['session_id']; other=a.new_task()['session_id']
        counter=0
        def tool(op, sid=session, iid=None, **args):
            nonlocal counter
            counter+=1
            return request(a,'POST','/v1/mcp',{'session_id':sid,'invocation_id':iid or f'invoke-{counter}','request':{'op':op,**args}})
        def ok(pair):
            assert pair[0] in (200,201,202),pair
            return pair[1]
        def terminal(call):
            value=ok(tool('status',call_id=call))
            return value if value['state'] not in ('accepted','dispatching','running') else None
        script=root/'fixture.py';script.write_text(FIXTURE);log=root/'calls.jsonl'
        config={'name':'fixture','description':'Echo fixture','transport':{'kind':'stdio','command':sys.executable,'args':[str(script),str(log)],'cwd':str(root)},'grant':{'scope':'mesh'}}
        assert request(b,'POST','/admin/api/mcp',config,admin=True,auth=False)[0]==401
        config_file=root/'mcp.json';config_file.write_text(json.dumps(config))
        def cli(node,*args):
            return json.loads(subprocess.check_output([str(f.TARGET/'zork'),'mcp',*args,'--data',str(node.root)],text=True,timeout=25))
        server=cli(b,'add',str(config_file));ref=server['server_ref']
        assert cli(b,'get',ref['server_id'])['name']=='fixture'
        assert cli(b,'probe',ref['server_id'])['items'][0]['name']=='echo'
        assert cli(b,'list')['items'][0]['server']['availability']=='ready'
        assert re.fullmatch('[0-9A-HJKMNP-TV-Z]{26}',ref['server_id'])
        twin=ok(request(a,'POST','/admin/api/mcp',config,admin=True))
        found=f.wait(lambda: (v if len(v['items'])==2 else None) if (v:=ok(tool('search',query='fixture'))) else None,'mesh catalog')
        assert len({json.dumps(v['server_ref'],sort_keys=True) for v in found['items']})==2
        assert 'command' not in json.dumps(found)
        checks.append('authenticated_management_and_distinct_mesh_services')
        definition=ok(tool('inspect',server_ref=ref,tool='echo'))['items'][0]
        def call(iid=None,**args):
            return tool('call',iid=iid,server_ref=ref,tool='echo',binding_revision=definition['binding_revision'],arguments=args)
        receipt=ok(call(iid='same-call',text='first'));callid=receipt['call_id']
        done=f.wait(lambda:terminal(callid),'remote result');assert done['state']=='succeeded',done
        assert done['result']['content'][0]['text']=='first'
        assert ok(call(iid='same-call',text='first'))['call_id']==callid
        assert call(iid='same-call',text='different')[0]==400
        assert tool('status',sid=other,call_id=callid)[0]==400
        assert len(log.read_text().splitlines())==1
        checks.append('remote_stdio_call_dedup_conflict_and_private_results')
        # Fault injection: receipt lost before the caller durably records it.
        with sqlite3.connect(a.root/'state/mcp.sqlite') as db:
            db.execute("UPDATE routes SET call=NULL,request=? WHERE invocation='same-call'", (json.dumps({"op":"call","server_ref":ref,"tool":"echo","binding_revision":definition["binding_revision"],"arguments":{"text":"first"}}),))
        a.restart_gateway()
        f.wait(lambda:a.get('/v1/mesh').get('origin')==a.origin,'caller Mesh identity after restart')
        recovered=ok(tool('recover'))
        assert recovered['calls'][0]['call_id']==callid and not recovered['pending_delivery'],recovered
        assert len(log.read_text().splitlines())==1
        checks.append('sender_restart_recovers_original_receipt')
        invalid=ok(call(text=42));assert f.wait(lambda:terminal(invalid['call_id']),'invalid args')['state']=='failed'
        assert len(log.read_text().splitlines())==1
        large=ok(call(text='中文',large=True));large_done=f.wait(lambda:terminal(large['call_id']),'large result')
        assert large_done['read_required'];chunks=[];offset=0
        while True:
            chunk=ok(tool('read',call_id=large['call_id'],offset=offset));chunks.append(chunk['data']);offset=chunk['next_offset']
            if offset is None: break
        assert json.loads(''.join(chunks))['content'][0]['text']=='中文'*100000
        checks.append('schema_validation_and_bounded_large_results')
        delayed=ok(call(text='cancel',delay=15));f.wait(lambda:len(log.read_text().splitlines())==3,'upstream dispatch')
        ok(tool('cancel',call_id=delayed['call_id']));assert f.wait(lambda:terminal(delayed['call_id']),'cancel result')['state']=='outcome_unknown'
        checks.append('cancel_never_claims_unexecuted')
        # A persistent dispatch record survives a hard Gateway restart without replay.
        delayed=ok(call(iid='crash-call',text='crash',delay=15));f.wait(lambda:len(log.read_text().splitlines())==4,'crash dispatch')
        b.restart_gateway()
        f.wait(lambda:b.get('/v1/mesh').get('origin')==b.origin,'server Mesh identity after restart')
        assert f.wait(lambda:terminal(delayed['call_id']),'crash recovery')['state']=='outcome_unknown'
        assert ok(call(iid='crash-call',text='crash',delay=15))['call_id']==delayed['call_id']
        assert len(log.read_text().splitlines())==4
        checks.append('gateway_crash_does_not_repeat_effects')
        # Config revisions invalidate previously inspected definitions.
        config['description']='changed'
        server=ok(request(b,'PUT','/admin/api/mcp/'+ref['server_id'],{'expected_revision':server['config_revision'],'config':config},admin=True))
        stale=ok(call(text='stale'));assert f.wait(lambda:terminal(stale['call_id']),'stale definition')['state']=='failed'
        assert len(log.read_text().splitlines())==4
        config['grant']={'scope':'local'}
        server=ok(request(b,'PUT','/admin/api/mcp/'+ref['server_id'],{'expected_revision':server['config_revision'],'config':config},admin=True))
        assert tool('inspect',server_ref=ref,tool='echo')[0]==200
        assert tool('status',call_id=callid)[0]==200
        checks.append('definition_changes_and_legacy_grants_do_not_override_mesh_trust')
        http=ThreadingHTTPServer(('127.0.0.1',0),HttpMcp);threading.Thread(target=http.serve_forever,daemon=True).start()
        remote=ok(request(b,'POST','/admin/api/mcp',{'name':'http','transport':{'kind':'http','url':f'http://127.0.0.1:{http.server_port}/mcp'},'grant':{'scope':'mesh'}},admin=True))
        definition=ok(tool('inspect',server_ref=remote['server_ref'],tool='echo'))['items'][0]
        receipt=ok(tool('call',server_ref=remote['server_ref'],tool='echo',binding_revision=definition['binding_revision'],arguments={}))
        done=f.wait(lambda:terminal(receipt['call_id']),'HTTP result');assert done['result']['content'][0]['text']=='http-sse-ok',done
        checks.append('streamable_http_sse_and_session_headers')
        twin_id=twin['server_ref']['server_id']
        assert cli(a,'disable',twin_id)['availability']=='disabled'
        assert cli(a,'enable',twin_id)['availability']=='unprobed'
        assert cli(a,'remove',twin_id)['ok']
        # Actual model -> dynamic tool -> Gateway path, not just HTTP handlers.
        content=json.dumps({'fake_tools':[{'name':'mcp.search','input':{'query':'http'}}]})
        ok(request(a,'POST',f'/v1/im/sessions/{session}/messages',{'content':content,'request_id':'mcp-agent-fixture'}))
        def agent_result():
            for segment in (a.root/'shared-files/sessions'/session/'segments').glob('*.jsonl'):
                for line in segment.read_text().splitlines():
                    event=json.loads(line).get('event',{})
                    result=event.get('result',{})
                    if event.get('kind')=='tool_result' and result.get('tool')=='mcp.search':
                        assert result['outcome']=='succeeded',result
                        assert any(v['name']=='http' for v in result['data']['items']),result
                        return True
            return False
        f.wait(agent_result,'actual Agent MCP tool result')
        checks.append('agent_dynamic_tool_result')
        print(json.dumps({'checks':checks,'count':len(checks)},ensure_ascii=False,indent=2),flush=True)
        success=True
    finally:
        if http: http.shutdown();http.server_close()
        for node in reversed(nodes): node.stop()
        # Preserve failure evidence; successful fixtures contain no useful user data.
        if success: shutil.rmtree(root)

if __name__=='__main__': main()
