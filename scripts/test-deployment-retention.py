#!/usr/bin/env python3
"""Deployment store retention: selection rules, safety stops and LaunchServices cleanup.

Set ZORK_TEST_LSREGISTER=1 on a Mac to also register a throwaway app copy with the
real LaunchServices database and verify the sweep unregisters it.
"""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/lib'))
import deployment_retention as retention

NOW = 1_800_000_000.0
DAY = 86400.0


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value))


def touch(path, stamp):
    os.utime(path, (stamp, stamp))


def plist(path, identifier):
    path.mkdir(parents=True, exist_ok=True)
    (path / 'Info.plist').write_text(
        '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>'
        f'<key>CFBundleIdentifier</key><string>{identifier}</string>'
        '<key>CFBundlePackageType</key><string>APPL</string></dict></plist>')


class FakeLaunchServices(retention.LaunchServices):
    def __init__(self, registered=()):
        self.calls = []
        self.registered_paths = list(registered)
        super().__init__(runner=self.fake, platform='darwin')

    def fake(self, argv):
        self.calls.append(argv)
        if argv[1] == '-dump':
            text = ''.join(f'path:                       {p} (0x{i:x})\n' for i, p in enumerate(self.registered_paths))
            return type('Result', (), {'stdout': text, 'returncode': 0})()
        if argv[1] == '-u':
            self.registered_paths = [p for p in self.registered_paths if p not in argv[2:]]
        return type('Result', (), {'stdout': '', 'returncode': 0})()

    def unregistered(self):
        return [p for argv in self.calls if argv[1] == '-u' for p in argv[2:]]


class Store:
    def __init__(self, root):
        self.root = root
        root.mkdir(parents=True, exist_ok=True)
        write(root / 'deployment-config.json', {'repo': '', 'channels': {
            'dev': {'node': {'data': str(root / 'dev/data'), 'payload': str(root / 'dev/bin')},
                    'app': {'data': str(root / 'client-dev'), 'payload': str(root / 'apps/Zork Dev.app')}},
            'release': {'node': {'data': str(root / 'release-data'), 'payload': str(root / 'release-bin')}}}})

    def candidate(self, name, channel='dev', kind='node', age=10 * DAY, app=False):
        path = self.root / 'candidates' / name
        write(path / 'deployment.json', {'id': name, 'channel': channel, 'kind': kind, 'schema': 1})
        if app:
            plist(path / 'Zork.app/Contents', 'ing.zork-dev.desktop')
            plist(path / 'Zork.app/Contents/Helpers/ZorkStation.app/Contents', 'ing.zork-dev.desktop.station')
        touch(path / 'deployment.json', NOW - age)
        touch(path, NOW - age)
        return path

    def record(self, name, channel='dev', kind='node'):
        return {'id': name, 'channel': channel, 'kind': kind}

    def transaction(self, name, candidate, previous=None, phase='accepted', channel='dev', kind='node', age=5 * DAY, app=False):
        path = self.root / 'transactions' / name
        write(path / 'journal.json', {'schema': 1, 'phase': phase, 'created_at': NOW - age, 'updated_at': NOW - age,
            'runtime': {'channel': channel, 'kind': kind, 'candidate': str(self.root / 'candidates' / candidate),
                        'candidate_id': candidate,
                        'previous_active': {'record': self.record(previous, channel, kind)} if previous else None}})
        (path / 'snapshot-0').mkdir()
        if app:
            plist(path / 'displaced-payload/Contents', 'ing.zork-dev.desktop')
            plist(path / 'snapshot-1/Contents', 'ing.zork-dev.desktop')
        return path

    def active(self, candidate, transaction=None, channel='dev', kind='node', device=None, age=1 * DAY, extra=None):
        path = (self.root / 'devices' / device / (channel + '-active.json') if device
                else self.root / channel / (kind + '-active.json'))
        value = {'candidate': str(self.root / 'candidates' / candidate), 'record': self.record(candidate, channel, kind),
                 'transaction': str(self.root / 'transactions' / transaction) if transaction else None,
                 'accepted_at': NOW - age}
        value.update(extra or {})
        write(path, value)
        return path

    def plan(self):
        return retention.plan(self.root, now=NOW)


def names(pairs):
    return sorted(Path(p).name for p, _ in pairs)


class RetentionSelectionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='zork-retention-')
        self.store = Store(Path(self.temporary.name) / 'Zork')

    def tearDown(self):
        self.temporary.cleanup()

    def node_history(self):
        s = self.store
        for index, name in enumerate(['a', 'b', 'c', 'd']):
            s.candidate(name, age=(10 - index) * DAY)
        s.transaction('t1', 'a', age=9 * DAY)
        s.transaction('t2', 'b', 'a', age=8 * DAY)
        s.transaction('t3', 'c', 'b', age=7 * DAY)
        s.transaction('t4', 'd', 'c', age=6 * DAY)
        s.active('d', 't4')

    def test_keeps_active_previous_and_latest_accepted_transaction(self):
        self.node_history()
        plan = self.store.plan()
        self.assertIsNone(plan.blocked)
        deleted = names(plan.delete)
        self.assertEqual(deleted, ['a', 'b', 't1', 't2', 't3'])
        kept = dict((Path(p).name, r) for p, r in plan.keep)
        self.assertIn('active', kept['d'])
        self.assertIn('previous', kept['c'])
        self.assertIn('named by active', kept['t4'])

    def test_unfinished_and_unreadable_transactions_and_their_candidates_are_kept(self):
        self.node_history()
        self.store.transaction('t0', 'a', phase='applying', age=9.5 * DAY)
        broken = self.store.root / 'transactions/t-broken'
        broken.mkdir()
        (broken / 'journal.json').write_text('{not json')
        self.store.transaction('t-failed', 'b', 'a', phase='recovery_failed', age=20 * DAY)
        plan = self.store.plan()
        kept = names(plan.keep)
        for name in ('t0', 't-broken', 't-failed', 'a', 'b'):
            self.assertIn(name, kept)
        self.assertEqual(names(plan.delete), ['t1', 't2', 't3'])

    def test_rolled_back_transactions_are_kept_for_an_investigation_window(self):
        self.node_history()
        self.store.transaction('t5-recent', 'b', 'd', phase='rolled_back', age=2 * DAY)
        self.store.transaction('t0-old', 'b', 'a', phase='rolled_back', age=30 * DAY)
        plan = self.store.plan()
        self.assertIn('t5-recent', names(plan.keep))
        self.assertIn('t0-old', names(plan.delete))

    def test_pending_transaction_blocks_all_deletion(self):
        self.node_history()
        write(self.store.root / 'dev/node-pending.json', {'transaction': str(self.store.root / 'transactions/t4')})
        plan = self.store.plan()
        self.assertIn('Interrupted', plan.blocked)
        self.assertEqual(plan.delete, [])

    def test_corrupt_receipt_blocks_all_deletion(self):
        self.node_history()
        (self.store.root / 'release').mkdir()
        (self.store.root / 'release/node-active.json').write_text('{"record": ')
        plan = self.store.plan()
        self.assertIn('Unreadable receipt', plan.blocked)
        self.assertEqual(plan.delete, [])
        write(self.store.root / 'release/node-active.json', {'candidate': 'x'})
        self.assertIn('without a build record', self.store.plan().blocked)

    def test_missing_receipts_keep_everything(self):
        self.node_history()
        (self.store.root / 'dev/node-active.json').unlink()
        plan = self.store.plan()
        self.assertIn('No active receipt', plan.blocked)
        self.assertEqual(plan.delete, [])
        outcome = retention.apply(plan, FakeLaunchServices())
        self.assertEqual(outcome['deleted'], [])
        self.assertEqual(len(list((self.store.root / 'candidates').iterdir())), 4)

    def test_unreadable_configuration_blocks_all_deletion(self):
        self.node_history()
        (self.store.root / 'deployment-config.json').write_text('[')
        self.assertIn('configuration', self.store.plan().blocked)

    def test_multiple_channels_kinds_and_devices(self):
        s = self.store
        # Local dev node, local release node.
        self.node_history()
        s.candidate('r1', channel='release', age=20 * DAY)
        s.candidate('r2', channel='release', age=19 * DAY)
        s.candidate('r3', channel='release', age=18 * DAY)
        s.transaction('tr3', 'r3', 'r2', channel='release', age=18 * DAY)
        s.active('r3', 'tr3', channel='release')
        # Two Macs on dev app with different active builds; no local transactions for them.
        for index, name in enumerate(['m1', 'm2', 'm3', 'm4', 'm5']):
            s.candidate(name, kind='app', age=(15 - index) * DAY, app=True)
        s.active('m5', 'remote-tx', kind='app', device='mba.local')
        s.active('m2', 'remote-tx', kind='app', device='mini.local')
        plan = s.plan()
        kept, deleted = names(plan.keep), names(plan.delete)
        for name in ('d', 'c', 'r3', 'r2', 'm5', 'm4', 'm2', 'm1'):
            self.assertIn(name, kept, name)
        for name in ('a', 'b', 'r1', 'm3'):
            self.assertIn(name, deleted, name)
        self.assertNotIn('d', deleted)

    def test_recorded_device_previous_wins_over_the_age_heuristic(self):
        s = self.store
        for index, name in enumerate(['m1', 'm2', 'm3']):
            s.candidate(name, kind='app', age=(5 - index) * DAY)
        s.active('m3', kind='app', device='mba.local')
        write(s.root / 'devices/mba.local/dev-previous.json',
              {'candidate': str(s.root / 'candidates/m1'), 'record': s.record('m1', kind='app')})
        plan = s.plan()
        self.assertEqual(names(plan.delete), ['m2'])
        self.assertIn('m1', names(plan.keep))

    def test_undeployed_candidates_are_kept(self):
        self.node_history()
        s = self.store
        s.candidate('newer', age=0.5 * DAY)          # built after the active deployment
        s.candidate('fresh', age=60)                 # just built
        s.candidate('t-only', channel='test', age=30 * DAY)  # channel never deployed here
        (s.root / 'candidates/garbage').mkdir()
        plan = s.plan()
        kept = dict((Path(p).name, r) for p, r in plan.keep)
        self.assertIn('not yet deployed', kept['newer'])
        self.assertIn('last hour', kept['fresh'])
        self.assertIn('never deployed', kept['t-only'])
        self.assertIn('unreadable', kept['garbage'])

    def test_staging_tools_and_backups(self):
        self.node_history()
        s = self.store
        old_build = s.root / 'candidates/.build-old'
        new_build = s.root / 'candidates/.build-new'
        for path, age in ((old_build, 2 * DAY), (new_build, 60)):
            path.mkdir()
            (path / 'build.log').write_text('log')
            touch(path, NOW - age)
        install_old = s.root / 'staging.noindex/install-old'
        install_old.mkdir(parents=True)
        touch(install_old, NOW - 2 * DAY)
        legacy = s.root / '.install-legacy'
        legacy.mkdir()
        touch(legacy, NOW - 2 * DAY)
        tools = s.root / 'tools'
        for name, age in (('recovery-old', 9), ('recovery-wrapped', 5), ('recovery-newest', 1),
                          ('installer-aaaa', 9),
                          ('installer-zzzz', 2), ('other-tool', 30)):
            (tools / name).mkdir(parents=True)
            touch(tools / name, NOW - age * DAY)
        (s.root / 'bin').mkdir()
        (s.root / 'bin/zork-node').write_text(f"os.execv(x, [x, '{tools}/recovery-wrapped/dev/recovery.py'])\n")
        backups = s.root / 'backups/dev'
        for stamp in ('20260901', '20260902', '20260903', '20260904', '20260905'):
            (backups / stamp).mkdir(parents=True)
        plan = s.plan()
        deleted, kept = names(plan.delete), names(plan.keep)
        self.assertIn('.build-old', deleted)
        self.assertIn('.build-new', kept)
        self.assertIn('install-old', deleted)
        self.assertIn('.install-legacy', deleted)
        self.assertIn('recovery-old', deleted)
        self.assertIn('recovery-wrapped', kept)
        self.assertIn('recovery-newest', kept)
        self.assertIn('installer-aaaa', deleted)
        self.assertIn('installer-zzzz', kept)   # newest
        self.assertIn('other-tool', kept)
        self.assertEqual([n for n in deleted if n.startswith('2026')], ['20260901', '20260902'])

    def test_installer_of_a_kept_candidate_and_receipt_references_are_kept(self):
        s = self.store
        s.candidate('abcdef01', kind='app', age=3 * DAY)
        s.candidate('12345678', kind='app', age=2 * DAY)
        s.active('12345678', kind='app', device='mba.local', extra={
            'recovery_argv': ['python3', str(s.root / 'tools/installer-99999999/dev/recovery.py')]})
        for name, age in (('installer-abcdef', 5), ('installer-99999999', 6), ('installer-11111111', 7), ('installer-1234', 1)):
            (s.root / 'tools' / name).mkdir(parents=True)
            touch(s.root / 'tools' / name, NOW - age * DAY)
        plan = s.plan()
        self.assertEqual([n for n in names(plan.delete) if n.startswith('installer')], ['installer-11111111'])

    def test_apply_deletes_unregisters_migrates_and_keeps_paths_working(self):
        s = self.store
        for index, name in enumerate(['m1', 'm2', 'm3']):
            s.candidate(name, kind='app', age=(5 - index) * DAY, app=True)
        s.transaction('t2', 'm2', 'm1', kind='app', age=4 * DAY, app=True)
        s.transaction('t3', 'm3', 'm2', kind='app', age=3 * DAY, app=True)
        receipt = s.active('m3', 't3', kind='app')
        installed = s.root / 'apps/Zork Dev.app'
        plist(installed / 'Contents', 'ing.zork-dev.desktop')
        outside = str(Path(self.temporary.name) / 'Elsewhere/Zork.app')
        stale = str(s.root / 'candidates/long-gone/Zork.app')
        ls = FakeLaunchServices([str(installed), outside, stale,
                                 str(s.root / 'candidates/m1/Zork.app'),
                                 str(s.root / 'transactions/t3/displaced-payload')])
        plan = s.plan()
        self.assertEqual(names(plan.delete), ['m1', 't2'])
        outcome = retention.apply(plan, ls)
        self.assertEqual(outcome['errors'], [])
        self.assertEqual(sorted(Path(d['path']).name for d in outcome['deleted']), ['m1', 't2'])
        unregistered = ls.unregistered()
        # Bundles of deleted copies are unregistered before deletion.
        self.assertTrue(any(p.endswith('candidates/m1/Zork.app/Contents/Helpers/ZorkStation.app') for p in unregistered))
        # Kept copies (now below *.noindex), helpers and stale registrations are unregistered too.
        self.assertTrue(any(p.endswith('candidates.noindex/m3/Zork.app') for p in unregistered), unregistered)
        self.assertTrue(any(p.endswith('ZorkStation.app') for p in unregistered))
        self.assertTrue(any(p.endswith('transactions.noindex/t3/displaced-payload') for p in unregistered))
        self.assertIn(stale, unregistered)
        self.assertNotIn(str(installed), unregistered)
        self.assertNotIn(outside, unregistered)
        # Layout: real directories are *.noindex; recorded paths still resolve.
        for name in retention.STORE_DIRS:
            self.assertTrue((s.root / name).is_symlink())
            self.assertTrue((s.root / (name + '.noindex')).is_dir())
        active = json.loads(receipt.read_text())
        self.assertTrue((Path(active['candidate']) / 'deployment.json').is_file())
        self.assertTrue((Path(active['transaction']) / 'journal.json').is_file())
        # A second run is a no-op for data and keeps the layout.
        again = retention.apply(s.plan(), FakeLaunchServices())
        self.assertEqual(again['deleted'], [])
        self.assertEqual(again['migrated'], [])

    def test_noindex_layout_resumes_after_interruption(self):
        root = self.store.root
        (root / 'transactions.noindex').mkdir()
        retention.ensure_noindex_layout(root)
        self.assertEqual(os.readlink(root / 'transactions'), 'transactions.noindex')
        self.assertEqual(os.readlink(root / 'candidates'), 'candidates.noindex')
        # Both present as real directories: left untouched.
        other = Path(self.temporary.name) / 'Other'
        (other / 'candidates').mkdir(parents=True)
        (other / 'candidates.noindex').mkdir()
        retention.ensure_noindex_layout(other)
        self.assertFalse((other / 'candidates').is_symlink())

    def test_refuses_to_delete_outside_the_store_or_configured_storage(self):
        s = self.store
        configured = retention.configured_paths(s.root)
        (s.root / 'dev/data').mkdir(parents=True)
        with self.assertRaises(RuntimeError):
            retention.check_deletable(s.root, s.root / 'dev/data', configured)
        with self.assertRaises(RuntimeError):
            retention.check_deletable(s.root, s.root / 'candidates', configured)
        bad = s.root / 'candidates/x'
        bad.mkdir(parents=True)
        write(s.root / 'deployment-config.json', {'channels': {'dev': {'node': {'data': str(bad / 'data'), 'payload': str(s.root / 'p')}}}})
        with self.assertRaises(RuntimeError):
            retention.check_deletable(s.root, bad, retention.configured_paths(s.root))

    def test_blocked_run_still_sweeps_launch_services(self):
        s = self.store
        s.candidate('m1', kind='app', app=True)
        ls = FakeLaunchServices()
        plan = s.plan()
        self.assertIsNotNone(plan.blocked)
        outcome = retention.apply(plan, ls)
        self.assertEqual(outcome['deleted'], [])
        self.assertTrue(any(p.endswith('m1/Zork.app') for p in ls.unregistered()))

    def test_dry_run_reports_without_changes(self):
        self.node_history()
        before = sorted(p.name for p in (self.store.root / 'candidates').iterdir())
        report = retention.prune(self.store.root, launch_services=FakeLaunchServices(), now=NOW)
        self.assertNotIn('applied', report)
        self.assertEqual(sorted(Path(d['path']).name for d in report['delete']), ['a', 'b', 't1', 't2', 't3'])
        self.assertGreater(report['reclaim_bytes'], 0)
        self.assertEqual(sorted(p.name for p in (self.store.root / 'candidates').iterdir()), before)
        self.assertFalse((self.store.root / 'candidates').is_symlink())
        self.assertIn('delete', retention.render(report))

    def test_no_launch_services_off_macos(self):
        ls = retention.LaunchServices(platform='linux')
        self.assertFalse(ls.available)
        self.assertEqual(ls.registered(), [])
        self.assertEqual(ls.unregister(['/x']), [])

    def test_management_wrappers_include_prune(self):
        spec = importlib.util.spec_from_file_location('recovery', ROOT / 'scripts/dev/recovery.py')
        recovery = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(recovery)
        result = recovery.install_tools(self.store.root, ROOT)
        self.assertTrue((self.store.root / 'bin/zork-prune').is_file())
        self.assertTrue((Path(result['tools']) / 'lib/deployment_retention.py').is_file())


