#!/usr/bin/env python3
"""Exercise the real native download script on fresh and stopped installations."""
import http.server
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import shutil
import tarfile
import tempfile
import threading
from urllib.request import urlopen

spec = importlib.util.spec_from_file_location('enrollment', Path(__file__).with_name('test-mesh-enrollment.py'))
e = importlib.util.module_from_spec(spec)
spec.loader.exec_module(e)
f = e.f
f.TARGET = f.ROOT / 'target/release'
release_spec = importlib.util.spec_from_file_location('release', f.ROOT / 'scripts/build/native-release.py')
release = importlib.util.module_from_spec(release_spec)
release_spec.loader.exec_module(release)


def main():
    root = Path(tempfile.mkdtemp(prefix='zinstaller-', dir='/tmp'))
    registry = str(root / 'registry')
    os.environ['ZORK_REGISTRY_DIR'] = registry
    version = release.version()
    key = release.host_platform()
    assets = f.ROOT / 'artifacts/native-release'
    package = assets / f'zork-{version}-{key}.tar.gz'
    assert package.is_file(), 'Run release:pack first'
    mirror = root / 'releases/download' / f'v{version}'
    mirror.mkdir(parents=True)
    shutil.copyfile(package, mirror / package.name)
    shutil.copyfile(assets / f'{key}.tsv', mirror / 'manifest.tsv')
    packaged = root / 'packaged'
    packaged.mkdir()
    release.verify_archive(package, version)
    with tarfile.open(package) as tar:
        tar.extractall(packaged)
    env = dict(os.environ, ZORK_REGISTRY_DIR=registry, ZORK_CLIENT_DATA=str(root / 'client'), ZORK_DESKTOP_FAKE_AGENT='1')
    source = f.Node(root / 'source')
    source.config['admin'] = {'token': 'enrollment-fixture'}
    source.config['mesh']['name'] = 'source'
    (source.root / 'config.json').write_text(json.dumps(source.config))
    fresh = root / 'fresh install with spaces'
    standalone = root / 'standalone'
    command = ['/bin/sh', str(f.ROOT / 'scripts/install.sh'), '--base-url', (root / 'releases').as_uri(), '--version', version, '--']
    def run(args, success=True):
        result = subprocess.run(command + args, cwd=root, env=env, capture_output=True, text=True, timeout=90)
        assert (result.returncode == 0) == success, result.stderr
        return result
    def current_pids():
        return [(fresh / p).read_text() for p in ['zork.pid', 'run/zork-station.pid']]
    server = None
    print('isolated native installer:', root, flush=True)
    try:
        # Existing operators may have started Station from a relative --data path.
        source.log=(source.root/'supervisor.log').open('ab')
        source.process=subprocess.Popen([str(f.TARGET/'zork'),'start','--data','source','--fake-agent'],
            cwd=root,env=env,stdout=source.log,stderr=source.log,start_new_session=True)
        f.wait(lambda: source.request('GET', '/readyz')[0] == 200, 'source ready')
        assert json.loads(run(['mesh','status','--data',str(source.root),'--json']).stdout)['origin']==source.origin
        assert json.loads(run(['install', '--data', str(standalone), '--name', 'standalone', '--json']).stdout)['installed']
        standalone_config = json.loads((standalone / 'config.json').read_text())
        assert standalone_config['mesh']['name'] == 'standalone'
        assert not standalone_config['mesh'].get('group')
        assert (standalone / 'run/zork-station.pid').is_file()
        run(['stop', '--data', str(standalone)])
        print('PASS: standalone installation starts a node without creating an invitation', flush=True)
        invitation = e.admin(source, 'POST', '/v1/node/mesh/invites')['invitation']
        args = ['mesh', 'join', invitation, '--data', str(fresh), '--name', 'fresh node', '--json']
        assert json.loads(run(args).stdout)['joined']
        identity = json.loads(run(['mesh', 'status', '--data', str(fresh), '--json']).stdout)['origin']
        pids = current_pids()
        assert json.loads(run(args).stdout)['already_joined']
        assert current_pids() == pids
        assert json.loads(run(['install', '--data', str(fresh), '--json']).stdout)['installed']
        assert current_pids() == pids
        assert all((fresh / 'bin' / name).is_file() for name in ['zork', 'zork-station', 'zork-agent', 'zork-gh'])
        settings = json.loads((fresh / 'service.json').read_text())
        assert settings == {'enabled': True, 'start_at_login': True}
        config = json.loads((fresh / 'config.json').read_text())
        assert all(value.startswith('127.0.0.1:') for value in config['bind'].values())
        assert len(set(config['bind'].values())) == 4
        print('PASS: fresh native installation from verified download, paths with spaces, background service, idempotent reuse', flush=True)

        # Root selection is deliberately explicit when several Stations are live.
        ambiguous = run(['mesh', 'join', invitation], success=False)
        assert 'Multiple Station installations' in ambiguous.stderr, ambiguous.stderr
        assert current_pids() == pids
        run(['stop', '--data', str(fresh)])
        config = json.loads((fresh / 'config.json').read_text())
        config['context']['keep_recent_tokens'] = 30000
        (fresh / 'config.json').write_text(json.dumps(config))
        assert json.loads(run(args).stdout)['already_joined']
        assert json.loads(run(['mesh', 'status', '--data', str(fresh), '--json']).stdout)['origin'] == identity
        assert json.loads((fresh / 'config.json').read_text())['context']['keep_recent_tokens'] == 30000
        print('PASS: multiple-instance detection and stopped installation reuse preserve identity and existing settings', flush=True)

        class OlderStation(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(200 if self.path == '/readyz' else 404)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(b'{}')
            def log_message(self, *_):
                pass
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), OlderStation)
        threading.Thread(target=server.serve_forever, daemon=True).start()
        older = root / 'older'
        older.mkdir()
        older_config = {'bind': {'runtime': f'127.0.0.1:{server.server_port}'}}
        (older / 'config.json').write_text(json.dumps(older_config))
        before = (older / 'config.json').read_bytes()
        incompatible = run(['mesh', 'join', invitation, '--data', str(older)], success=False)
        assert 'does not support mesh enrollment' in incompatible.stderr
        assert (older / 'config.json').read_bytes() == before and not (older / 'zork.pid').exists()
        print('PASS: incompatible running Station gets an actionable error without replacement or configuration reset', flush=True)
        (assets / 'installer-result.json').write_text(json.dumps({'root': str(root), 'platform': os.uname().sysname, 'archive_sha256': release.sha256(mirror / package.name), 'checks': ['fresh_native_download', 'standalone_install', 'install_command', 'all_components', 'background', 'repeat', 'ambiguous_instances', 'stopped_install', 'identity', 'preserve_config', 'old_station']}, indent=2))
    finally:
        if (fresh / 'bin/zork').exists():
            subprocess.run([str(fresh / 'bin/zork'), 'stop', '--data', str(fresh)], env=env, capture_output=True, timeout=30)
        if (standalone / 'bin/zork').exists():
            subprocess.run([str(standalone / 'bin/zork'), 'stop', '--data', str(standalone)], env=env, capture_output=True, timeout=30)
        source.stop()
        if server:
            server.shutdown()


if __name__ == '__main__':
    main()
