#!/usr/bin/env python3
"""Read-only, deterministic checks for the updater's persisted node intent."""
import importlib.util
import hashlib
from pathlib import Path
import sqlite3
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('installer', Path(__file__).parent / 'lib/install-macos-client.py')
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class HelperBundleTests(unittest.TestCase):
    @unittest.skipUnless(sys.platform == 'darwin', 'APFS staging is macOS-only')
    def test_framework_staging_preserves_links_and_isolates_signing_writes(self):
        spec = importlib.util.spec_from_file_location('browser_runtime', Path(__file__).parent / 'lib/browser-runtime.py')
        runtime = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(runtime)
        with tempfile.TemporaryDirectory(prefix='zork-framework-clone-') as folder:
            root = Path(folder)
            source = root / 'Source.framework'
            source.mkdir()
            binary = source / 'runtime'
            binary.write_bytes(b'original')
            binary.chmod(0o755)
            subprocess.run(['xattr', '-w', 'zork.fixture', 'metadata', str(binary)], check=True)
            (source / 'Current').symlink_to('runtime')
            target = root / 'Copy.framework'
            runtime.copy_framework(source, target)
            self.assertEqual((target / 'Current').readlink(), Path('runtime'))
            self.assertEqual((target / 'runtime').stat().st_mode & 0o777, 0o755)
            self.assertNotEqual((target / 'runtime').stat().st_ino, binary.stat().st_ino)
            self.assertNotIn('zork.fixture', subprocess.check_output(['xattr', str(target / 'runtime')], text=True))
            (target / 'runtime').write_bytes(b'resigned copy')
            self.assertEqual(binary.read_bytes(), b'original')

    def test_bundle_entry_points_and_sibling_discovery(self):
        spec = importlib.util.spec_from_file_location('packager', Path(__file__).parent / 'package-macos-client.py')
        packager = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(packager)
        import plistlib
        with tempfile.TemporaryDirectory(prefix='zork helper layout ') as folder:
            root = Path(folder)
            binaries = root / 'bin'
            binaries.mkdir()
            for name in packager.COMPONENTS:
                (binaries / name).write_bytes(name.encode())
            app = root / 'Zork.app'
            mac = app / 'Contents/MacOS'
            mac.mkdir(parents=True)
            assets = Path(__file__).resolve().parents[1] / 'crates/zork-ui/assets/app'
            launcher = root / 'launcher'
            launcher.write_bytes(b'launcher')
            helpers = packager.stage_binaries(app, binaries, assets, '1.2.3', launcher)
            for helper, (name, bundle_name, role) in zip(helpers, packager.HELPERS):
                with (helper / 'Contents/Info.plist').open('rb') as file:
                    info = plistlib.load(file)
                self.assertTrue(info['LSBackgroundOnly'])
                wrapped = name in packager.RUNTIME_ALIASES
                self.assertEqual(info['CFBundleExecutable'], 'ZorkHelperLauncher' if wrapped else name)
                self.assertEqual(info.get('ZorkRuntimeExecutable'), name if wrapped else None)
                self.assertEqual(info['CFBundleIdentifier'], 'surf.zork.desktop.' + role)
                if name == 'zork-station':
                    self.assertEqual(info['CFBundleDisplayName'], 'Zork-Station')
                    self.assertEqual(info['CFBundleIdentifier'], 'surf.zork.desktop.gateway')
                if name == 'zork-service-watch':
                    self.assertEqual(info['CFBundleDisplayName'], 'Zork-Service-Watch')
                    self.assertTrue((helper / 'Contents/MacOS' / name).is_symlink())
                    self.assertEqual((helper / 'Contents/MacOS' / name).read_bytes(), b'zork-station')
                self.assertEqual((helper / 'Contents/Resources' / info['CFBundleIconFile']).read_bytes()[:4], b'icns')
                executable = (mac / name).resolve(strict=True)
                self.assertEqual(executable, (helper / 'Contents/MacOS' / info['CFBundleExecutable']).resolve())
                for sibling in [*packager.COMPONENTS, *packager.RUNTIME_ALIASES]:
                    expected = (b'launcher' if sibling != name and sibling in packager.RUNTIME_ALIASES
                                else packager.RUNTIME_ALIASES.get(sibling, sibling).encode())
                    self.assertEqual((executable.parent / sibling).read_bytes(), expected)

    @unittest.skipUnless(sys.platform == 'darwin', 'AppKit helper launch is macOS-only')
    def test_compatibility_symlinks_preserve_arguments(self):
        spec = importlib.util.spec_from_file_location('packager', Path(__file__).parent / 'package-macos-client.py')
        packager = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(packager)
        repo = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory(prefix='zork helper exec ') as folder:
            root = Path(folder)
            binaries = root / 'bin'
            binaries.mkdir()
            for name in packager.COMPONENTS:
                shutil.copyfile('/bin/echo', binaries / name)
                (binaries / name).chmod(0o755)
            launcher = root / 'launcher'
            subprocess.run(['clang', str(repo / 'scripts/build/macos-helper-launcher.m'),
                            '-framework', 'AppKit', '-framework', 'ApplicationServices', '-o', str(launcher)], check=True)
            app = root / 'Zork.app'
            mac = app / 'Contents/MacOS'
            mac.mkdir(parents=True)
            helpers = packager.stage_binaries(app, binaries, repo / 'crates/zork-ui/assets/app', '1.2.3', launcher)
            for helper, (name, _, _) in zip(helpers, packager.HELPERS):
                for target in [helper / 'Contents/MacOS' / name, helper]:
                    if target.is_symlink():
                        continue
                    subprocess.run(['codesign', '--force', '--sign', '-', str(target)],
                                   check=True, capture_output=True)
            try:
                for name in ['zork', 'zork-station', 'zork-service-watch']:
                    result = subprocess.run([str(mac / name), '--data', 'path with spaces', '--fake-agent'],
                                            capture_output=True, text=True, check=True, timeout=15)
                    self.assertEqual(result.stdout.strip(), '--data path with spaces --fake-agent')
            finally:
                tool = '/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister'
                for helper in helpers:
                    subprocess.run([tool, '-u', str(helper)], capture_output=True)

    def test_installer_tracks_nested_helpers_but_not_other_apps(self):
        app = Path('/Applications/Zork.app')
        commands = '\n'.join([
            str(app / 'Contents/MacOS/zork-gui'),
            str(app / 'Contents/Helpers/ZorkStation.app/Contents/MacOS/zork-station') + ' --data example',
            '/Applications/Other.app/Contents/MacOS/zork',
        ])
        with patch.object(installer.subprocess, 'check_output', return_value=commands):
            self.assertEqual(len(installer.processes(app)), 2)


