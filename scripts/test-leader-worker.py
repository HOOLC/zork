#!/usr/bin/env python3
"""Real Station/Agent processes, dynamic tools and isolated fake model inputs."""
import concurrent.futures,importlib.util,json,os,signal,sqlite3,subprocess,tempfile,time
from pathlib import Path
from urllib.request import Request,urlopen
from urllib.error import HTTPError
spec=importlib.util.spec_from_file_location('fixture',Path(__file__).with_name('test-mesh.py'));f=importlib.util.module_from_spec(spec);spec.loader.exec_module(f)
root=Path(tempfile.mkdtemp(prefix='zlw-',dir='/tmp'));(root/'profiles').mkdir()
(root/'profiles/fixture.json').write_text(json.dumps({'provider':'openai','billing':'usage','base_url':'http://127.0.0.1:9/v1','auth':{'type':'api_key','key':'sk-fixture'},'models':[{'id':'fixture-model','api':'openai-completions','streaming':False,'thinking':['off'],'default_thinking':'off','capabilities':{'input':['text']},'limits':{'context_window_tokens':100000,'max_output_tokens':10000},'default':True}]}))
bindings={name:f'127.0.0.1:{f.port()}' for name in ('station','runtime','control','agent')}
(root/'config.json').write_text(json.dumps({'bind':bindings,'admin':{'token':'isolated-node-admin'}}));url='http://'+bindings['runtime'];agent_url='http://'+bindings['agent'];log=(root/'process.log').open('ab');process=None

def req(method,path,body=None,headers=None,base=url):
    headers={'Content-Type':'application/json',**(headers or {})};data=None if body is None else json.dumps(body).encode()
    try:response=urlopen(Request(base+path,data=data,method=method,headers=headers),timeout=15)
    except HTTPError as e:response=e
    with response:raw=response.read();return response.status,json.loads(raw) if raw else None

def admin(method,path,body=None):
    status,value=req(method,path,body,{'Authorization':'Bearer isolated-node-admin'});assert status in (200,201,202),(status,value);return value

def start():
    global process
    process=subprocess.Popen([str(f.TARGET/'zork'),'start','--data',str(root),'--fake-agent'],stdout=log,stderr=log,start_new_session=True)
    f.wait(lambda:req('GET','/readyz')[0]==200,'station ready');f.wait(lambda:req('GET','/readyz',base=agent_url)[0]==200,'agent ready')
def stop():
    global process
    if process:
        process.terminate()
        try:process.wait(timeout=12)
        except subprocess.TimeoutExpired:os.killpg(process.pid,signal.SIGKILL);process.wait()
        process=None

