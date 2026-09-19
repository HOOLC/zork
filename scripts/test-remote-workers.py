#!/usr/bin/env python3
"""Two nodes: Leader-owned Tasks invoke explicitly granted remote Worker Agents."""
import importlib.util,json,os,sqlite3,tempfile,time
from pathlib import Path
from urllib.request import Request,urlopen
from urllib.error import HTTPError
spec=importlib.util.spec_from_file_location('fixture',Path(__file__).with_name('test-mesh.py'));f=importlib.util.module_from_spec(spec);spec.loader.exec_module(f)
root=Path(tempfile.mkdtemp(prefix='zrw-',dir='/tmp'));a,b=f.Node(root/'leader'),f.Node(root/'worker')
a.pair(b);b.pair(a)
for node in (a,b):node.config['admin']={'token':'remote-worker-fixture'};node.config['mesh']['peers'][0]['execute']=[];(node.root/'config.json').write_text(json.dumps(node.config))
def req(node,method,path,body=None,leader=None):
    headers={'Content-Type':'application/json','Authorization':'Bearer remote-worker-fixture'}
    if leader:headers['x-zork-session-key']=leader['session_key']
    try:response=urlopen(Request(node.url+path,method=method,data=None if body is None else json.dumps(body).encode(),headers=headers),timeout=12)
    except HTTPError as e:response=e
    with response:raw=response.read();return response.status,json.loads(raw) if raw else None
def admin(node,method,path,body=None):
    status,value=req(node,method,path,body);assert status in (200,201,202),(status,value);return value
def tasks(node):return node.get('/v1/tasks')['items']
def start():
    for node in (a,b):node.start()
    for node in (a,b):f.wait(lambda:node.request('GET','/readyz')[0]==200,'Station ready');f.wait(lambda:urlopen(node.agent_url+'/readyz',timeout=2).status==200,'Agent ready')
try:
    start()
    selection={'profile_id':'fixture','model':'fixture-model','thinking':'off'}
    leader=admin(a,'POST','/v1/node/agents',dict(selection,id='leader',name='Leader A',role='leader'));admin(a,'POST','/v1/node/agents/leader/open',{})
    worker=admin(b,'POST','/v1/node/agents',dict(selection,id='worker',name='Worker B',role='worker',allowed_leaders=[a.origin+'/leader']))
    remote_id=b.origin+'/worker'
    f.wait(lambda:any(w['id']==remote_id for w in req(a,'GET','/v1/agent/workers',leader=leader)[1]['items']),'authorized remote catalog')
    goal=json.dumps({'fake_tools':[{'name':'shell.run','input':{'command':"while ! test -f activity-release; do sleep 0.1; done; printf 'one\\n' >> executions.txt\nprintf '# remote report\\n' > report.md"}},{'name':'chat.post_message','input':{'text':'Remote Worker completed'}}]})
    body={'request_id':'first','worker_id':remote_id,'goal':goal}
    status,first=req(a,'POST','/v1/agent/tasks',body,leader);assert status==200,(status,first)
    assert req(a,'POST','/v1/agent/tasks',body,leader)[1]['session_id']==first['session_id']
    status,second=req(a,'POST','/v1/agent/tasks',dict(body,request_id='second'),leader);assert status==200,(status,second)
    def worker_activity():
        status, value=req(a,'GET',f"/v1/im/sessions/{first['session_id']}/status")
        if status != 200: return None
        return next((p for p in value['items'] if p['id']==remote_id and p['name']=='Worker B' and p.get('activity') and p['activity'].get('calls')),None)
    active=f.wait(worker_activity,'remote concrete Worker activity',30)
    assert any('activity-release' in c['detail'] for c in active['activity']['calls']),active
    assert active['session_id']==first['session_id']
    f.wait(lambda:len(tasks(b))==2,'both remote workspaces')
    for t in tasks(b):(Path(t['workspace'])/'activity-release').touch()
    f.wait(lambda:len(tasks(a))==2 and all(t['state']=='review' and t['last_run_status']=='finished' for t in tasks(a)),'two remote Worker results',90)
    assert len({t['workspace'] for t in tasks(b)})==2
    assert all((Path(t['workspace'])/'executions.txt').read_text().splitlines()==['one'] for t in tasks(b))
    runtime_ids={s['session_id'] for s in json.load(urlopen(a.agent_url+'/sessions'))['items']};assert runtime_ids=={leader['session_id']},runtime_ids
    remote_sessions={t['session_id'] for t in tasks(b)}
    with sqlite3.connect(a.root/'state/station.sqlite') as db:
        f.wait(lambda:db.execute('SELECT COUNT(*) FROM leader_notifications WHERE delivered=1').fetchone()[0]==2,'remote results notify original Leader')
        db.execute("UPDATE mesh_links SET state='queued'")
    with sqlite3.connect(b.root/'state/station.sqlite') as db:db.execute("UPDATE mesh_links SET state='dispatching'")
    a.stop();b.stop();start();time.sleep(3)
    assert len(tasks(a))==2 and {t['session_id'] for t in tasks(b)}==remote_sessions
    assert all(t['run_count']==1 for t in tasks(a)),tasks(a)
    first_task=next(t for t in tasks(a) if t['session_id']==first['session_id'])
    rework_goal=json.dumps({'fake_tools':[{'name':'chat.post_file','input':{'attachments':[{'file_path':'report.md'}],'text':'Remote Agent artifact'}},{'name':'chat.post_message','input':{'text':'Remote rework complete'}}]})
    route='/v1/agent/tasks/'+first_task['task_id']+'/rework';rework={'request_id':'revise','goal':rework_goal,'expected_revision':first_task['revision']}
    status,value=req(a,'POST',route,rework,leader);assert status==200,(status,value)
    assert req(a,'POST',route,rework,leader)[0]==200
    f.wait(lambda:any(t['result_text']=='Remote rework complete' for t in tasks(a)),'remote same-session rework',90)
    artifact=f.wait(lambda:next(iter(a.get('/v1/artifacts')['items']),None),'remote file imported')
    with urlopen(a.url+'/v1/artifacts/'+artifact['artifact_id']+'/content') as response:assert response.read()==b'# remote report\n'
    assert {t['session_id'] for t in tasks(b)}==remote_sessions and len(tasks(a))==2
    admin(b,'PUT','/v1/node/agents/worker/grants',{'expected_allowed_leaders':worker['allowed_leaders'],'allowed_leaders':[]})
    assert req(a,'GET','/v1/agent/workers',leader=leader)[1]['items']==[]
    status,value=req(a,'POST','/v1/agent/tasks',dict(body,request_id='revoked'),leader)
    if status==200:f.wait(lambda:next(t for t in tasks(a) if t['session_id']==value['session_id'])['mesh'].get('error'),'revocation blocks remote intake')
    else:assert status==403,(status,value)
    assert len(tasks(b))==2,'revoked Worker ran a new task'
    print('PASS: remote Worker catalog, 2 isolated sessions, Leader-owned Tasks without local Worker runtime, durable notifications, lost ACK replay, same-session remote rework, artifact transfer, revoked grants')
    artifacts=f.ROOT/'artifacts/leader-worker';(artifacts/'remote-result.json').write_text(json.dumps({'root':str(root),'leader_origin':a.origin,'worker_origin':b.origin,'worker_sessions':sorted(remote_sessions)},indent=2))
finally:a.stop();b.stop();print(root)
