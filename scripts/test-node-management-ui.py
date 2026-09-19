#!/usr/bin/env python3
"""Native Profile → Leader → Worker → Home flow with an isolated local node."""
import importlib.util,json,os,sqlite3,subprocess,tempfile,time
from pathlib import Path
from urllib.request import Request,urlopen
spec=importlib.util.spec_from_file_location('ui',Path(__file__).parent / 'lib/native_gui_fixture.py');ui=importlib.util.module_from_spec(spec);spec.loader.exec_module(ui)
root=Path(tempfile.mkdtemp(prefix='znui-',dir='/tmp'));client=root/'client';native=ui.Native(None,root)
artifacts=ui.fixture.ROOT/'artifacts/leader-worker';artifacts.mkdir(parents=True,exist_ok=True)
def leader_element(agent_id):
    with sqlite3.connect(client/'client.db') as db:
        saved=json.loads(db.execute('SELECT value FROM nodes').fetchone()[0])
    return 'leader-'+saved['id']+'-'+agent_id
def request(path):
    config=json.loads((client/'node/config.json').read_text());return json.load(urlopen(Request('http://'+config['bind']['runtime']+path,headers={'Authorization':'Bearer '+config['admin']['token']}),timeout=5))
def scroll(delta):native.ui('/v1/actions',{'type':'scroll','target':{'x':700,'y':480},'delta_y':delta});time.sleep(.1)
def visible(id):
    for _ in range(18):
        element=native.element(id)
        if element and element['visible_bounds']['height']>=8:return element
        scroll(-170)
    raise AssertionError('not visible: '+id)
