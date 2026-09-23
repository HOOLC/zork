#!/usr/bin/env python3
"""Prove Station owns iroh without a supervisor, daemon, or local control service."""
import importlib.util
import json
import os
import time
from urllib.request import Request, urlopen
from pathlib import Path
import signal
import subprocess
import tempfile

spec = importlib.util.spec_from_file_location('fixture', Path(__file__).with_name('test-mesh.py'))
f = importlib.util.module_from_spec(spec)
spec.loader.exec_module(f)


def main():
    root = Path(tempfile.mkdtemp(prefix='zstation-embed-', dir='/tmp'))
    node = f.Node(root / 'node')  # Also bootstraps with Station alone.
    origin = node.origin
    session_id = None
    shell_pid = None
    def agent(method, route, body=None):
        request = Request(node.agent_url + route, method=method, headers={'Content-Type':'application/json'}, data=None if body is None else json.dumps(body).encode())
        with urlopen(request, timeout=8) as response:
            raw = response.read()
            return json.loads(raw) if raw else None
    for index in range(3):
        with (node.root / f'station-{index}.log').open('wb') as log:
            station = subprocess.Popen([str(f.TARGET / 'zork-station'), '--data', str(node.root), '--fake-agent'],
                                       stdout=log, stderr=log)
            try:
                f.wait(lambda: node.get('/v1/mesh').get('origin') == origin, 'standalone Station iroh ready')
                assert not (node.root / 'zork.pid').exists(), 'Supervisor was required'
                assert not (node.root / 'run/zork-agent.pid').exists(), 'Agent was started'
                assert not (node.root / 'mesh/iroh/control.sock').exists(), 'daemon control socket created'
                assert not (node.root / 'mesh/iroh/control.token').exists(), 'daemon control token created'
                children = subprocess.run(['pgrep', '-P', str(station.pid)], capture_output=True, text=True)
                assert children.returncode == 1 and not children.stdout.strip(), children.stdout
                ready = agent('GET', '/readyz')
                assert ready['embedded'] and ready['pid'] == station.pid
                if session_id is None:
                    session_id = agent('POST','/sessions',{'profile_id':'fixture','model':'fixture-model','thinking':'off','workspace':str(node.workspace)})['session_id']
                    command = "echo $$ > child.pid; printf 'started\\n' >> executions.txt; sleep 60"
                    agent('POST',f'/sessions/{session_id}/mailbox',{'content':json.dumps({'fake_tool':{'name':'shell.run','input':{'command':command}}})})
                    f.wait(lambda: (node.workspace/'child.pid').exists(), 'embedded shell started')
                    shell_pid = int((node.workspace/'child.pid').read_text())
                else:
                    f.wait(lambda: agent('GET',f'/sessions/{session_id}')['session_id'] == session_id, 'embedded session recovered')
                    assert (node.workspace/'executions.txt').read_text().splitlines() == ['started']
                # Fixed bind is reused on each restart; shutdown must release it.
                if index == 1:
                    duplicate = subprocess.run([str(f.TARGET / 'zork-station'), '--data', str(node.root), '--fake-agent'],
                                               capture_output=True, text=True, timeout=20)
                    assert duplicate.returncode != 0, 'duplicate node ownership was accepted'
                    assert node.get('/v1/mesh')['origin'] == origin, 'duplicate stopped the owner'
                station.send_signal(signal.SIGINT if index == 1 else signal.SIGTERM)
                assert station.wait(timeout=30) == 0, 'Station did not drain its iroh tasks'
                assert not (node.root / 'run/zork-station.pid').exists(), 'Station readiness leaked'
                if shell_pid is not None:
                    try: os.kill(shell_pid, 0)
                    except ProcessLookupError: pass
                    else: raise AssertionError('embedded shell survived Station shutdown')
                    shell_pid = None
            finally:
                if station.poll() is None:
                    station.kill()
                    station.wait()
    print(f'PASS: Station owns Agent and iroh; no sidecar/control service; Agent PID equals Station; shell shutdown and session recovery; duplicate exclusion; stable identity and UDP bind across SIGTERM/SIGINT restarts; {root}')


if __name__ == '__main__':
    main()
