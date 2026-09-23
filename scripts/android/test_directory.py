#!/usr/bin/env python3
"""Android JNI and ViewModel observe an entire real Mesh and recover after restart."""
from test_apks import require_test_apks
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('directory_fixture', ROOT / 'scripts/test-mobile-mesh-directory.py')
d = importlib.util.module_from_spec(spec)
spec.loader.exec_module(d)
PACKAGE = 'ing.zork.android.test'
CLASS = 'ing.zork.android.MeshDirectoryTest'
RUNNER = PACKAGE + '.test/androidx.test.runner.AndroidJUnitRunner'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--serial', required=True)
    parser.add_argument('--apk', type=Path, required=True)
    parser.add_argument('--test-apk', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    require_test_apks(args.apk, args.test_apk)
    assert args.serial.startswith('emulator-'), 'This fixture is only for an isolated emulator'
    adb = [str(Path(os.environ.get('ANDROID_HOME', Path.home() / 'Library/Android/sdk')) / 'platform-tools/adb'), '-s', args.serial]
    args.output.mkdir(parents=True, exist_ok=True)
    for apk in (args.apk, args.test_apk):
        subprocess.run([*adb, 'install', '-r', str(apk.resolve())], check=True)
    root = Path(tempfile.mkdtemp(prefix='zad-', dir='/tmp'))
    os.environ['ZORK_REGISTRY_DIR'] = str(root / 'registry')
    os.environ['ZORK_CHANNEL'] = 'test'
    nodes, running, relay, relay_port = [], None, None, None
    def report():
        result = subprocess.run([*adb, 'exec-out', 'run-as', PACKAGE, 'cat', 'files/mesh-directory-report.json'], capture_output=True)
        try: return json.loads(result.stdout) if result.returncode == 0 else {}
        except json.JSONDecodeError: return {}
    def stage(name):
        def reached():
            value = report()
            if value.get('stage') == name: return value
            if running and running[0].poll() is not None:
                raise AssertionError(Path(running[1].name).read_text())
            return None
        return d.f.wait(reached, name, 90)
    def instrument(method, **values):
        subprocess.run([*adb, 'shell', 'am', 'force-stop', PACKAGE], check=True)
        command = [*adb, 'shell', 'am', 'instrument', '-w', '-r', '-e', 'class', CLASS + '#' + method, '-e', 'isolated', 'true', '-e', 'fixture', root.name]
        for key, value in values.items(): command += ['-e', key, value]
        log = (args.output / (method + '.log')).open('w')
        process = subprocess.Popen([*command, RUNNER], stdout=log, stderr=subprocess.STDOUT, text=True)
        return process, log
    def finished(job):
        process, log = job
        assert process.wait(timeout=180) == 0
        log.close()
        value = Path(log.name).read_text()
        assert 'OK (1 test)' in value, value
        print(value, flush=True)
    try:
        relay_log = (args.output / 'relay.log').open('w')
        relay = subprocess.Popen([str(d.f.TARGET / 'examples/relay-fixture')], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=relay_log, text=True)
        relay_port = json.loads(relay.stdout.readline())['port']
        # The emulator's NAT does not forward LAN multicast. Supply the same
        # real anonymous fixture relay on its loopback, alongside the logged-out
        # public relay. No peer address or user network is changed.
        subprocess.run([*adb, 'reverse', f'tcp:{relay_port}', f'tcp:{relay_port}'], check=True)
        for name in ('android-a', 'android-b', 'android-c'):
            node = d.f.Node(root / name); nodes.append(node)
            node.config['admin'] = {'token': d.TOKEN}
            node.config['mesh'].update(name=name, offline=False, bind=None,
                relay_urls=['https://relay.zork.ing', f'http://127.0.0.1:{relay_port}'], discovery_url='http://127.0.0.1:9/pkarr', quic_discovery_urls=[])
            (node.root / 'config.json').write_text(json.dumps(node.config))
            node.start(); d.ready(node)
            d.f.wait(lambda: d.admin(node, 'GET', '/v1/node/mesh')['address']['relays'], 'relay address advertised')
        a, b, c = nodes
        d.join(b, a)
        invite = d.admin(a, 'POST', '/v1/node/mesh/client-invites')
        running = instrument('approvalDynamicMembersAndRecovery', a=a.origin, b=b.origin, c=c.origin, ticket=invite['invitation'])
        identity = stage('awaiting_approval')['identity']
        claim = next(i for i in d.admin(a, 'GET', '/v1/node/mesh/invites')['items'] if i['id'] == invite['id'])
        assert claim['device']['origin'] == identity
        d.admin(a, 'POST', '/v1/node/mesh/invites/' + invite['id'] + '/approve', {'origin': identity, 'claim_id': claim['claim_id']})
        stage('initial_members')
        d.join(c, a); stage('member_added')
        d.admin(a, 'POST', '/v1/node/mesh/members/remove', {'origin': c.origin}); stage('member_removed')
        a.stop(); b.restart_station(); d.ready(b)
        subprocess.run([*adb, 'shell', 'run-as', PACKAGE, 'tee', 'files/mesh-directory-step'], input=b'restart', stdout=subprocess.DEVNULL, check=True)
        stage('recovered'); finished(running); running = None
        running = instrument('reopenAfterProcessRestart', a=a.origin, b=b.origin)
        finished(running); running = None
        assert report()['stage'] == 'reopened'
        (args.output / 'result.json').write_text(json.dumps({'fixture': str(root), 'serial': args.serial, 'channel': 'test',
            'transport_fixture': 'real loopback relay exposed to emulator NAT through a task-owned ADB TCP reverse',
            'checks': ['approved whole Mesh', 'JNI directory subscription', 'ViewModel member add/remove', 'inviter offline',
                'changed endpoint port', 'foreground and process recovery', 'channel rejection before IO']}, indent=2))
        print('PASS: Android whole Mesh directory, platform projection and recovery', flush=True)
    finally:
        if running:
            running[0].terminate(); running[0].wait(timeout=15); running[1].close()
            subprocess.run([*adb, 'shell', 'am', 'force-stop', PACKAGE], check=False)
        for node in nodes: node.stop()
        if relay:
            relay.stdin.close(); relay.wait(timeout=15); relay_log.close()
        if relay_port:
            subprocess.run([*adb, 'reverse', '--remove', f'tcp:{relay_port}'], check=False)
        print('Isolated fixture:', root, flush=True)


if __name__ == '__main__':
    main()
