#!/usr/bin/env python3
"""Real two-Station device -> MCP -> managed-skill workflow with a fake model."""
import base64
import importlib.util
import json
import os
import signal
from pathlib import Path
import shlex
import shutil
import sqlite3
import subprocess
import sys
import tempfile
import time
import threading

ROOT=Path(__file__).resolve().parents[1]
spec=importlib.util.spec_from_file_location('mcp_fixture',ROOT/'scripts/test-mcp.py')
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);f=m.f


def main():
    root=Path(tempfile.mkdtemp(prefix='zork-node-tools-'));print('fixture:',root,flush=True)
    nodes=[];success=False;checks=[]
    try:
        a,b=f.Node(root/'a'),f.Node(root/'b');nodes=[a,b]
        a.pair(b);b.pair(a)
        for node in nodes:
            node.config['mesh']['peers'][0]['client']=True
            node.config['admin']={'token':'mcp-fixture'}
            (node.root/'config.json').write_text(json.dumps(node.config))
            node.request=lambda method,path,body=None,n=node:m.request(n,method,path,body)
            node.start()
        for node in nodes:
            f.wait(lambda n=node:n.request('GET','/readyz')[0]==200,'ready')
            f.wait(lambda n=node:n.get('/v1/mesh').get('origin')==n.origin,'Mesh identity')
        def ok(result):
            assert result[0] in (200,201,202),result
            return result[1]
        session=a.new_task()['session_id'];counter=0
        executions={}
        def events(node,sid):
            out=[]
            execution=executions.get((node.root,sid),sid)
            for segment in (node.root/'shared-files/sessions'/execution/'segments').glob('*.jsonl'):
                for line in segment.read_text().splitlines():
                    event=json.loads(line).get('event',{})
                    if event.get('kind')=='tool_result':out.append(event['result'])
            return out
        def all_events(node,sid):
            out=[]
            execution=executions.get((node.root,sid),sid)
            for segment in (node.root/'shared-files/sessions'/execution/'segments').glob('*.jsonl'):
                for line in segment.read_text().splitlines():
                    try:out.append(json.loads(line).get('event',{}))
                    except json.JSONDecodeError:pass
            return out
        def dispatch(name,args,node=a,sid=session):
            nonlocal counter
            counter+=1
            before={i['invocation_id'] for e in all_events(node,sid) for i in e.get('invocations',[])}
            content=json.dumps({'fake_tools':[{'name':name,'input':args}]})
            ok(m.request(node,'POST',f'/v1/im/sessions/{sid}/messages',{'content':content,'request_id':f'node-tools-{counter}'}))
            def started():
                return next((i for e in all_events(node,sid) for i in e.get('invocations',[]) if i['invocation_id'] not in before and i['tool']==name),None)
            return f.wait(started,'Agent dispatch '+name)
        def result_for(invocation,node=a,sid=session,outcome='succeeded'):
            def completed():
                for result in events(node,sid):
                    if result['invocation_id']==invocation['invocation_id']:
                        assert result['outcome']==outcome,result
                        return result
            return f.wait(completed,'Agent completion '+invocation['tool'])
        def agent(name,args,node=a,sid=session,failed=False):
            return result_for(dispatch(name,args,node,sid),node,sid,'failed' if failed else 'succeeded')
        def raw(name,args,iid=None,sid=session):
            nonlocal counter
            counter+=1
            return m.request(a,'POST','/v1/node-tools',{'session_id':sid,'invocation_id':iid or f'raw-{counter}','tool':name,'arguments':args})
        def terminal(receipt):
            value=ok(raw('device.status',{'operation_id':receipt['operation_id']}))
            return value if value['state'] not in ('accepted','dispatching','running') else None
        def settle(event,expected='succeeded'):
            assert event['data']['state']==expected,event
            assert 'operation_id' not in event['data'],event
            return event['data'].get('result')
        def receipt_for(invocation):
            with sqlite3.connect(a.root/'state/node-tools.sqlite') as db:
                row=db.execute('SELECT id FROM outbox WHERE invocation=?',(invocation['invocation_id'],)).fetchone()
            return {'operation_id':row[0]} if row and row[0] else None

        local_dir=a.workspace/'cwd-probe';local_dir.mkdir()
        agent('shell.run',{'command':'printf %s "$ZORK_TOOL_PROBE" > value.txt','cwd':'cwd-probe','env':{'ZORK_TOOL_PROBE':'local-env'}})
        assert (local_dir/'value.txt').read_text()=='local-env'
        invalid=agent('shell.run',{'command':'touch invalid-target-ran','target':42},failed=True)
        assert not (a.workspace/'invalid-target-ran').exists()
        checks.append('local_shell_cwd_environment_and_invalid_target_validation')
        listed=agent('device.list',{})['data'];assert len(listed['targets'])==2,listed
        attached=ok(m.request(a,'POST','/v1/services',{'session_id':session,'action':'attach','name':'Inventory-only external service','port':9,'request_id':'inventory-attach'}))
        service_inventory=ok(m.request(a,'GET','/v1/node/resources'))
        service=next(v for v in service_inventory['items'] if v['kind']=='service' and v['name']=='Inventory-only external service')
        assert service['status']=='external' and service['scope']=='shared' and service['owner_session']==session and service['url'].startswith('zork://service/'),service
        assert agent('device.inspect',{'target':b.origin})['data']['commands']['python3']
        skill_content='---\nname: echo-guide\ndescription: Use the shared echo MCP for the requested task\n---\nUse mcp.search, mcp.inspect and mcp.call. Read scripts/helper.py when needed.\n'
        prep="from pathlib import Path\nimport os\np=Path.cwd()\n(p/'mcp-server.py').write_text("+repr(m.FIXTURE)+")\n(p/'guide/scripts').mkdir(parents=True,exist_ok=True)\n(p/'guide/SKILL.md').write_text("+repr(skill_content)+")\n(p/'guide/scripts/helper.py').write_text('print(42)\\n')\nwith (p/'prep-count').open('a') as f:f.write('once\\n')\nprint('REMOTE-PREPARED')"
        command=shlex.quote(sys.executable)+' -c '+shlex.quote(prep)
        exec_args={'target':b.origin,'command':command}
        executed=agent('shell.run',exec_args);prepared=settle(executed);cwd=Path(prepared['cwd']);assert str(b.root) in str(cwd),prepared
        assert (cwd/'prep-count').read_text()=='once\n'
        retry=ok(raw('device.exec',exec_args,iid=executed['invocation_id']));assert retry['operation_id']==receipt_for(executed)['operation_id']
        assert (cwd/'prep-count').read_text()=='once\n'
        assert 'read_with' not in executed['data']['result']
        assert 'REMOTE-PREPARED' in executed['data']['output']
        assert 'REMOTE-PREPARED' in json.dumps(agent('file.read',{'path':executed['data']['output_path']})['data'])
        other=a.new_task()['session_id'];assert raw('device.status',{'target':b.origin,'operation_id':retry['operation_id']},sid=other)[0]==400
        checks.append('device_target_exec_dedup_output_and_session_ownership')

        cancel_pid_file=cwd/'cancel-child.pid'
        code=("import subprocess,sys,time;from pathlib import Path;"
              "child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(120)']);"
              f"Path({str(cancel_pid_file)!r}).write_text(str(child.pid));"
              "print('CANCEL-READY',flush=True);time.sleep(120)")
        running=dispatch('shell.run',{'target':b.origin,'command':shlex.quote(sys.executable)+' -u -c '+shlex.quote(code)})
        f.wait(lambda:cancel_pid_file.exists(),'child process starts')
        assert not any(r['invocation_id']==running['invocation_id'] for r in events(a,session)), 'exec completed before process exit'
        log_path=a.workspace/'.zork'/f"live-{running['invocation_id']}.log"
        f.wait(lambda:log_path.exists() and 'CANCEL-READY' in log_path.read_text(),'remote live output arrives')
        agent('file.read',{'path':str(log_path)})
        cancellation=agent('tool.cancel',{'invocation_id':running['invocation_id']})
        persisted=events(a,session)
        assert next(i for i,r in enumerate(persisted) if r['invocation_id']==running['invocation_id']) < next(i for i,r in enumerate(persisted) if r['invocation_id']==cancellation['invocation_id'])
        terminal_cancel=result_for(running,outcome='cancelled')['data']
        assert cancellation['data']['target_outcome']=='cancelled' and cancellation['data']['target_result']==terminal_cancel,cancellation
        assert terminal_cancel['state']=='cancelled' and terminal_cancel['result']['process_state']=='exited',terminal_cancel
        assert terminal_cancel['result']['effects_may_have_occurred'] is True
        for pid in [terminal_cancel['result']['pid'],int(cancel_pid_file.read_text())]:
            assert subprocess.run(['ps','-p',str(pid),'-o','pid='],capture_output=True).returncode!=0,'completion preceded process cleanup'
        assert 'CANCEL-READY' in terminal_cancel['output']
        failure=agent('shell.run',{'target':b.origin,'command':'echo FAILURE-OUTPUT; exit 7'},failed=True)['data']
        assert failure['state']=='failed' and failure['result']['exit_code']==7,failure
        assert 'FAILURE-OUTPUT' in failure['output']
        noisy=agent('shell.run',{'target':b.origin,'command':"python3 -c \"print('x'*100000);print('END-OF-OUTPUT')\"; exit 9"},failed=True)['data']
        assert noisy['result']['exit_code']==9 and noisy['result']['process_state']=='exited',noisy
        assert noisy['output'].endswith('END-OF-OUTPUT\n') and len(noisy['output'])<=65536
        assert (a.workspace/noisy['output_path']).stat().st_size>100000
        checks.append('ordinary_completion_live_file_standard_cancel_and_process_reaping')

        shutdown=ok(raw('device.exec',{'target':b.origin,'command':'echo SHUTDOWN-READY; sleep 120'}))
        f.wait(lambda:(v if (v:=ok(raw('device.status',{'operation_id':shutdown['operation_id']})))['state']=='running' else None),'shutdown job starts')
        b.stop();b.start()
        f.wait(lambda:b.get('/v1/mesh').get('origin')==b.origin,'executor restarts after graceful shutdown')
        restored=f.wait(lambda:(v[1] if (v:=raw('device.status',{'operation_id':shutdown['operation_id']}))[0]==200 else None),'shutdown receipt readable')
        assert restored['state']=='cancelled' and restored['result']['process_state']=='exited',restored
        checks.append('graceful_station_shutdown_waits_for_device_process_termination')

        config={'name':'echo-mcp','transport':{'kind':'stdio','command':'python3','args':[str(cwd/'mcp-server.py'),str(cwd/'calls')]}}
        installed=agent('mcp.install',{'target':b.origin,'config':config})['data'];server_id=installed['server_id'];assert installed['target']==b.origin
        listed_servers=agent('mcp.list',{'target':b.origin})['data']['items']
        assert any(v['server'].get('server_id')==server_id and v['server'].get('target')==b.origin for v in listed_servers),listed_servers
        assert all('server_ref' not in v['server'] for v in listed_servers),listed_servers
        inventory=ok(m.request(b,'GET','/v1/node/resources'))
        visible_server=next(v for v in inventory['items'] if v['kind']=='mcp' and v['id']==server_id)
        assert visible_server['scope']=='mesh' and 'transport' not in json.dumps(visible_server),visible_server
        assert 'env' not in visible_server and 'command' not in visible_server and 'secret_env' not in visible_server,visible_server
        assert m.request(b,'GET','/v1/node/resources',auth=False)[0]==401
        found=agent('mcp.search',{'target':b.origin})['data']['items']
        assert any(v.get('server_id')==server_id and v.get('target')==b.origin for v in found),found
        assert all('server_ref' not in v for v in found),found
        invalid=agent('mcp.inspect',{'target':b.origin,'server_id':server_id,'tool_name':'echo'},failed=True)['data']
        assert invalid['state']=='not_dispatched' and 'server_ref' not in invalid['error'],invalid
        invalid_type=agent('mcp.inspect',{'target':b.origin,'server_id':42},failed=True)['data']
        assert invalid_type['state']=='not_dispatched' and 'Allowed fields:' in invalid_type['error'] and 'server_ref' not in invalid_type['error'],invalid_type
        assert agent('mcp.inspect',{'target':b.origin,'server_id':server_id})['data']['items'][0]['name']=='echo'
        definition=agent('mcp.inspect',{'target':b.origin,'server_id':server_id,'tool':'echo'})['data']['items'][0]
        called=agent('mcp.call',{'target':b.origin,'server_id':server_id,'tool':'echo','binding_revision':definition['binding_revision'],'arguments':{'text':'workflow'}})['data']
        assert called['state']=='succeeded' and called['result']['content'][0]['text']=='workflow',called
        assert 'operation_id' not in called and 'call_id' not in called,called
        large=agent('mcp.call',{'target':b.origin,'server_id':server_id,'tool':'echo','binding_revision':definition['binding_revision'],'arguments':{'text':'large-output','large':True}})['data']
        assert len(json.dumps(large))<65536 and large['output_path'],large
        full=json.loads((a.workspace/large['output_path']).read_text())
        assert full['content'][0]['text']=='large-output'*100000
        delayed=dispatch('mcp.call',{'target':b.origin,'server_id':server_id,'tool':'echo','binding_revision':definition['binding_revision'],'arguments':{'text':'CANCEL-MCP','delay':120}})
        f.wait(lambda:'CANCEL-MCP' in (cwd/'calls').read_text(),'MCP upstream dispatch')
        cancellation=agent('tool.cancel',{'invocation_id':delayed['invocation_id']})
        stopped_mcp=result_for(delayed,outcome='failed')['data']
        assert cancellation['data']['target_outcome']=='failed' and cancellation['data']['target_result']==stopped_mcp,cancellation
        assert stopped_mcp['state']=='outcome_unknown' and stopped_mcp['result']['process_state']=='exited',stopped_mcp
        assert stopped_mcp['result']['effects_may_have_occurred'] is True,stopped_mcp
        assert 'call_id' not in stopped_mcp and 'operation_id' not in stopped_mcp
        policy_call=dispatch('mcp.call',{'target':b.origin,'server_id':server_id,'tool':'echo','binding_revision':definition['binding_revision'],'arguments':{'text':'POLICY-DISABLE','delay':120}})
        f.wait(lambda:'POLICY-DISABLE' in (cwd/'calls').read_text(),'MCP dispatch before disable')
        current=agent('mcp.inspect',{'target':b.origin,'server_id':server_id})['data']
        disabled=agent('mcp.update',{'target':b.origin,'server_id':server_id,'expected_revision':current['config_revision'],'config':{'enabled':False}})['data']
        interrupted=result_for(policy_call,outcome='failed')['data']
        assert interrupted['state']=='outcome_unknown' and interrupted['result']['error']=='mcp_disabled' and interrupted['result']['process_state']=='exited',interrupted
        disabled_details=agent('mcp.inspect',{'target':b.origin,'server_id':server_id})['data']
        assert disabled_details['availability']=='disabled' and disabled_details['config']['enabled'] is False,disabled_details
        assert disabled_details['config']['transport']==current['config']['transport'],disabled_details
        rejected=agent('mcp.call',{'target':b.origin,'server_id':server_id,'tool':'echo','binding_revision':definition['binding_revision'],'arguments':{'text':'MUST-NOT-DISPATCH'}},failed=True)['data']
        assert rejected['state']=='not_dispatched' and 'mcp_disabled' in rejected['error'],rejected
        assert 'MUST-NOT-DISPATCH' not in (cwd/'calls').read_text()
        enabled=agent('mcp.update',{'target':b.origin,'server_id':server_id,'expected_revision':disabled['config_revision'],'config':{'enabled':True}})['data']
        definition=agent('mcp.inspect',{'target':b.origin,'server_id':server_id,'tool':'echo'})['data']['items'][0]
        changing=dispatch('mcp.call',{'target':b.origin,'server_id':server_id,'tool':'echo','binding_revision':definition['binding_revision'],'arguments':{'text':'POLICY-REVISION','delay':120}})
        f.wait(lambda:'POLICY-REVISION' in (cwd/'calls').read_text(),'MCP dispatch before revision update')
        current=agent('mcp.inspect',{'target':b.origin,'server_id':server_id})['data']
        changed_config={'description':'Revised while running'}
        agent('mcp.update',{'target':b.origin,'server_id':server_id,'expected_revision':current['config_revision'],'config':changed_config})
        changed=result_for(changing,outcome='failed')['data']
        assert changed['state']=='outcome_unknown' and changed['result']['error']=='mcp_definition_changed' and changed['result']['process_state']=='exited',changed
        current=agent('mcp.inspect',{'target':b.origin,'server_id':server_id})['data']
        restricted={'tool_allowlist':[]}
        restricted_result=agent('mcp.update',{'target':b.origin,'server_id':server_id,'expected_revision':current['config_revision'],'config':restricted})['data']
        excluded=agent('mcp.call',{'target':b.origin,'server_id':server_id,'tool':'echo','binding_revision':definition['binding_revision'],'arguments':{'text':'EXCLUDED-NO-DISPATCH'}},failed=True)['data']
        assert excluded['state']=='not_dispatched' and 'mcp_tool_not_allowed' in excluded['error'],excluded
        assert 'EXCLUDED-NO-DISPATCH' not in (cwd/'calls').read_text()
        restricted['tool_allowlist']=None
        agent('mcp.update',{'target':b.origin,'server_id':server_id,'expected_revision':restricted_result['config_revision'],'config':restricted})
        checks.append('named_mcp_install_and_call_after_remote_dependency_preparation')

        old=b.root/'existing-skill';old.mkdir();(old/'SKILL.md').write_text('---\nname: existing-guide\ndescription: Preserve this existing skill\n---\nKeep this skill.\n')
        ok(m.request(b,'POST','/v1/node/agents',{'id':'research','name':'Research','role':'leader','profile_id':'fixture','model':'fixture-model','thinking':'off','skill_paths':[str(old)]}))
        opened=ok(m.request(b,'POST','/v1/node/agents/research/open',{}));research=opened['chat_id']
        executions[(b.root,research)]=opened['agent']['session_id']
        agents=agent('agent.list',{'target':b.origin})['data']['items'];assert any(v['id']=='research' for v in agents),agents
        current=agent('agent.inspect',{'target':b.origin,'agent_id':'research'})['data']
        agent('agent.update',{'target':b.origin,'agent_id':'research','expected_revision':current['revision'],'changes':{'skill_paths':[str(old),str(cwd/'guide')]}})
        catalog=ok(m.request(b,'GET','/v1/node/agents/research/skills'))['catalog']
        assert {'echo-guide','existing-guide'} <= {v['name'] for v in catalog['skills']},catalog
        assert 'echo-guide' in agent('file.read',{'path':str(cwd/'guide/SKILL.md')},node=b,sid=research)['data']['content']
        assert (cwd/'guide/scripts/helper.py').read_text() == 'print(42)\n'
        checks.append('ordinary_skill_files_and_agent_configuration_preserve_existing_sources')
        b.config['mesh']['peers'][0].update(client=False,collaborate=False,execute=[])
        (b.root/'config.json').write_text(json.dumps(b.config))
        assert agent('device.inspect',{'target':b.origin})['data']['commands']['python3']
        assert 'TRUSTED-MEMBER' in agent('shell.run',{'target':b.origin,'command':'echo TRUSTED-MEMBER'})['data']['output']
        assert agent('mcp.inspect',{'target':b.origin,'server_id':server_id,'tool':'echo'})['data']['items']
        checks.append('mesh_members_need_no_additional_client_or_collaboration_grants')
        occupied=[ok(raw('device.exec',{'target':b.origin,'command':'sleep 120'},iid=f'occupied-{i}')) for i in range(8)]
        queue_args={'target':b.origin,'command':'touch queued-must-not-run'}
        queued=[]
        waiter=threading.Thread(target=lambda:queued.append(raw('device.exec',queue_args,iid='queued-cancel')),daemon=True);waiter.start()
        time.sleep(.3);assert not queued,'full node failed instead of applying backpressure'
        interrupted=ok(m.request(a,'POST','/v1/node-tools/interrupt',{'session_id':session,'invocation_id':'queued-cancel','tool':'device.exec','arguments':queue_args}))
        assert interrupted['state']=='cancelled' and interrupted['result']['process_state']=='not_started',interrupted
        for value in occupied:ok(raw('device.cancel',{'operation_id':value['operation_id']}))
        waiter.join(timeout=10);assert queued, 'queue did not drain'
        assert not (cwd/'queued-must-not-run').exists()
        for value in occupied:f.wait(lambda value=value:terminal(value),'occupied command cleanup')
        assert raw('device.exec',queue_args,iid='queued-cancel')[0]==400
        checks.append('capacity_wait_and_cancelled_queue_never_launches_late')

        crash_file=cwd/'crash-once'
        code=f"from pathlib import Path;import time;f=Path({str(crash_file)!r}).open('a');f.write('once\\n');f.close();time.sleep(20)"
        crash_args={'target':b.origin,'command':'python3 -c '+shlex.quote(code)}
        crashing={'data':ok(raw('device.exec',crash_args,iid='crash-once')),'invocation_id':'crash-once'}
        f.wait(lambda:crash_file.exists(),'crash command dispatched')
        running=ok(raw('device.status',{'operation_id':crashing['data']['operation_id']}))
        pid=running['result']['pid']
        b.restart_station();f.wait(lambda:b.get('/v1/mesh').get('origin')==b.origin,'executor Mesh restored')
        unknown=ok(raw('device.status',{'operation_id':crashing['data']['operation_id']}));assert unknown['state']=='outcome_unknown',unknown
        same=ok(raw('device.exec',crash_args,iid=crashing['invocation_id']));assert same['operation_id']==unknown['operation_id'] and same['state']=='outcome_unknown',same
        assert crash_file.read_text()=='once\n'
        command_line=subprocess.run(['ps','-p',str(pid),'-o','command='],capture_output=True,text=True).stdout
        if command_line:
            assert str(crash_file) in command_line,command_line
            os.killpg(pid,signal.SIGKILL)
        checks.append('device_crash_leaves_unknown_receipt_without_replay')

        print(json.dumps({'checks':checks,'count':len(checks)},indent=2),flush=True);success=True
    finally:
        for node in reversed(nodes):node.stop()
        if success:shutil.rmtree(root)

if __name__=='__main__':main()