def tasks():return req('GET','/v1/tasks')[1]['items']
def create(id,role,allowed=[]):return admin('POST','/v1/node/agents',{'id':id,'name':id,'role':role,'profile_id':'fixture','model':'fixture-model','thinking':'off','allowed_leaders':allowed})
try:
    start();assert not (root/'bin/zork-call').exists(),'runtime still installs CLI'
    assert req('GET','/v1/node/agents')[0]==401
    leader=create('leader','leader');other=create('other-leader','leader');worker=create('worker','worker',['leader']);hidden=create('hidden','worker',[])
    with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
        opened=list(pool.map(lambda _:admin('POST','/v1/node/agents/leader/open',{}),range(6)))
    session=opened[0]['session_id'];assert all(v['session_id']==session for v in opened)
    assert tasks()==[],'Leader conversation must never become a product Task'
    summary=next(s for s in req('GET','/v1/im/sessions')[1]['items'] if s['session_id']==session)
    report=Path(summary['workspace'])/'leader-report.md';report.write_text('# Leader report\n')
    file_input=json.dumps({'fake_tool':{'name':'chat.post_file','input':{'attachments':[{'file_path':str(report)}],'text':'Conversation file'}}})
    assert req('POST',f'/v1/im/sessions/{session}/messages',{'content':file_input})[0]==202
    artifact=f.wait(lambda:next(iter(req('GET','/v1/artifacts')[1]['items']),None),'Leader file tool')
    assert artifact['task_id'] is None and artifact['session_id']==session
    assert tasks()==[],'Leader file fabricated a Task'
    report.unlink()
    with urlopen(url+'/v1/artifacts/'+artifact['artifact_id']+'/content') as response:assert response.read()==b'# Leader report\n'

    assert req('POST','/v1/node/agents/worker/open',{}, {'Authorization':'Bearer isolated-node-admin'})[0]==403
    headers={'x-zork-session-key':leader['session_key']}
    assert [v['id'] for v in req('GET','/v1/agent/workers',headers=headers)[1]['items']]==['worker']
    assert req('GET','/v1/agent/workers',headers={'x-zork-session-key':other['session_key']})[1]['items']==[]
    # Leader's model invokes a dynamic logical tool, not a shell subprocess.
    goal=json.dumps({'fake_tool':{'name':'chat.post_message','input':{'text':'Worker delivered via dynamic tool'}}})
    body={'worker_id':'worker','goal':goal}
    dynamic=json.dumps({'fake_tool':{'name':'agent.assign','input':body}})
    assert req('POST',f'/v1/im/sessions/{session}/messages',{'content':dynamic})[0]==202
    task=f.wait(lambda:next((t for t in tasks() if t['state']=='review'),None),'Worker result via native tool')
    first_session=task['session_id'];assert task['result_text']=='Worker delivered via dynamic tool'
    with sqlite3.connect(root/'state/station.sqlite') as db:
        body['request_id']=db.execute('SELECT request_id FROM worker_tasks WHERE session_id=?',(first_session,)).fetchone()[0]
    participants=req('GET',f'/v1/im/sessions/{first_session}/status')[1]['items']
    assert [p['id'] for p in participants]==['leader','worker'],participants
    assert [p['session_id'] for p in participants]==[session,first_session],participants
    assert all('session_key' not in p for p in participants)
    assert [p['id'] for p in req('GET',f'/v1/im/sessions/{session}/status')[1]['items']]==['leader']

    assert req('POST','/v1/agent/tasks',body,headers)[1]['session_id']==first_session
    assert req('POST','/v1/agent/tasks',dict(body,goal='different'),headers)[0]==409
    two=req('POST','/v1/agent/tasks',dict(body,request_id='assignment-two'),headers);assert two[0]==200,two
    second_session=two[1]['session_id'];assert first_session!=second_session
    f.wait(lambda:len([t for t in tasks() if t['state']=='review'])==2,'second Worker completes')
    assert len({t['workspace'] for t in tasks()})==2
    worker_key=req('GET',f'/v1/tools/context?threadId={first_session}')[1]['sessionKey']
    assert req('GET','/v1/agent/workers',headers={'x-zork-session-key':worker_key})[0]==403
    with sqlite3.connect(root/'state/station.sqlite') as db:
        f.wait(lambda:db.execute('SELECT COUNT(*) FROM leader_notifications WHERE delivered=1').fetchone()[0]==2,'Leader receives durable notifications')
        before=db.execute('SELECT session_id FROM worker_tasks ORDER BY request_id').fetchall()
        db.execute("UPDATE worker_tasks SET state='sending' WHERE request_id=?",(body['request_id'],))
        db.execute("UPDATE leader_notifications SET delivered=0")
    stop();start()
    assert admin('POST','/v1/node/agents/leader/open',{})['session_id']==session
    assert req('POST','/v1/agent/tasks',body,headers)[1]['session_id']==first_session
    assert len(tasks())==2,'restart created duplicate Task or promoted Leader to Task'
    time.sleep(2)
    assert all(t['run_count']==1 for t in tasks()),'replayed mailbox input executed twice'
    current=next(t for t in tasks() if t['session_id']==first_session)
    revised_goal=json.dumps({'fake_tool':{'name':'chat.post_message','input':{'text':'Reworked in the original Worker Session'}}})
    rework={'request_id':'revision-one','expected_revision':current['revision'],'goal':revised_goal}
    route='/v1/agent/tasks/'+current['task_id']+'/rework'
    assert req('POST',route,rework,headers)[0]==200
    assert req('POST',route,rework,headers)[0]==200
    f.wait(lambda:any(t['result_text']=='Reworked in the original Worker Session' for t in tasks()),'same-session rework')
    assert len(tasks())==2
    assert next(t for t in tasks() if t['task_id']==current['task_id'])['session_id']==first_session
    with sqlite3.connect(root/'state/station.sqlite') as db:assert before==db.execute('SELECT session_id FROM worker_tasks ORDER BY request_id').fetchall()
    assert all(t['session_id']!=session for t in tasks())
    result={'root':str(root),'leader_session':session,'worker_sessions':[first_session,second_session],'checks':['dynamic assign','dynamic visible reply','Leader file without Task','scoped worker catalog','worker cannot dispatch','one Leader session under concurrency','one isolated session per Task','idempotent assignment','durable notifications','restart preserves bindings','no zork-call runtime dependency','lost acknowledgements replay once','rework keeps Worker Session']}
    artifacts=f.ROOT/'artifacts/leader-worker';artifacts.mkdir(parents=True,exist_ok=True);(artifacts/'result.json').write_text(json.dumps(result,indent=2))
    print('PASS: '+', '.join(result['checks']),flush=True)
finally:stop();log.close();print(root,flush=True)
