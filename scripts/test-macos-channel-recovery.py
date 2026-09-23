#!/usr/bin/env python3
"""Exercise three signed clients and the real installer using only isolated data/apps."""
import argparse
from contextlib import ExitStack, closing
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import shutil
import sqlite3
import subprocess
import sys
import tarfile
import tempfile
from unittest.mock import patch
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/lib'))
from channels import CHANNELS, app_name
from deployment import atomic_json, copy_tree, digest, manifest
from deployment_macos import AppRuntime, LSREGISTER, validate_app
from deployment_build import load_packager, source_stamp


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


drill = load('node_drill', ROOT / 'scripts/test-release-dev-recovery.py')
recovery = drill.recovery


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--candidate', type=Path, help='Captured app candidate; package three isolated identities from its verified Cargo inputs')
    parser.add_argument('--dev-app', type=Path)
    parser.add_argument('--release-app', type=Path)
    parser.add_argument('--test-app', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if bool(args.candidate) == bool(args.dev_app or args.release_app or args.test_app) or (not args.candidate and not (args.dev_app and args.release_app and args.test_app)):
        parser.error('provide --candidate or --dev-app, --release-app and --test-app')
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix='zarc-', dir='/tmp')).resolve()
    runtimes = {}
    report = {'passed': False, 'host': subprocess.check_output(['hostname'], text=True).strip(), 'checks': [],
              'scripts_source': source_stamp(ROOT),
              'scope': 'three signed fixture app identities, isolated local data and Mesh; real installer, Agent and chat tools; deterministic external model'}
    def passed(name, evidence):
        report['checks'].append({'name': name, 'evidence': evidence})
        atomic_json(output / 'result.json', report)
        print('PASS ' + name, flush=True)
    try:
        if args.candidate:
            record, _ = recovery.load_candidate(args.candidate)
            if record['kind'] != 'app':
                raise RuntimeError('A node candidate does not contain captured GUI/browser inputs')
            raw = args.candidate.resolve() / 'bin'
            build_record = args.candidate.resolve() / 'build.json'
            if json.loads(build_record.read_text()) != record['source']:
                raise RuntimeError('Candidate build record differs from its manifest')
            for name, expected in record['source']['binaries'].items():
                if digest(raw / name) != expected:
                    raise RuntimeError('Captured fixture input differs: ' + name)
            packager = load_packager(ROOT)
            for channel in CHANNELS:
                app = root / 'prepared' / (channel + '.app')
                options = SimpleNamespace(bin_dir=raw, browser_bin_dir=raw,
                    id_prefix='ing.zork.recovery-fixture.' + channel, channel=channel,
                    services_config=None, build_record=build_record,
                    cua_runtime=args.candidate.resolve() / 'cua-runtime')
                with patch.dict(os.environ, {'ZORK_CODESIGN_IDENTITY': '-'}):
                    packager.build_app(options, ROOT, app)
                if json.loads((app / 'Contents/Resources/build.json').read_text()) != record['source']:
                    raise RuntimeError('Packager lost captured build_record provenance')
                setattr(args, channel + '_app', app)
            report['input_candidate'] = record['id']
        build_records = [json.loads((app.resolve() / 'Contents/Resources/build.json').read_text())
                         for app in (args.dev_app, args.release_app, args.test_app)]
        if any(record != build_records[0] for record in build_records):
            raise RuntimeError('All fixture channels must use the same captured build')
        report['build_record'] = build_records[0]
        report['fixture_manifests'] = {channel: manifest(app.resolve(), build_records[0], channel, 'app')
            for channel, app in ((channel, getattr(args, channel + '_app')) for channel in CHANNELS)}
        with ExitStack() as stack:
            config = {'repo': str(ROOT), 'channels': {}}
            for channel in CHANNELS:
                source = getattr(args, channel + '_app').resolve()
                app = root / (channel + '.app')
                copy_tree(source, app)
                data = root / channel / 'client'
                data.mkdir(parents=True)
                with closing(sqlite3.connect(data / 'client.db')) as db:
                    db.execute('CREATE TABLE cache(node TEXT,key TEXT,value TEXT NOT NULL,PRIMARY KEY(node,key))')
                    db.execute("INSERT INTO cache VALUES ('device','local-node-enabled','true')")
                    db.commit()
                provider = stack.enter_context(drill.Provider())
                provider.configure(data / 'node')
                atomic_json(data / 'node/config.json', {
                    'bind': {name: f'127.0.0.1:{drill.port()}' for name in ('station', 'runtime', 'control', 'agent')},
                    'admin': {'token': os.urandom(24).hex()},
                    'mesh': {'enabled': True, 'offline': True, 'bind': f'127.0.0.1:{drill.port(True)}', 'peers': [], 'workspaces': []}})
                prefix = plistlib.loads((app / 'Contents/Info.plist').read_bytes())['CFBundleIdentifier'].removesuffix('.desktop')
                if not prefix.startswith('ing.zork.recovery-fixture.'):
                    raise RuntimeError('Only dedicated fixture bundle IDs are permitted in this drill')
                settings = {'data': str(data), 'payload': str(app), 'channel': channel, 'id_prefix': prefix,
                            'health': {'profile': 'recovery-fixture', 'model': 'recovery-model', 'timeout': 30}}
                config['channels'][channel] = {'app': settings}
                runtime = AppRuntime(settings, output / (channel + '.log'))
                runtimes[channel] = runtime
                validate_app(app, channel, prefix)
                runtime.start(runtime.capture(), candidate=True)
            atomic_json(root / 'deployment-config.json', config)
            baseline = {channel: runtime.health() for channel, runtime in runtimes.items()}
            assert len({item['node']['origin'] for item in baseline.values()}) == 3
            groups = {}
            for channel, runtime in runtimes.items():
                runtime.node.request('/v1/node/mesh/invites', {})
                group = json.loads((runtime.data / 'node/config.json').read_text())['mesh']['group']
                assert group is not None
                groups[channel] = hashlib.sha256(json.dumps(group, sort_keys=True).encode()).hexdigest()
            assert len(set(groups.values())) == 3
            names = {}
            for channel in CHANNELS:
                pid = baseline[channel]['gui']['pid']
                script = ('ObjC.import("AppKit"); var a=$.NSRunningApplication.runningApplicationWithProcessIdentifier(' + str(pid) + '); '
                          'JSON.stringify({name:ObjC.unwrap(a.localizedName),bundle:ObjC.unwrap(a.bundleIdentifier)})')
                names[channel] = json.loads(subprocess.check_output(['osascript', '-l', 'JavaScript', '-e', script], text=True))
                assert names[channel]['name'] == app_name(channel), names
                assert names[channel]['bundle'] == config['channels'][channel]['app']['id_prefix'] + '.desktop'
            passed('signed release/dev/test GUIs coexist with distinct names, identities, data, Mesh and chat replies',
                   {'processes': baseline, 'names': names, 'mesh_group_digests': groups})

            test_candidate = root / 'test-incoming'
            test_candidate.mkdir()
            copy_tree(args.test_app.resolve(), test_candidate / 'Zork.app')
            atomic_json(test_candidate / 'deployment.json', manifest(
                test_candidate / 'Zork.app', build_records[0], 'test', 'app'))
            recovery.apply_candidate(root, 'test', test_candidate)
            runtimes['test'].stop()
            for channel in ('release', 'dev'):
                now = runtimes[channel].health()
                assert now['gui']['pid'] == baseline[channel]['gui']['pid']
                assert now['node']['station']['pid'] == baseline[channel]['node']['station']['pid']
                assert now['node']['origin'] == baseline[channel]['node']['origin']
            passed('test reinstall and shutdown preserve both daily clients and their chat replies', baseline)

            candidate = root / 'incoming'
            candidate.mkdir()
            copy_tree(args.dev_app.resolve(), candidate / 'Zork.app')
            (candidate / 'Zork.app/Contents/Resources/recovery-fixture.txt').write_text('installer candidate\n')
            subprocess.run(['codesign', '--force', '--sign', '-', str(candidate / 'Zork.app')], check=True, capture_output=True)
            source_record = json.loads((candidate / 'Zork.app/Contents/Resources/build.json').read_text())
            record = manifest(candidate / 'Zork.app', source_record, 'dev', 'app')
            atomic_json(candidate / 'deployment.json', record)
            stage = root / 'upload'
            stage.mkdir()
            archive = stage / 'app.tar.gz'
            with tarfile.open(archive, 'w:gz') as package:
                package.add(candidate, arcname='candidate')
            with (output / 'installer.log').open('w') as log:
                subprocess.run([sys.executable, str(ROOT / 'scripts/lib/install-macos-client.py'), str(stage), digest(archive),
                    '--root', str(root), '--channel', 'dev', '--profile', 'recovery-fixture', '--model', 'recovery-model'],
                    check=True, stdout=log, stderr=log)
            installed = json.loads((stage / 'result.json').read_text())
            assert installed['record'] == record
            release_now = runtimes['release'].health(chat=False)
            assert release_now['gui']['pid'] == baseline['release']['gui']['pid']
            assert release_now['node']['station']['pid'] == baseline['release']['node']['station']['pid']
            passed('real archive installer switches only dev and accepts the exact GUI and helper images',
                   {'candidate': record['id'], 'health': installed['health'], 'release_unchanged': release_now})

            dev = runtimes['dev']
            before = dev.capture()
            old_chat = baseline['dev']['node']['chat']
            old_page = dev.node.request('/v1/im/sessions/' + old_chat['chat_id'] + '/messages')
            old_group = json.loads((dev.data / 'node/config.json').read_text())['mesh']['group']
            def corrupt_migration(node):
                value = json.loads((node / 'config.json').read_text())
                value['bind']['runtime'] = 'injected-startup-failure'
                atomic_json(node / 'config.json', value)
                (node / 'state/station.sqlite').rename(node / 'state/migrated.sqlite')
                (dev.data / 'history-after-failure.txt').write_text('retain this failed-version history\n')
            config['channels']['dev']['app']['health']['timeout'] = 15
            atomic_json(root / 'deployment-config.json', config)
            with patch.object(recovery, 'migrate_config', side_effect=corrupt_migration):
                try:
                    recovery.apply_candidate(root, 'dev', candidate)
                    raise AssertionError('Node startup failure accepted by app installer')
                except RuntimeError as error:
                    assert 'acceptance' in str(error), str(error)
            recovered = dev.health()
            page = dev.node.request('/v1/im/sessions/' + old_chat['chat_id'] + '/messages')
            assert page['source_epoch'] != old_page['source_epoch']
            assert {old_chat['request_id'], old_chat['reply_id']}.issubset({m['id'] for m in page['items']})
            assert dev.capture()['gui_sha256'] == before['gui_sha256']
            assert recovered['node']['origin'] == baseline['dev']['node']['origin']
            assert json.loads((dev.data / 'node/config.json').read_text())['mesh']['group'] == old_group
            assert list((root / 'transactions').glob('*/failed-*/history-after-failure.txt'))
            assert runtimes['release'].gui()['pid'] == baseline['release']['gui']['pid']
            passed('failed app startup restores bundle, client/Station data and history, with new source epochs', recovered)

            original_pid = dev.gui()['pid']
            corrupt = root / 'bad-signature'
            copy_tree(candidate, corrupt)
            (corrupt / 'Zork.app/Contents/Resources/recovery-fixture.txt').write_text('unsigned modification\n')
            atomic_json(corrupt / 'deployment.json', manifest(corrupt / 'Zork.app', source_record, 'dev', 'app'))
            try:
                recovery.apply_candidate(root, 'dev', corrupt)
                raise AssertionError('Bad code signature accepted')
            except subprocess.CalledProcessError:
                pass
            assert dev.gui()['pid'] == original_pid
            assert runtimes['release'].gui()['pid'] == baseline['release']['gui']['pid']
            passed('signature failure is rejected before stopping either client', {'dev_pid': original_pid})
            report['passed'] = True
    except BaseException as error:
        report['error'] = str(error)
        raise
    finally:
        evidence = output / 'transactions'
        evidence.mkdir(exist_ok=True)
        for journal in (root / 'transactions').glob('*/journal.json'):
            shutil.copyfile(journal, evidence / (journal.parent.name + '.json'))
        errors = []
        for runtime in runtimes.values():
            try:
                runtime.stop()
            except Exception as error:
                errors.append(str(error))
        report['cleanup_errors'] = errors
        if errors:
            report.update(passed=False, retained_fixture=str(root))
        else:
            for app in root.rglob('*.app'):
                subprocess.run([LSREGISTER, '-u', str(app)], capture_output=True)
            shutil.rmtree(root)
            report['isolated_apps_and_data_removed'] = True
        atomic_json(output / 'result.json', report)
    print('PASS macOS channel recovery: ' + str(output / 'result.json'), flush=True)


if __name__ == '__main__':
    main()
