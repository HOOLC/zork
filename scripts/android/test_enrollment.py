#!/usr/bin/env python3
"""Real isolated Station + client-core phone enrollment; no production state."""
import importlib.util,json,os,subprocess,tempfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('fixture',ROOT/'scripts/test-mesh.py')
fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)
root=Path(tempfile.mkdtemp(prefix='zork-phone-enrollment-'))
node=fixture.Node(root/'station')
node.config['admin']={'token':'isolated-phone-enrollment'}
node.config['mesh']['bind']='0.0.0.0:'+str(node.udp)
(node.root/'profiles/fixture.json').unlink()
(node.root/'config.json').write_text(json.dumps(node.config))
with (root/'station.log').open('wb') as log:
 process=subprocess.Popen([str(fixture.TARGET/'zork-station'),'--data',str(node.root)],stdout=log,stderr=log)
 try:
  fixture.wait(lambda:node.get('/v1/mesh').get('origin'),'Station ready')
  env={**os.environ,'ZORK_ENROLLMENT_URL':node.url,'ZORK_ENROLLMENT_TOKEN':'isolated-phone-enrollment'}
  subprocess.run(['cargo','test','--locked','-p','zork-client-core','--test','client_enrollment','--','--ignored','--nocapture'],cwd=ROOT,env=env,check=True)
 finally:
  process.terminate();process.wait(timeout=30)
  print('Isolated evidence:',root,flush=True)