class InstallerIntegrationTests(unittest.TestCase):
    """install-macos-client.py stores the candidate below *.noindex and prunes after accepting."""

    def test_installer_prunes_after_accept(self):
        import tarfile
        from unittest.mock import patch
        from deployment import digest, manifest
        with tempfile.TemporaryDirectory(prefix='zork-installer-retention-') as folder:
            base = Path(folder)
            root = base / 'Zork'
            store = Store(root)
            now = time.time()
            for index, name in enumerate(['old1', 'old2', 'prev']):
                path = store.candidate(name, kind='app', age=0, app=True)
                stamp = now - (5 - index) * DAY
                os.utime(path / 'deployment.json', (stamp, stamp))
            write(root / 'dev/app-active.json', {'candidate': str(root / 'candidates/prev'),
                                                 'record': store.record('prev', kind='app'), 'accepted_at': now - DAY})
            for name, age in (('recovery-old', 9), ('installer-old', 9)):
                (root / 'tools' / name).mkdir(parents=True)
                os.utime(root / 'tools' / name, (now - age * DAY,) * 2)
            candidate = base / 'build/candidate'
            plist(candidate / 'Zork.app/Contents', 'ing.zork-dev.desktop')
            record = manifest(candidate / 'Zork.app', {'id': 'source'}, 'dev', 'app')
            (candidate / 'deployment.json').write_text(json.dumps(record))
            stage = base / 'stage'
            stage.mkdir()
            with tarfile.open(stage / 'app.tar.gz', 'w:gz') as package:
                package.add(candidate, arcname='candidate')
            spec = importlib.util.spec_from_file_location('installer_under_test', ROOT / 'scripts/lib/install-macos-client.py')
            installer = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(installer)
            recovery = installer.recovery_module()

            def accept(root_path, channel, candidate_dir):
                active = {'candidate': str(Path(candidate_dir).resolve()), 'record': record,
                          'transaction': str(root_path / 'transactions/t-new'), 'health': {}, 'accepted_at': time.time()}
                (root_path / 'transactions/t-new').mkdir(parents=True)
                write(root_path / 'transactions/t-new/journal.json', {'phase': 'accepted', 'created_at': time.time(),
                      'runtime': {'channel': 'dev', 'kind': 'app', 'candidate_id': record['id'],
                                  'previous_active': {'record': store.record('prev', kind='app')}}})
                write(root_path / 'dev/app-active.json', active)
                return active

            fake = FakeLaunchServices()
            argv = ['install-macos-client.py', str(stage), digest(stage / 'app.tar.gz'), '--channel', 'dev',
                    '--root', str(root), '--profile', 'p', '--model', 'm']
            with patch.object(sys, 'argv', argv), patch.object(installer, 'validate_app'), \
                    patch.object(installer, 'recovery_module', return_value=recovery), \
                    patch.object(recovery, 'apply_candidate', side_effect=accept), \
                    patch.object(retention, 'LaunchServices', return_value=fake):
                installer.main()
            self.assertTrue((root / 'candidates').is_symlink())
            remaining = sorted(p.name for p in (root / 'candidates.noindex').iterdir())
            self.assertEqual(remaining, sorted([record['id'], 'prev']))
            self.assertTrue((stage / 'result.json').is_file())
            tools = sorted(p.name for p in (root / 'tools').iterdir())
            self.assertNotIn('recovery-old', tools)
            self.assertNotIn('installer-old', tools)
            self.assertIn('installer-' + record['id'][:16], tools)
            self.assertTrue(any(p.endswith('/prev/Zork.app') for p in fake.unregistered()))


