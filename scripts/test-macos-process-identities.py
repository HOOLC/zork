#!/usr/bin/env python3
"""Real bundle identities, CEF roles and service-watch lifetime; rebuild affected binaries first."""
from pathlib import Path
import importlib.util, json, os, signal, socket, subprocess, sys, tempfile, time, threading
from urllib.request import build_opener, ProxyHandler
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import argparse
parser = argparse.ArgumentParser(description='Verify packaged macOS process names, icons and guard lifetime with isolated fixtures.')
parser.add_argument('--app', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--node-only', action='store_true', help='Verify supervisor, Station and service-watch without the browser fixture')
args = parser.parse_args()
ROOT = Path(__file__).resolve().parents[1]
OUT = args.output.resolve()
APP = args.app.resolve()
OUT.mkdir(parents=True, exist_ok=True)
if sys.platform != 'darwin':
    raise SystemExit('Requires macOS')
spec = importlib.util.spec_from_file_location('browser_fixture', ROOT / 'scripts/test-browser-runtime.py')
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
INSPECT = None
HTTP = build_opener(ProxyHandler({}))

def identities(pids):
    return [json.loads(s) for s in subprocess.check_output([str(INSPECT), *map(str, pids)], text=True).splitlines()]

class Page(BaseHTTPRequestHandler):

    def log_message(self, *_):
        pass

    def do_GET(self):
        body = b'<!doctype html><title>Process icons</title><p>Browser process identities</p><script>localStorage.setItem("fixture","ok")</script>'
        self.send_response(200)
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)
with tempfile.TemporaryDirectory(prefix='zork-process-identity-') as directory:
    root = Path(directory)
    profile = root / 'browser'
    profile.mkdir()
    inspector = root / 'inspect.swift'
    INSPECT = root / 'inspect-process'
    inspector.write_text("""\
import AppKit
import Foundation
for value in CommandLine.arguments.dropFirst() {
    guard let pid = Int32(value) else { continue }
    let app = NSRunningApplication(processIdentifier: pid)
    let record: [String: Any] = ["pid": pid, "name": app?.localizedName ?? "", "bundle": app?.bundleIdentifier ?? "", "bundle_url": app?.bundleURL?.path ?? "", "icon": app?.icon != nil, "activation_policy": app?.activationPolicy.rawValue ?? -1]
    let bytes = try JSONSerialization.data(withJSONObject: record, options: [.sortedKeys])
    print(String(data: bytes, encoding: .utf8)!)
}
""")
    subprocess.run(['swiftc', str(inspector), '-o', str(INSPECT)], check=True)
    server = ThreadingHTTPServer(('127.0.0.1', 0), Page)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    runtime = None
    guard = None
    supervisor = None
    node_group = None
    report = {}
    try:
        node = root / 'node'
        node.mkdir()
        bindings = {}
        reservations = []
        try:
            for name in ('gateway', 'runtime', 'control', 'agent'):
                reservation = socket.socket()
                reservation.bind(('127.0.0.1', 0))
                reservations.append(reservation)
                bindings[name] = f'127.0.0.1:{reservation.getsockname()[1]}'
            (node / 'config.json').write_text(json.dumps({'bind': bindings}))
        finally:
            for reservation in reservations:
                reservation.close()
        with (OUT / 'node.log').open('wb') as log:
            supervisor = subprocess.Popen([str(APP / 'Contents/MacOS/zork'),
                'start', '--data', str(node), '--fake-agent'], stdin=subprocess.PIPE,
                stdout=log, stderr=log, env=dict(os.environ, ZORK_PARENT_PIPE='1',
                    ZORK_REGISTRY_DIR=str(root / 'registry')), start_new_session=True)
            node_group = supervisor.pid
        end = time.monotonic() + 15
        while time.monotonic() < end:
            assert supervisor.poll() is None, 'supervisor exited before readiness'
            try:
                with HTTP.open('http://' + bindings['runtime'] + '/readyz', timeout=.2) as response:
                    ready = json.load(response)
                    if ready.get('ok'):
                        break
            except OSError:
                pass
            time.sleep(.01)
        else:
            raise AssertionError('Station did not become ready')
        report['node_processes'] = identities([supervisor.pid, ready['pid']])
        assert {item['name'] for item in report['node_processes']} == {'Zork-Supervisor', 'Zork-Station'}, report
        assert {item['bundle'] for item in report['node_processes']} == {'ing.zork.desktop.supervisor', 'ing.zork.desktop.station'}, report
        assert all(item['icon'] and item['activation_policy'] == 2 for item in report['node_processes']), report
        supervisor.stdin.close()
        assert supervisor.wait(timeout=10) == 0
        supervisor = None
        try:
            os.kill(ready['pid'], 0)
        except ProcessLookupError:
            pass
        else:
            raise AssertionError('Station survived supervisor owner-pipe closure')
        report['node_pipe_cleanup'] = True
        for reason in ('owner-pipe', 'SIGTERM'):
            with (OUT / 'node.log').open('ab') as log:
                supervisor = subprocess.Popen([str(APP / 'Contents/MacOS/zork'),
                    'start', '--data', str(node), '--fake-agent'], stdin=subprocess.PIPE,
                    stdout=log, stderr=log, env=dict(os.environ, ZORK_PARENT_PIPE='1',
                        ZORK_REGISTRY_DIR=str(root / 'registry')), start_new_session=True)
                node_group = supervisor.pid
            if reason == 'owner-pipe':
                supervisor.stdin.close()
            else:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    assert supervisor.poll() is None, 'supervisor exited during startup'
                    processes = subprocess.check_output(['ps', '-axo', 'pid=,ppid='], text=True)
                    if any(int(line.split()[1]) == supervisor.pid for line in processes.splitlines()):
                        break
                    time.sleep(.001)
                else:
                    raise AssertionError('Station was not spawned')
                supervisor.terminate()
            assert supervisor.wait(timeout=10) == 0, reason
            supervisor = None
            try:
                os.killpg(node_group, 0)
            except ProcessLookupError:
                pass
            else:
                raise AssertionError(f'Child survived early {reason}')
            report[f'early_{reason}_cleanup'] = True
        (OUT / 'node-processes.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
        print('PASS node identities, normal exit and shutdown during startup', flush=True)
        status = root / 'guard-status.json'
        payload = {'command': ['/bin/sleep', '60'], 'cwd': str(root), 'status': str(status), 'run_id': 'identity-fixture'}
        entry = APP / 'Contents/Helpers/ZorkStation.app/Contents/MacOS/zork-service-watch'
        guard = subprocess.Popen([str(entry), '--service-process', json.dumps(payload)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        end = time.monotonic() + 15
        while not status.exists() and time.monotonic() < end:
            if guard.poll() is not None:
                raise RuntimeError(guard.stderr.read().decode())
            time.sleep(0.05)
        child_pid = json.loads(status.read_text())['pid']
        report['watch'] = identities([guard.pid])[0]
        assert report['watch']['name'] == 'Zork-Service-Watch', report
        assert report['watch']['icon'], report
        guard.stdin.close()
        assert guard.wait(timeout=10) == 128
        try:
            os.kill(child_pid, 0)
        except ProcessLookupError:
            pass
        else:
            raise AssertionError('service child survived owner pipe closure')
        report['watch_pipe_cleanup'] = True
        guard = None
        (OUT / 'node-processes.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
        print('PASS service-watch identity and owner-pipe cleanup', flush=True)
        if not args.node_only:
            runtime = m.Runtime(APP / 'Contents/Helpers/ZorkBrowser.app/Contents/MacOS/ZorkBrowser', profile)
            tab = runtime.open(f'http://127.0.0.1:{server.server_port}/')
            runtime.call('Zork.viewport', {'width': 600, 'height': 500, 'visible': True, 'scale': 1}, tab)
            assert runtime.evaluate(tab, 'localStorage.getItem("fixture")') == 'ok'
            runtime.evaluate(tab, 'new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(()=>r(true))))')
            child = []
            for line in subprocess.check_output(['ps', '-axo', 'pid=,ppid=,args='], text=True).splitlines():
                fields = line.strip().split(None, 2)
                if len(fields) == 3 and int(fields[1]) == runtime.p.pid:
                    child.append((int(fields[0]), fields[2]))
            report['browser_processes'] = identities([runtime.p.pid, *[pid for pid, _ in child]])
            report['child_commands'] = child
            print(json.dumps(report['browser_processes'], ensure_ascii=False, indent=2), flush=True)
            (OUT / 'runtime-processes.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
            expected = {'Zork-Browser', 'Zork-Browser-GPU', 'Zork-Browser-Network', 'Zork-Browser-Storage', 'Zork-Browser-Renderer'}
            assert expected.issubset({p['name'] for p in report['browser_processes']}), report
            assert all((p['icon'] for p in report['browser_processes'])), report
            (OUT / 'runtime.log').write_bytes((profile / 'runtime.log').read_bytes() if (profile / 'runtime.log').exists() else b'')
            runtime.close()
            runtime = None
        (OUT / 'validation.json').write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
        print('PASS selected process identity and lifecycle checks', flush=True)
    finally:
        if runtime:
            for name in ['runtime.log', 'probe-process.log', 'network.json']:
                source = profile / name
                if source.exists():
                    (OUT / name).write_bytes(source.read_bytes())
            try:
                runtime.close()
            except Exception:
                runtime.dispose()
        if guard and guard.poll() is None:
            guard.stdin.close()
            try:
                guard.wait(timeout=10)
            except subprocess.TimeoutExpired:
                guard.kill()
                guard.wait()
        if supervisor and supervisor.poll() is None:
            if supervisor.stdin and not supervisor.stdin.closed:
                supervisor.stdin.close()
            try:
                supervisor.wait(timeout=10)
            except subprocess.TimeoutExpired:
                supervisor.kill()
                supervisor.wait()
        if node_group is not None:
            try:
                os.killpg(node_group, signal.SIGKILL)
            except ProcessLookupError:
                pass
        server.shutdown()
        server.server_close()
