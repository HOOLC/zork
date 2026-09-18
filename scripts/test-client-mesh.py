#!/usr/bin/env python3
"""One real node plus a transport-only client; no client Station/Agent."""
import importlib.util,json,os,subprocess,tempfile
from pathlib import Path
spec=importlib.util.spec_from_file_location('fixture',Path(__file__).with_name('test-mesh.py'));f=importlib.util.module_from_spec(spec);spec.loader.exec_module(f)
root=Path(tempfile.mkdtemp(prefix='zmc-',dir='/tmp'));node=f.Node(root/'node');client_fixture=f.Node(root/'client');client=client_fixture.root
origin=client_fixture.origin
client_udp=client_fixture.udp;node.config['admin']={'token':'mesh-client-fixture'}
node.config['mesh']['peers']=[{'origin':origin,'name':'client','addr':f'127.0.0.1:{client_udp}','execute':[],'client':True}]
relay=os.environ.get('ZORK_TEST_RELAY');discovery=os.environ.get('ZORK_TEST_DISCOVERY')
if relay:
    node.config['mesh'].update(offline=False,relay_urls=[relay],discovery_url=discovery)
    node.config['mesh']['peers'][0].pop('addr')
(node.root/'config.json').write_text(json.dumps(node.config))
(client/'config.json').write_text(json.dumps({'mesh':{'enabled':True,'offline':True,'bind':f'127.0.0.1:{client_udp}','peers':[{'origin':node.origin,'name':'node','addr':f'127.0.0.1:{node.udp}','execute':[]}],'workspaces':[]}}))
if relay:
    config=json.loads((client/'config.json').read_text());config['mesh'].update(offline=False,relay_urls=[relay],discovery_url=discovery);config['mesh']['peers'][0].pop('addr');(client/'config.json').write_text(json.dumps(config))
try:
    node.start();f.wait(lambda:node.request('GET','/readyz')[0]==200,'node ready')
    from urllib.request import urlopen
    f.wait(lambda:urlopen(node.agent_url+'/readyz',timeout=2).status==200,'node Agent ready')
    env=dict(os.environ,ZORK_TEST_CLIENT_ROOT=str(client),ZORK_TEST_REMOTE_ORIGIN=node.origin,ZORK_TEST_REMOTE_CONFIG=str(node.root/'config.json'),CARGO_INCREMENTAL='0',CARGO_PROFILE_DEV_DEBUG='0',CARGO_BUILD_JOBS='4')
    subprocess.run(['cargo','test','--locked','-p','zork-gui','--test','client_mesh','--','--ignored','--nocapture'],cwd=f.ROOT,env=env,check=True)
    assert not (client/'run/zork-agent.pid').exists() and not (client/'run/zork-station.pid').exists()
    assert not (client/'client-mesh-ready.json').exists()
    print('PASS: in-process desktop transport, remote Leader tools and artifact transfer, client grants, forbidden routes, revocation, owned lifecycle')
finally:
    node.stop();print(root)
