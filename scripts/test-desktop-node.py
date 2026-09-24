#!/usr/bin/env python3
"""Isolated native lifecycle check. No user profiles or real model requests."""
import importlib.util,json,os,socket,subprocess,tempfile
from pathlib import Path
spec=importlib.util.spec_from_file_location('ui',Path(__file__).parent / 'lib/native_gui_fixture.py')
ui=importlib.util.module_from_spec(spec);spec.loader.exec_module(ui)
if os.environ.get('ZORK_TEST_BIN_DIR'):ui.fixture.TARGET=Path(os.environ['ZORK_TEST_BIN_DIR'])
root=Path(tempfile.mkdtemp(prefix='zd-',dir='/tmp'))
artifacts=Path(__file__).resolve().parents[1]/'artifacts/desktop-node';artifacts.mkdir(parents=True,exist_ok=True)
client=root/'client';native=ui.Native(None,root)
def launch():
    env=dict(os.environ,ZORK_CLIENT_DATA=str(client),ZORK_NODE_BINARY=str(ui.fixture.TARGET/'zork'),ZORK_DESKTOP_FAKE_AGENT='1',ZORK_GUI_PREFERENCES_PATH=str(root/'preferences.json'))
    if os.environ.get('ZORK_TEST_BIN_DIR'):env.pop('ZORK_NODE_BINARY',None)
    native.process=subprocess.Popen([str(ui.fixture.TARGET/'zork-gui'),'--dev','--dev-port',native.url.rsplit(':',1)[1],'--dev-token','mesh-native-fixture'],env=env,stdout=native.log,stderr=native.log)
    ui.wait(lambda:native.ui('/health'),'desktop ready')
    ui.wait(lambda:native.element('local-node-toggle',True),'node toggle ready')
def endpoints_closed(config):
    for binding in config["bind"].values():
        host,port=binding.rsplit(":",1)
        try:
            with socket.create_connection((host,int(port)),timeout=.2):return False
        except OSError:pass
    return True
def alive(pid):
    try:os.kill(pid,0);return True
    except ProcessLookupError:return False
try:
    launch();assert not (client/'node').exists(),'launch must not initialize node'
    native.screenshot(artifacts/'zero-node.png')
    native.click('local-node-toggle');ui.wait(lambda:native.element('desktop-manage'),'node opened')
    config=json.loads((client/'node/config.json').read_text())
    url='http://'+config['bind']['runtime']
    pid=int((client/'node/zork.pid').read_text())
    assert alive(pid)
    native.screenshot(artifacts/'running.png')
    native.click('desktop-manage');ui.wait(lambda:native.element('manage-tab-3',True),'settings navigation');native.click('manage-tab-3');ui.wait(lambda:native.element('device-more',True),'node controls')
    # Stop and start are rare device actions in the page's 更多 menu.
    def device_toggle():
        native.click('device-more');ui.wait(lambda:native.element('device-more-menu-0-local-node-toggle',True),'device menu')
        native.click('device-more-menu-0-local-node-toggle')
    device_toggle();ui.wait(lambda:not alive(pid) and endpoints_closed(config),'explicit stop closes all node listeners')
    ui.wait(lambda:native.element('device-more',True),'toggle reenabled')
    device_toggle();ui.wait(lambda:native.element('desktop-manage'),'node reopened')
    pid=int((client/'node/zork.pid').read_text())
    native.process.kill();native.process.wait();native.process=None
    ui.wait(lambda:not alive(pid) and endpoints_closed(config),'GUI crash closes owned node and all listeners')
    launch();assert native.element('local-node-toggle')['label']=='开启本机节点','restart stays disabled'
    native.screenshot(artifacts/'restart-disabled.png')
    (artifacts/'lifecycle-result.json').write_text(json.dumps({'root':str(root),'checks':['zero process before activation','explicit start','explicit stop','restart','GUI crash stops node','GUI relaunch stays off']},ensure_ascii=False,indent=2))
    print('PASS: zero-node startup, manual start/stop, crash cleanup, restart disabled')
finally:
    native.stop();native.log.close()
    print(root)