class NodeIntentTests(unittest.TestCase):
    def setUp(self):
        temp = tempfile.TemporaryDirectory(prefix='zork update fixture ')
        self.addCleanup(temp.cleanup)
        self.path = Path(temp.name) / 'client.db'
        with sqlite3.connect(self.path) as db:
            db.executescript('PRAGMA journal_mode=WAL; CREATE TABLE cache(node TEXT,key TEXT,value TEXT); CREATE TABLE nodes(value TEXT);')
        db.close()

    def test_saved_disabled_intent_overrides_a_running_node(self):
        with sqlite3.connect(self.path) as db:
            db.execute("INSERT INTO cache VALUES('device','local-node-enabled','false')")
        db.close()
        before = self.path.read_bytes()
        self.assertFalse(installer.local_node_enabled(self.path, True))
        self.assertEqual(before, self.path.read_bytes())

    def test_legacy_local_node_is_preserved(self):
        with sqlite3.connect(self.path) as db:
            db.execute('INSERT INTO nodes VALUES(?)', ('{"local":true}',))
        db.close()
        self.assertTrue(installer.local_node_enabled(self.path, False))

    def test_missing_database_uses_observed_running_state(self):
        self.assertTrue(installer.local_node_enabled(self.path.with_name('missing.db'), True))

    def test_transient_open_error_retries_without_writable_fallback(self):
        original = sqlite3.connect
        calls = []
        def connect(*args, **kwargs):
            calls.append((args, kwargs))
            if len(calls) == 1:
                raise sqlite3.OperationalError('unable to open database file')
            return original(*args, **kwargs)
        with patch.object(installer.sqlite3, 'connect', side_effect=connect), patch.object(installer.time, 'sleep'):
            self.assertFalse(installer.local_node_enabled(self.path, True))
        self.assertEqual(len(calls), 2)
        self.assertTrue(all(args[0].endswith('?mode=ro') and kwargs['uri'] for args, kwargs in calls))

    def test_persistent_failure_blocks_preflight(self):
        with patch.object(installer.sqlite3, 'connect', side_effect=sqlite3.OperationalError('unavailable')) as connect, patch.object(installer.time, 'sleep'):
            with self.assertRaises(sqlite3.OperationalError):
                installer.local_node_enabled(self.path, True)
        self.assertEqual(connect.call_count, 3)


    def test_slow_cold_start_can_become_ready_after_thirty_seconds(self):
        # Exercise verify's production polling without sleeping or a live node.
        app = self.path.parent / 'Zork.app'
        config = self.path.parent / 'Library/Application Support/Zork/client/node/config.json'
        config.parent.mkdir(parents=True)
        config.write_text('{"bind":{"runtime":"127.0.0.1:60016"}}')
        clock = [0.]
        class Response:
            status = 200
            def __enter__(self): return self
            def __exit__(self, *args): pass
        def ready(*args, **kwargs):
            if clock[0] < 45:
                raise OSError('cold startup still in progress')
            return Response()
        with patch.object(installer.Path, 'home', return_value=self.path.parent), patch.object(installer, 'processes', return_value=[str(app / 'Contents/MacOS/zork-gui')]), patch.object(installer.time, 'monotonic', side_effect=lambda: clock[0]), patch.object(installer.time, 'sleep', side_effect=lambda dt: clock.__setitem__(0, clock[0] + dt)), patch.object(installer, 'urlopen', side_effect=ready):
            installer.verify(app, True)
        self.assertGreaterEqual(clock[0], 45)
        self.assertLess(clock[0], 90)