def fill(id,text):visible(id);native.click(id);native.type(text)
def click(id):visible(id);native.click(id)
try:
    env=dict(os.environ,ZORK_CLIENT_DATA=str(client),ZORK_NODE_BINARY=str(ui.fixture.TARGET/'zork'),ZORK_DESKTOP_FAKE_AGENT='1')
    native.process=subprocess.Popen([str(ui.fixture.TARGET/'zork-gui'),'--dev','--dev-port',native.url.rsplit(':',1)[1],'--dev-token','mesh-native-fixture'],env=env,stdout=native.log,stderr=native.log)
    ui.wait(lambda:native.element('local-node-toggle',True),'empty desktop');click('local-node-toggle');ui.wait(lambda:native.element('desktop-manage'),'node enabled')
    click('desktop-manage');ui.wait(lambda:native.element('profile-provider-2'),'provider catalog');click('profile-provider-2')
    fill('profile-id','native-fixture');fill('profile-base-url','http://127.0.0.1:9/v1');fill('profile-model','fixture-model');fill('profile-key','fixture-key-must-be-masked')
    native.screenshot(artifacts/'profile-form.png');click('profile-save')
    ui.wait(lambda:any(p['profile_id']=='native-fixture' for p in request('/v1/im/profiles')['items']),'Profile saved')
    scroll(2000);click('manage-tab-1');ui.wait(lambda:native.element('agent-profile-0'),'Agent profile options')
    fill('agent-name','本机 Leader');click('agent-avatar-bunny');assert '已选择' in native.element('agent-avatar-bunny')['label'];native.screenshot(artifacts/'avatar-picker-create.png');click('agent-create');leader=ui.wait(lambda:next((a for a in request('/v1/node/agents')['items'] if a['role']=='leader'),None),'Leader saved')
    assert leader['avatar']=='bunny'
    scroll(2000);click('edit-avatar-'+leader['id']);assert '已选择' in native.element('agent-avatar-bunny')['label']
    click('agent-avatar-panda');click('agent-avatar-cancel');assert request('/v1/node/agents')['items'][0]['avatar']=='bunny'
    click('edit-avatar-'+leader['id']);click('agent-avatar-fox');native.screenshot(artifacts/'avatar-picker-edit.png');click('agent-avatar-save')
    ui.wait(lambda:request('/v1/node/agents')['items'][0]['avatar']=='fox','avatar saved')
    ui.wait(lambda:not native.element('agent-avatar-save'),'avatar editor closed')
    click('edit-avatar-'+leader['id']);assert '已选择' in native.element('agent-avatar-fox')['label'];click('agent-avatar-cancel')
    scroll(2000);click('agent-add');click('agent-role-worker');fill('agent-name','检查 Worker');click('agent-avatar-bear');click('agent-grant-'+leader['id']);click('agent-create')
    worker=ui.wait(lambda:next((a for a in request('/v1/node/agents')['items'] if a['role']=='worker'),None),'Worker saved');assert worker['allowed_leaders']==[leader['id']];assert worker['avatar']=='bear'
    native.screenshot(artifacts/'agents-settings.png');click('manage-tab-2');ui.wait(lambda:native.element('node-mesh-toggle'),'device controls');native.screenshot(artifacts/'device-settings.png');click('manage-tab-4');native.screenshot(artifacts/'account-settings.png');click('manage-tab-3');native.screenshot(artifacts/'node-settings.png');click('desktop-return')
    ui.wait(lambda:native.element(leader_element(leader['id']),True),'Leader visible on home');assert not native.element(leader_element(worker['id']))
    native.screenshot(artifacts/'leaders-home.png');click(leader_element(leader['id']));ui.wait(lambda:native.element('composer-input'),'Leader conversation')
    fill('composer-input',json.dumps({'fake_tool':{'name':'chat.post_message','input':{'text':'原生客户端已连接到固定 Leader 会话。'}}},ensure_ascii=False))
    # Existing conversation submit button uses the send-message automation ID.
    elements=json.loads(native.ui('/v1/elements'))['elements'];(artifacts/'conversation-elements.json').write_text(json.dumps(elements,ensure_ascii=False,indent=2))
    native.ui('/v1/actions',{'type':'key','keystroke':'enter'})
    ui.wait(lambda:any(m['role']=='assistant' for m in request('/v1/im/sessions/'+leader['session_id']+'/messages')['items']),'Leader visible transcript')
    assert request('/v1/tasks')['items']==[]
    native.screenshot(artifacts/'leader-conversation.png')
    fill('composer-input','离线草稿必须保留')
    click('desktop-manage');click('manage-tab-3');scroll(2000);click('local-node-toggle')
    ui.wait(lambda:native.element('local-node-toggle',True) and native.element('local-node-toggle')['label']=='开启本机节点','node stopped')
    with sqlite3.connect(client/'client.db') as db:
        saved=json.loads(db.execute('SELECT value FROM nodes').fetchone()[0])
    click('browse-node-'+saved['id']);ui.wait(lambda:native.element(leader_element(leader['id'])),'cached Leader')
    click(leader_element(leader['id']));ui.wait(lambda:native.element('composer-input'),'offline conversation')
    with sqlite3.connect(client/'client.db') as db:
        assert json.loads(db.execute('SELECT value FROM cache WHERE key=?',('draft:'+leader['session_id'],)).fetchone()[0])=='离线草稿必须保留'
    native.screenshot(artifacts/'offline-conversation.png')
    click('composer-input');native.ui('/v1/actions',{'type':'key','keystroke':'enter'})
    def queued():
        with sqlite3.connect(client/'client.db') as db:return db.execute('SELECT count(*) FROM outbox').fetchone()[0]
    ui.wait(lambda:queued()==1,'offline send persisted')
    native.stop()
    native.process=subprocess.Popen([str(ui.fixture.TARGET/'zork-gui'),'--dev','--dev-port',native.url.rsplit(':',1)[1],'--dev-token','mesh-native-fixture'],env=env,stdout=native.log,stderr=native.log)
    ui.wait(lambda:native.element('local-node-toggle',True),'relaunch disabled');assert queued()==1
    click('local-node-toggle');ui.wait(lambda:native.element('desktop-manage'),'node restarted');ui.wait(lambda:queued()==0,'durable outbox delivered')
    persisted={a['id']:a for a in request('/v1/node/agents')['items']};assert persisted[leader['id']]['avatar']=='fox';assert persisted[worker['id']]['avatar']=='bear'
    messages=request('/v1/im/sessions/'+leader['session_id']+'/messages')['items'];assert sum(m['content']=='离线草稿必须保留' for m in messages)==1
    print('PASS: avatar selection, cancel/edit, persisted across restart, native Profile, masked secret, Leader/Worker grant and Home, offline history/draft, outbox survives GUI restart and delivers once')
finally:
    if native.process and native.process.poll() is None:native.screenshot(root/"final.png")
    native.stop();native.log.close();print(root)