@unittest.skipUnless(sys.platform == 'darwin' and os.environ.get('ZORK_TEST_LSREGISTER') == '1',
                     'set ZORK_TEST_LSREGISTER=1 on a Mac to touch the real LaunchServices database')
class RealLaunchServicesTests(unittest.TestCase):
    def test_registered_store_copy_is_unregistered_and_deleted(self):
        ls = retention.LaunchServices()
        folder = Path(tempfile.mkdtemp(prefix='zork-ls-probe-', dir=Path.home()))
        try:
            store = Store(folder / 'Zork')
            identifier = 'ing.zork.retention-probe.' + os.urandom(4).hex()
            for index, name in enumerate(['p1', 'p2', 'p3']):
                path = store.candidate(name, kind='app', age=(5 - index) * DAY)
                app = path / 'Zork.app'
                (app / 'Contents/MacOS').mkdir(parents=True)
                (app / 'Contents/Info.plist').write_text(
                    '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>'
                    f'<key>CFBundleIdentifier</key><string>{identifier}.{name}</string>'
                    '<key>CFBundleExecutable</key><string>run</string>'
                    '<key>CFBundlePackageType</key><string>APPL</string></dict></plist>')
                (app / 'Contents/MacOS/run').write_text('#!/bin/sh\n')
                (app / 'Contents/MacOS/run').chmod(0o755)
                subprocess.run([retention.LSREGISTER, '-f', str(app)], check=True)
            # The configured installed app stays registered; the previous installed
            # bundle, moved into a transaction as replace_directory does, must not.
            installed = store.root / 'apps/Zork Dev.app'
            shutil.copytree(store.root / 'candidates/p3/Zork.app', installed, symlinks=True)
            displaced_source = folder / 'Applications/Zork Dev.app'
            shutil.copytree(store.root / 'candidates/p2/Zork.app', displaced_source, symlinks=True)
            subprocess.run([retention.LSREGISTER, '-f', str(installed), str(displaced_source)], check=True)
            store.transaction('t3', 'p3', 'p2', kind='app', age=0.5 * DAY)
            shutil.move(str(displaced_source), store.root / 'transactions/t3/displaced-payload')
            store.active('p3', 't3', kind='app')
            registered = [p for p in ls.registered() if str(folder) in p]
            self.assertEqual(len(registered), 5, registered)
            report = retention.prune(store.root, do_apply=True, now=NOW)
            self.assertEqual([Path(d['path']).name for d in report['applied']['deleted']], ['p1'])
            remaining = [p for p in ls.registered() if str(folder) in p or str(folder.resolve()) in p]
            self.assertEqual([Path(p).name for p in remaining], ['Zork Dev.app'], remaining)
            self.assertTrue((store.root / 'candidates').is_symlink())
        finally:
            for app in folder.rglob('*.app'):
                subprocess.run([retention.LSREGISTER, '-u', str(app)], capture_output=True)
            shutil.rmtree(folder)


if __name__ == '__main__':
    unittest.main()