class AppRetentionTests(unittest.TestCase):
    def exercise(self, fail_start, fail_registration=False):
        with tempfile.TemporaryDirectory(prefix='zork install transaction ') as folder:
            home = Path(folder)
            stage = home / 'stage'
            stage.mkdir()
            archive = stage / 'app.tar.gz'
            archive.write_bytes(b'fixture archive')
            incoming = stage / 'Zork.app'
            incoming.mkdir()
            (incoming / 'version').write_text('new')
            current = home / 'Applications/Zork.app'
            current.mkdir(parents=True)
            (current / 'version').write_text('old')
            for app in (incoming, current):
                (app / 'Contents/Helpers/ZorkBrowser.app').mkdir(parents=True)
            data = home / 'Library/Application Support/Zork/client/data'
            data.parent.mkdir(parents=True)
            data.write_bytes(b'preserved user data')
            registrations = []
            launches = []
            def run(argv):
                if Path(argv[0]).name == 'lsregister':
                    self.assertEqual(argv[1], '-f')
                    self.assertEqual(argv[2:], [str(current.resolve()),
                        str((current / 'Contents/Helpers/ZorkBrowser.app').resolve())])
                    version = (current / 'version').read_text()
                    registrations.append(version)
                    if fail_registration and version == 'new':
                        raise RuntimeError('failed registration')
                if argv[0] == 'open':
                    self.assertEqual(registrations[-1], (current / 'version').read_text())
                    launches.append(argv[1])
            with patch.object(installer.Path, 'home', return_value=home), \
                    patch.object(installer.sys, 'argv', ['installer', str(stage), hashlib.sha256(archive.read_bytes()).hexdigest()]), \
                    patch.object(installer, 'run', side_effect=run), \
                    patch.object(installer, 'processes', return_value=[]), \
                    patch.object(installer, 'quit_app'), \
                    patch.object(installer, 'verify', side_effect=RuntimeError('failed startup') if fail_start else None):
                if fail_start or fail_registration:
                    with self.assertRaisesRegex(RuntimeError, 'failed registration' if fail_registration else 'failed startup'):
                        installer.main()
                else:
                    installer.main()
            failed = fail_start or fail_registration
            self.assertEqual(registrations, ['new', 'old'] if failed else ['new'])
            self.assertEqual(launches, [] if fail_registration else [str(current)])
            self.assertEqual((current / 'version').read_text(), 'old' if failed else 'new')
            self.assertFalse((stage / 'previous.app').exists())
            self.assertFalse(list((home / 'Applications').glob('Zork-backup-*.app')))
            self.assertEqual(data.read_bytes(), b'preserved user data')

    def test_success_leaves_no_old_app(self):
        self.exercise(False)

    def test_failure_restores_old_app_without_history_backup(self):
        self.exercise(True)

    def test_registration_failure_restores_and_registers_the_previous_app(self):
        self.exercise(False, fail_registration=True)


if __name__ == '__main__':
    unittest.main()
