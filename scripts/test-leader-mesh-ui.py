#!/usr/bin/env python3
"""Native client-only → Leader node → remote Worker, using real iroh transport."""
import importlib.util,json,os,subprocess,tempfile,time
from pathlib import Path
from urllib.request import Request,urlopen
spec=importlib.util.spec_from_file_location('ui',Path(__file__).parent / 'lib/native_gui_fixture.py');ui=importlib.util.module_from_spec(spec);spec.loader.exec_module(ui)
f=ui.fixture;root=Path(tempfile.mkdtemp(prefix='zlmui-',dir='/tmp'));a,b=f.Node(root/'leader'),f.Node(root/'worker');a.pair(b);b.pair(a)
client=root/'client';client.mkdir();transport=client/'transport'
client_fixture=f.Node(transport);origin=client_fixture.origin;udp=client_fixture.udp
(transport/'config.json').write_text(json.dumps({'mesh':{'offline':True,'bind':f'127.0.0.1:{udp}'}}))
for node in (a,b):node.config['admin']={'token':'fixture'}
a.config['mesh']['peers'].append({'origin':origin,'name':'Client','addr':f'127.0.0.1:{udp}','client':True,'execute':[]})
for node in (a,b):(node.root/'config.json').write_text(json.dumps(node.config))
print('Native current mesh fixture:', root, flush=True)
native=ui.Native(None,root);art=Path(os.environ.get('ZORK_GUI_SCREENSHOT_DIR', f.ROOT/'artifacts/leader-worker'));art.mkdir(parents=True,exist_ok=True)
def admin(node,method,path,body=None):return json.load(urlopen(Request(node.url+path,method=method,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json','Authorization':'Bearer fixture'}),timeout=15))
def scroll(delta):native.ui('/v1/actions',{'type':'scroll','target':{'x':700,'y':480},'delta_y':delta});time.sleep(.1)
def visible(id):
    for _ in range(18):
        e=native.element(id)
        if e and e['visible_bounds']['height']>=8:return e
        scroll(-150)
    raise AssertionError('not visible: '+id)
def click(id):visible(id);native.click(id)
def fill(id,text):click(id);native.type(text)
try:
    for node in (a,b):node.start()
    for node in (a,b):f.wait(lambda:node.request('GET','/readyz')[0]==200,'Station ready');f.wait(lambda:urlopen(node.agent_url+'/readyz',timeout=2).status==200,'Agent ready')
    selection={'profile_id':'fixture','model':'fixture-model','thinking':'off'}
    leader=admin(a,'POST','/v1/node/agents',dict(selection,id='leader',name='产品 Leader',role='leader'))
    admin(a,'POST','/v1/node/agents',dict(selection,id='engineering',name='开发助手',role='leader'))
    admin(a,'POST','/v1/node/agents',dict(selection,id='research',name='研究助手',role='leader'))
    admin(b,'POST','/v1/node/agents',dict(selection,id='worker',name='远端 Worker',role='worker',allowed_leaders=[a.origin+'/leader']))
    env=dict(os.environ,ZORK_CLIENT_DATA=str(client),ZORK_NODE_BINARY=str(f.TARGET/'zork'),
             ZORK_GUI_PREFERENCES_PATH=str(root/'preferences.json'),ZORK_GUI_LOCALE='zh-CN')
    native.process=subprocess.Popen([str(f.TARGET/'zork-gui'),'--dev','--dev-port',native.url.rsplit(':',1)[1],'--dev-token','mesh-native-fixture'],env=env,stdout=native.log,stderr=native.log)
    f.wait(lambda:native.element('connect-existing-node',True),'zero node client');click('connect-existing-node');f.wait(lambda:native.element('copy-mesh-identity'),'client identity')
    fill('remote-name','Leader 节点');fill('remote-origin',a.origin);fill('remote-addr',f'127.0.0.1:{a.udp}');click('connect-remote')
    f.wait(lambda:native.element('leader-'+a.origin+'-leader',True),'remote Leader home');native.screenshot(art/'remote-leader-home.png');click('leader-'+a.origin+'-leader');f.wait(lambda:native.element('composer-input'),'remote Leader conversation')
    worker_goal='复核最新交付，检查遗漏并整理待处理事项。'
    goal=json.dumps({'fake_tools':[{'name':'agent.assign','input':{'worker_id':b.origin+'/worker','goal':worker_goal}},{'name':'chat.post_message','input':{'text':'已指派给远端 Worker。'}}]},ensure_ascii=False)
    fill('composer-input','帮我检查今天的交付，安排 Worker 做一次复核。');native.ui('/v1/actions',{'type':'key','keystroke':'enter'})
    f.wait(lambda:a.get('/v1/im/sessions/'+leader['session_id']+'/messages')['items'],'visible user delivery')
    def mailbox(node,session,content):
        with urlopen(Request(node.agent_url+'/sessions/'+session+'/mailbox',method='POST',data=json.dumps({'content':content}).encode(),headers={'Content-Type':'application/json'}),timeout=10) as r:assert r.status==202
    mailbox(a,leader['session_id'],goal)
    execution=f.wait(lambda:next((task for task in b.get('/v1/tasks')['items']
                                 if task.get('last_run_status') == 'finished'),None),
                     'remote Worker initial run finished')
    report = Path(execution['workspace']) / 'review.md'
    report.write_text('# 交付复核\n\n交付内容齐全，建议补充一段安装说明。\n')
    mailbox(b, execution['session_id'], json.dumps({'fake_tools': [
        {'name': 'chat.post_file', 'input': {'attachments': [{'file_path': 'review.md'}], 'text': '复核报告'}},
        {'name': 'chat.post_message', 'input': {'text': '检查已完成。交付内容齐全，建议补充一段安装说明。'}},
    ]}, ensure_ascii=False))
    task=f.wait(lambda:next((t for t in a.get('/v1/tasks')['items'] if t['state']=='review'),None),'remote Worker completes',90)
    f.wait(lambda:native.element('leader-task-'+a.origin+'-'+task['task_id']),'Leader related Task card');native.screenshot(art/'remote-leader-tasks.png');click('leader-task-'+a.origin+'-'+task['task_id']);f.wait(lambda:native.element('composer-input') and native.element('header-member-'+b.origin+'/worker'),'remote Worker conversation');native.screenshot(art/'remote-worker-task.png')
    artifact = f.wait(lambda: next((item for item in a.get('/v1/artifacts')['items']
                                  if item['name'] == 'review.md'), None), 'replicated report')
    click('conversation-files-button')
    file_id = 'conversation-artifact-' + artifact['artifact_id']
    f.wait(lambda: native.element(file_id), 'conversation file list')
    click(file_id)
    f.wait(lambda: native.element('drive-save', True), 'immutable report preview')
    native.screenshot(art / 'remote-worker-file.png')
    native.ui('/v1/actions', {'type': 'key', 'keystroke': 'escape'})
    f.wait(lambda: not native.element('drive-save'), 'preview closed by Escape')
    click('conversation-files-button')
    f.wait(lambda: native.element(file_id), 'file list reopens')
    click('composer-input')
    f.wait(lambda: not native.element(file_id), 'file list closed by outside click')
    assert not any(native.element(id) for id in ('rail-inbox', 'rail-drive', 'rail-tasks',
                                                 'start-session', 'task-accept', 'mesh-stop'))
    # The connected Leader owns these execution records; its Worker runs on a different node.
    click('leader-' + a.origin + '-leader')
    f.wait(lambda: native.element('header-member-leader'), 'Leader conversation restored')
    click('header-member-leader')
    f.wait(lambda: native.element('history-close'), 'conversation history panel')
    f.wait(lambda: any(e['id'].startswith('history-record') and e['visible']
                      for e in json.loads(native.ui('/v1/elements'))['elements']),
           'real Leader execution history records')
    native.screenshot(art / 'remote-leader-history.png')
    click('history-close')
    f.wait(lambda: not native.element('history-close'), 'history panel closed')
    assert '当前 领队' in native.element('leader-'+a.origin+'-leader')['label']
    click('leader-'+a.origin+'-engineering');f.wait(lambda:'当前 领队' in native.element('leader-'+a.origin+'-engineering')['label'],'second Leader selected');assert native.element('leader-task-'+a.origin+'-'+task['task_id']), 'Task must remain beneath its own Leader'
    click('leader-'+a.origin+'-leader');f.wait(lambda:native.element('leader-task-'+a.origin+'-'+task['task_id']),'first Leader task restored')
    native.screenshot(art/('leaders-900.png' if os.environ.get('ZORK_GUI_TEST_WINDOW_SIZE') else 'leaders-final.png'))
    assert not (client/'node/config.json').exists() and not (client/'node/zork.pid').exists(), 'client unexpectedly initialized a local node'
    assert not list((client/'node/run').glob('*.pid')), 'client unexpectedly started local node processes'
    ready=json.loads((transport/'client-mesh-ready.json').read_text());assert ready['embedded'] and ready['pid']==native.process.pid;native.stop()
    def gone():
        try:os.kill(ready['pid'],0);return False
        except ProcessLookupError:return True
    f.wait(gone,'client transport stops');assert a.request('GET','/readyz')[0]==200 and b.request('GET','/readyz')[0]==200
    print('PASS: native client-only pairing, remote Leader chat, remote Worker assignment, related Task card, execution history, file preview, client quit leaves remote nodes running')
finally:
    if native.process and native.process.poll() is None:native.screenshot(root/'final.png')
    native.stop();native.log.close();a.stop();b.stop();print(root)
