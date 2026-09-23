#!/usr/bin/env python3
"""Two real Zork supervisors with isolated iroh identities and fake models.

The Agent executes real shell and registered Station tools; no model network
request or user workspace is used. Keep the temp directory for failure diagnosis.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import sqlite3
import socket
import subprocess
import tempfile
import time
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]
TARGET = Path(os.environ.get('ZORK_TEST_BIN_DIR', str(ROOT / 'target/debug')))


def port(udp=False):
    with socket.socket(type=socket.SOCK_DGRAM if udp else socket.SOCK_STREAM) as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def wait(check, label, timeout=60):
    end = time.monotonic() + timeout
    last = None
    while time.monotonic() < end:
        try:
            result = check()
            if result:
                return result
        except (OSError, URLError, ConnectionError, TimeoutError, KeyError, StopIteration, ValueError) as error:
            last = error
        time.sleep(.15)
    raise AssertionError(f'timed out: {label}; last={last}')


class Node:
    def __init__(self, root):
        self.root = root
        root.mkdir()
        self.workspace = root / 'workspace'
        self.workspace.mkdir()
        (root / 'profiles').mkdir()
        (root / 'profiles/fixture.json').write_text(json.dumps({
            'provider': 'openai', 'billing': 'usage', 'base_url': 'http://127.0.0.1:9/v1',
            'auth': {'type': 'api_key', 'key': 'sk-test'},
            'models': [{'id': 'fixture-model', 'api': 'openai-completions', 'streaming': False,
                        'thinking': ['off'], 'default_thinking': 'off',
                        'capabilities': {'input': ['text']},
                        'limits': {'context_window_tokens': 100000, 'max_output_tokens': 10000}, 'default': True}],
        }))
        self.udp = port(True)
        bindings = {name: f'127.0.0.1:{port()}' for name in ('station', 'runtime', 'control', 'agent')}
        self.url = 'http://' + bindings['runtime']
        self.agent_url = 'http://' + bindings['agent']
        self.config = {'im_connections': [], 'bind': bindings, 'urls': {}, 'admin': {}, 'mesh': {
            'enabled': True, 'offline': True, 'bind': f'127.0.0.1:{self.udp}',
            'peers': [], 'workspaces': [{'id': 'lab', 'path': str(self.workspace), 'profile_id': 'fixture', 'model': 'fixture-model', 'thinking': 'off'}],
        }}
        self.process = None
        (root / 'config.json').write_text(json.dumps(self.config))
        # Station itself owns iroh. No supervisor or transport helper is
        # involved in identity initialization, network startup or shutdown.
        with (root / 'bootstrap.log').open('wb') as log:
            bootstrap = subprocess.Popen([str(TARGET / 'zork-station'), '--data', str(root)],
                                         stdout=log, stderr=log)
            try:
                def identity():
                    assert bootstrap.poll() is None, f'Station exited during bootstrap; see {root / "bootstrap.log"}'
                    return self.get('/v1/mesh').get('origin')
                self.origin = wait(identity, 'Station-owned iroh initialization')
                children = subprocess.run(['pgrep', '-P', str(bootstrap.pid)], capture_output=True, text=True)
                assert children.returncode == 1 and not children.stdout.strip(), 'Station spawned a transport helper'
                assert not (root / 'zork.pid').exists(), 'Station required a supervisor'
                assert not (root / 'mesh/iroh/control.sock').exists(), 'Station created a Mesh control socket'
                assert not (root / 'mesh/iroh/control.token').exists(), 'Station created a Mesh control token'
            finally:
                bootstrap.terminate()
                assert bootstrap.wait(timeout=30) == 0, 'Station did not close its Mesh tasks cleanly'

    def pair(self, other):
        self.config['mesh']['peers'] = [{'origin': other.origin, 'name': other.root.name,
                                         'addr': f'127.0.0.1:{other.udp}', 'execute': ['lab']}]
        (self.root / 'config.json').write_text(json.dumps(self.config))

    def start(self):
        self.log = (self.root / 'supervisor.log').open('ab')
        self.process = subprocess.Popen([str(TARGET / 'zork'), 'start', '--data', str(self.root), '--fake-agent'],
                                        stdout=self.log, stderr=self.log, start_new_session=True)

    def stop(self):
        if self.process:
            self.process.terminate()
            try:
                self.process.wait(timeout=12)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait()
            self.process = None
            self.log.close()

    def request(self, method, path, body=None):
        data = None if body is None else json.dumps(body).encode()
        request = Request(self.url + path, data=data, method=method, headers={'Content-Type': 'application/json'})
        try:
            response = urlopen(request, timeout=8)
        except HTTPError as error:
            response = error
        with response:
            raw = response.read()
            return response.status, json.loads(raw) if raw else None

    def get(self, path):
        status, body = self.request('GET', path)
        assert status == 200, (status, body)
        return body

    def task(self, task_id):
        return self.get(f'/v1/tasks/{task_id}')['task']

    def new_task(self):
        status, session = self.request('POST', '/v1/im/sessions', {'profile_id': 'fixture', 'model': 'fixture-model', 'thinking': 'off', 'workspace': str(self.workspace)})
        assert status == 201, (status, session)
        return next(task for task in self.get('/v1/tasks')['items'] if task['session_id'] == session['session_id'])

    def restart_station(self):
        pid_file = self.root / 'run/zork-station.pid'
        old = int(pid_file.read_text())
        os.kill(old, signal.SIGKILL)
        wait(lambda: int(pid_file.read_text()) != old and self.request('GET', '/readyz')[0] == 200,
             'supervisor restores Station')


def main():
    parser = argparse.ArgumentParser()
    args = parser.parse_args()
    root = Path(tempfile.mkdtemp(prefix='zmp-', dir='/tmp'))
    print(f'isolated product mesh: {root}', flush=True)
    a, b = Node(root / 'a'), Node(root / 'b')
    a.pair(b); b.pair(a)
    try:
        a.start(); b.start()
        for node in (a, b):
            wait(lambda: node.request('GET', '/readyz')[0] == 200, node.root.name + ' ready')
            wait(lambda: urlopen(node.agent_url+'/readyz',timeout=2).status==200,node.root.name+' Agent ready')
            assert node.get('/v1/mesh')['origin'] == node.origin
        task = a.new_task()
        # This is a real shell invocation through the fake model, with an
        # observable side effect that would expose duplicate execution.
        (b.workspace / 'report.md').write_text('# Remote report\n\nVerified delivery.\n')
        script = "printf 'started\\n' >> executions.txt\nsleep 5"
        goal = json.dumps({'fake_tools': [
            {'name': 'shell.run', 'input': {'command': script}},
            {'name': 'chat.post_file', 'input': {'attachments': [{'file_path': str(b.workspace / 'report.md')}], 'text': 'remote artifact'}},
            {'name': 'chat.post_message', 'input': {'text': 'Remote result ready for review.'}},
        ]})
        command = {'command_id': 'mesh-fixture-1', 'expected_revision': task['revision'], 'executor_origin': b.origin, 'workspace_id': 'lab', 'goal': goal}
        route = f"/v1/tasks/{task['task_id']}/delegate"
        assert a.request('POST', route, command)[0] == 202
        assert a.request('POST', route, command)[0] == 202
        changed = dict(command, goal='different command')
        assert a.request('POST', route, changed)[0] == 409
        wait(lambda: (b.workspace / 'executions.txt').exists(), 'remote runtime actually starts')
        # Fault injection: B committed intake, but A did not persist its ACK.
        with sqlite3.connect(a.root/'state/station.sqlite') as db:
            db.execute("UPDATE mesh_links SET state='queued' WHERE assignment_id='mesh-fixture-1'")
        b.restart_station()
        wait(lambda: a.task(task['task_id'])['state'] == 'review' and a.task(task['task_id'])['last_run_status'] == 'finished', 'remote result and run completion', 90)
        owner = a.task(task['task_id'])
        assert owner['run_count'] == 1, owner
        assert (b.workspace / 'executions.txt').read_text().splitlines() == ['started']
        assert owner['result_text'] == 'Remote result ready for review.'
        assert any(item['task_id'] == task['task_id'] for item in a.get('/v1/inbox')['items'])
        detail = a.get(f"/v1/tasks/{task['task_id']}")
        assert len(detail['artifacts']) == 1, detail
        artifact = detail['artifacts'][0]
        assert artifact['workspace'].startswith(b.origin + ':'), artifact
        with urlopen(a.url + f"/v1/artifacts/{artifact['artifact_id']}/content") as response:
            assert response.read() == b'# Remote report\n\nVerified delivery.\n'
        (b.workspace / 'report.md').unlink()
        remote_task = next(t for t in b.get('/v1/tasks')['items'] if t['mesh'])
        assert b.request('POST', f"/v1/tasks/{remote_task['task_id']}/transitions", {'expected_revision': remote_task['revision'], 'action': 'accept'})[0] == 409
        assert a.request('POST', f"/v1/im/sessions/{task['session_id']}/messages", {'content': 'must not run locally'})[0] == 409
        print('PASS: remote Agent + registered tool result/file delivery; duplicate command and Station restart do not duplicate the run', flush=True)

        a.restart_station()
        restored = a.task(task['task_id'])
        assert restored['result_message_id'] == owner['result_message_id'] and restored['run_count'] == 1
        assert a.request('POST', f"/v1/tasks/{task['task_id']}/transitions", {'expected_revision': task['revision'], 'action': 'accept'})[0] == 409
        status, accepted = a.request('POST', f"/v1/tasks/{task['task_id']}/transitions", {'expected_revision': restored['revision'], 'action': 'accept'})
        assert status == 200 and accepted['state'] == 'completed', (status, accepted)
        wait(lambda: b.task(remote_task['task_id'])['state'] == 'completed', 'owner decision replicated')
        b.stop()
        with urlopen(a.url + f"/v1/artifacts/{artifact['artifact_id']}/content") as response:
            assert response.read() == b'# Remote report\n\nVerified delivery.\n'
        assert not any(item['task_id'] == task['task_id'] for item in a.get('/v1/inbox')['items'])
        print('PASS: owner-only CAS acceptance converges; copied file survives original deletion and executor shutdown', flush=True)

        queued=a.new_task()
        long_goal=json.dumps({'fake_tool':{'name':'shell.run','input':{'command':"printf 'started\\n' >> cancelled-executions.txt\nsleep 30\nprintf 'unexpected\\n' > should-not-exist.txt"}}})
        command2={'command_id':'mesh-fixture-cancel','expected_revision':queued['revision'],'executor_origin':b.origin,'workspace_id':'lab','goal':long_goal}
        assert a.request('POST',f"/v1/tasks/{queued['task_id']}/delegate",command2)[0]==202
        a.restart_station()
        assert a.task(queued['task_id'])['mesh']['state']=='queued'
        b.start()
        wait(lambda:b.request('GET','/readyz')[0]==200,'executor restarted')
        wait(lambda:(b.workspace/'cancelled-executions.txt').exists(),'offline queued task resumes',90)
        assert a.request('POST',f"/v1/im/sessions/{queued['session_id']}/cancel")[0]==202
        wait(lambda:a.task(queued['task_id'])['last_run_status']=='cancelled','remote cancellation acknowledged',90)
        current=a.task(queued['task_id'])
        assert a.request('POST',f"/v1/tasks/{queued['task_id']}/transitions",{'expected_revision':current['revision'],'action':'cancel'})[0]==200
        remote_cancel=next(t for t in b.get('/v1/tasks')['items'] if t.get('mesh',{}).get('assignment_id')=='mesh-fixture-cancel')
        wait(lambda:b.task(remote_cancel['task_id'])['state']=='cancelled','cancel decision replicated')
        assert (b.workspace/'cancelled-executions.txt').read_text().splitlines()==['started']
        assert not (b.workspace/'should-not-exist.txt').exists()
        print('PASS: offline outbox survives owner restart and resumes once; remote stop is acknowledged before product cancellation',flush=True)
        (root / 'result.json').write_text(json.dumps({'checks': ['remote_run', 'command_dedup', 'lost_ack', 'restart_during_run', 'result_inbox', 'artifact_drive', 'owner_only_accept', 'revision_conflict', 'decision_replication', 'offline_artifact','offline_queue_restart','remote_cancel'], 'task_id': task['task_id']}, indent=2))
        print('all product mesh checks passed', flush=True)
    finally:
        a.stop(); b.stop()


if __name__ == '__main__':
    main()
