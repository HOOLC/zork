#!/usr/bin/env python3
"""Destructive fault injection stays in isolated temporary roots, never user channels."""
import errno
from contextlib import closing
import io
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
import tarfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/lib'))
import deployment
from deployment import Transaction, atomic_json, inventory, manifest, replace_directory, verify_manifest
from deployment_build import stability, source_stamp
from deployment_macos import AppRuntime
from deployment_health import NodeRuntime, renew_restored_epochs

spec = importlib.util.spec_from_file_location('recovery', ROOT / 'scripts/dev/recovery.py')
recovery = importlib.util.module_from_spec(spec)
spec.loader.exec_module(recovery)


class Runtime:
    def __init__(self, data, binary):
        self.data, self.binary = data, binary
        self.running = True
        self.fail_start = False
        self.fail_recovery = False

    def capture(self):
        return {'running': self.running}

    def stop(self):
        self.running = False

    def start(self, original, candidate=False):
        if candidate and self.fail_start:
            (self.data / 'config.json').write_text('{"new_schema":true}')
            (self.data / 'identity').unlink()
            (self.data / 'new-history').write_text('written by failed version')
            raise RuntimeError('injected startup failure after migration')
        if not candidate and self.fail_recovery:
            raise RuntimeError('injected old-version startup failure')
        self.running = candidate or original['running']

    def verify_original(self, original):
        if self.running != original['running']:
            raise RuntimeError('running state was not restored')
        return {'restored': True}

    def after_restore(self):
        return {}

    def health(self):
        return {'chat': 'unit fixture'}

    def verify_preserved(self, _):
        return {}


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix='zork-recovery-test-')
        self.root = Path(self.scratch.name).resolve()
        self.data, self.binary = self.root / 'data', self.root / 'bin'
        self.data.mkdir()
        self.binary.mkdir()
        (self.data / 'config.json').write_text('{"bind":{"gateway":"old"}}')
        (self.data / 'identity').write_text('persistent identity')
        (self.data / 'history').write_text('conversation before cutover')
        (self.binary / 'zork').write_text('old executable')
        self.candidate = self.root / 'candidate'
        self.candidate.mkdir()
        (self.candidate / 'zork').write_text('new executable')
        self.before = [inventory(p) for p in (self.data, self.binary)]
        self.runtime = Runtime(self.data, self.binary)

    def tearDown(self):
        self.scratch.cleanup()

    def transaction(self):
        return Transaction(self.root / 'transaction', self.runtime, [self.data, self.binary], {})

    def apply(self, tx):
        (self.data / 'config.json').write_text('{"bind":{"station":"new"}}')
        replace_directory(self.candidate, self.binary, tx)

    def unchanged(self):
        self.assertEqual(self.before, [inventory(p) for p in (self.data, self.binary)])
        self.assertTrue(self.runtime.running)

    def test_backup_io_failure_aborts_before_migration(self):
        tx = self.transaction()
        with patch.object(deployment, 'copy_tree', side_effect=OSError(errno.ENOSPC, 'disk full')):
            with self.assertRaises(OSError):
                tx.run(self.apply, lambda: {'chat': True})
        self.assertFalse(tx.journal.get('backup_complete', False))
        self.assertEqual(tx.journal['phase'], 'rolled_back')
        self.unchanged()

    def test_partial_backup_is_not_a_recovery_point(self):
        tx = self.transaction()
        real_copy = deployment.copy_tree
        def partial(source, destination):
            if Path(source) == self.binary:
                raise PermissionError('second root unreadable')
            real_copy(source, destination)
        with patch.object(deployment, 'copy_tree', side_effect=partial):
            with self.assertRaises(PermissionError):
                tx.run(self.apply, lambda: {})
        self.unchanged()

    def test_full_disk_cannot_prevent_unchanged_original_from_restarting(self):
        tx = self.transaction()
        real_json = deployment.atomic_json
        def full_disk(path, value):
            if value['phase'] not in ('stopping', 'stopped'):
                raise OSError(errno.ENOSPC, 'journal disk full')
            return real_json(path, value)
        with patch.object(deployment, 'atomic_json', side_effect=full_disk):
            with self.assertRaisesRegex(RuntimeError, 'recovery failed'):
                tx.run(self.apply, lambda: {})
        self.unchanged()

    def test_startup_failure_restores_identity_config_binary_and_history(self):
        tx = self.transaction()
        self.runtime.fail_start = True
        with self.assertRaisesRegex(RuntimeError, 'startup failure'):
            tx.run(self.apply, lambda: {})
        self.unchanged()
        self.assertEqual((tx.directory / 'failed-0/new-history').read_text(), 'written by failed version')

    def test_switch_failure_after_old_payload_moved_is_recoverable(self):
        tx = self.transaction()
        real_replace = os.replace
        def failed_replace(source, target):
            if Path(target) == self.binary and '-incoming-' in str(source):
                raise OSError(errno.EIO, 'injected rename failure')
            return real_replace(source, target)
        with patch.object(os, 'replace', side_effect=failed_replace):
            with self.assertRaises(OSError):
                tx.run(self.apply, lambda: {})
        self.unchanged()

    def test_chat_failure_rolls_back_even_after_successful_launch(self):
        tx = self.transaction()
        def no_reply():
            raise RuntimeError('Agent never replied')
        with self.assertRaisesRegex(RuntimeError, 'never replied'):
            tx.run(self.apply, no_reply)
        self.unchanged()

    def test_interrupted_apply_is_recovered_from_a_reloaded_journal(self):
        tx = self.transaction()
        self.runtime.stop()
        tx.backup()
        tx.save('applying')
        self.apply(tx)
        reloaded = Transaction.load(tx.directory, self.runtime)
        reloaded.recover()
        self.unchanged()
        reloaded.recover()
        self.unchanged()

    def test_corrupt_snapshot_never_replaces_live_data(self):
        tx = self.transaction()
        self.runtime.stop()
        tx.backup()
        (tx.directory / 'snapshot-0/identity').write_text('corrupted')
        before = inventory(self.data)
        with self.assertRaisesRegex(RuntimeError, 'damaged'):
            tx.recover()
        self.assertEqual(before, inventory(self.data))

    def test_failed_recovery_retains_verified_snapshot_and_can_retry(self):
        tx = self.transaction()
        self.runtime.fail_start = self.runtime.fail_recovery = True
        with self.assertRaisesRegex(RuntimeError, 'Deployment and recovery failed'):
            tx.run(self.apply, lambda: {})
        self.assertEqual(tx.journal['phase'], 'recovery_failed')
        self.assertEqual(inventory(tx.directory / 'snapshot-0'), self.before[0])
        self.runtime.fail_recovery = False
        Transaction.load(tx.directory, self.runtime).recover()
        self.unchanged()

    def test_external_storage_link_is_rejected(self):
        outside = self.root / 'outside'
        outside.write_text('unmanaged history')
        (self.data / 'linked-history').symlink_to(outside)
        tx = self.transaction()
        with self.assertRaisesRegex(RuntimeError, 'leaves the backed-up roots'):
            tx.run(self.apply, lambda: {})
        self.assertEqual(outside.read_text(), 'unmanaged history')

    def test_payload_manifest_catches_helper_links_and_modes(self):
        alias = self.candidate / 'helper'
        alias.symlink_to('zork')
        record = manifest(self.candidate, {}, 'dev', 'node')
        verify_manifest(self.candidate, record)
        alias.unlink()
        alias.symlink_to('another-binary')
        with self.assertRaisesRegex(RuntimeError, 'Payload'):
            verify_manifest(self.candidate, record)

    def test_week_is_bound_to_successful_observations_of_the_same_build(self):
        day = 24 * 3600
        events = [{'at': n * day, 'candidate': 'a', 'passed': True} for n in range(9)]
        self.assertEqual(stability(events, 'a', now=8 * day)['observations'], 9)
        for invalid in (events[:-2], events[:3] + events[5:],
                        events[:4] + [{'at': 4 * day, 'candidate': 'a', 'passed': False}] + events[5:],
                        events[:4] + [{'at': 4 * day, 'candidate': 'b', 'passed': True}] + events[5:]):
            with self.subTest(events=invalid), self.assertRaisesRegex(RuntimeError, 'seven days'):
                stability(invalid, 'a', now=8 * day)

    def test_invalid_or_future_observation_cannot_complete_a_week(self):
        day = 24 * 3600
        events = [{'at': n * day, 'candidate': 'a', 'passed': True} for n in range(9)]
        for stamp in (float('nan'), float('inf'), 9 * day, 'invalid', None, True, 6 * day):
            with self.subTest(stamp=stamp), self.assertRaises(RuntimeError):
                stability(events[:-1] + [dict(events[-1], at=stamp)], 'a', now=8 * day)

    def test_external_preferences_are_checked_for_channel_overlap(self):
        settings = self.deployment_config()
        settings['channels']['dev']['node']['preferences'] = str(self.root / 'release/data/preferences.json')
        atomic_json(self.root / 'deployment-config.json', settings)
        with self.assertRaisesRegex(RuntimeError, 'overlaps'):
            recovery.channel_config(self.root, 'dev', 'node')

    def test_reinstall_same_candidate_restarts_observation_period(self):
        self.deployment_config()
        candidate = self.root / 'built'
        candidate.mkdir()
        deployment.copy_tree(self.candidate, candidate / 'payload')
        record = manifest(candidate / 'payload', {}, 'dev', 'node')
        atomic_json(candidate / 'deployment.json', record)
        now = 10 * 86400
        atomic_json(recovery.events_path(self.root, 'node'), [
            {'at': n * 86400, 'candidate': record['id'], 'passed': True} for n in range(3, 11)])
        with patch.object(recovery, 'runtime_for', return_value=self.runtime), patch('time.time', return_value=now + 1):
            recovery.apply_candidate(self.root, 'dev', candidate)
        with self.assertRaisesRegex(RuntimeError, 'seven days'):
            stability(json.loads(recovery.events_path(self.root, 'node').read_text()), record['id'], now=now + 1)

    def test_pending_transaction_blocks_direct_apply(self):
        self.deployment_config()
        atomic_json(self.root / 'dev/node-pending.json', {'transaction': 'existing'})
        with patch.object(recovery, 'load_candidate', return_value=({'kind': 'node'}, self.candidate)):
            with self.assertRaisesRegex(RuntimeError, 'interrupted'):
                recovery.apply_candidate(self.root, 'dev', self.candidate)
        self.unchanged()

    def test_app_rollback_restores_node_that_outlived_gui(self):
        runtime = AppRuntime({'payload': str(self.binary), 'data': str(self.data), 'channel': 'dev'}, self.root / 'app.log')
        original = {'running': False, 'node': {'running': True, 'service_loaded': False}}
        with patch.object(runtime.node, 'start') as start, patch('deployment_macos.subprocess.Popen') as gui:
            runtime.start(original)
        start.assert_called_once_with(original['node'], candidate=False)
        gui.assert_not_called()

    def test_source_stamp_ignores_inherited_git_repository(self):
        with patch.dict(os.environ, {'GIT_DIR': str(self.root / 'not-a-repo'), 'GIT_WORK_TREE': str(self.root)}):
            stamp = source_stamp(ROOT)
        self.assertEqual(len(stamp['commit']), 40)

    def test_restored_epochs_invalidate_cursors_without_changing_identity_or_messages(self):
        chat = self.data / 'chats/chat-id'
        (chat / '.zork').mkdir(parents=True)
        (chat / 'messages').mkdir()
        atomic_json(chat / '.zork/source.json', {'format': 1, 'chat_id': 'chat-id', 'epoch': 'old-chat-epoch'})
        messages = chat / 'messages/0001.jsonl'
        messages.write_text('original immutable message bytes\n')
        (self.data / 'state').mkdir()
        database = self.data / 'state/station.sqlite'
        with closing(sqlite3.connect(database)) as db:
            db.executescript("CREATE TABLE sync_meta(singleton INTEGER,epoch TEXT,sequence INTEGER);"
                "INSERT INTO sync_meta VALUES(1,'old-sync-epoch',17);"
                "CREATE TABLE sync_identity(owner TEXT); INSERT INTO sync_identity VALUES('same-owner');"
                "CREATE TABLE sync_exports(id TEXT); INSERT INTO sync_exports VALUES('old-export');"
                "CREATE TABLE receipts(value TEXT); INSERT INTO receipts VALUES('same-receipt');")
        result = renew_restored_epochs(self.data)
        self.assertEqual(result['chat_sources'], ['chat-id'])
        self.assertNotEqual(json.loads((chat / '.zork/source.json').read_text())['epoch'], 'old-chat-epoch')
        self.assertEqual(messages.read_text(), 'original immutable message bytes\n')
        self.assertEqual((self.data / 'identity').read_text(), 'persistent identity')
        with closing(sqlite3.connect(database)) as db:
            self.assertEqual(db.execute('SELECT owner FROM sync_identity').fetchone(), ('same-owner',))
            self.assertEqual(db.execute('SELECT sequence FROM sync_meta').fetchone(), (17,))
            self.assertEqual(db.execute('SELECT * FROM receipts').fetchone(), ('same-receipt',))
            self.assertEqual(db.execute('SELECT * FROM sync_exports').fetchall(), [])

    def test_gui_lease_exit_between_status_and_stop_is_not_an_unrelated_process(self):
        runtime = NodeRuntime(self.data, self.binary, self.root / 'run.log')
        with patch('deployment_health.pause_service'), patch('deployment_health.process_path', return_value=None), \
                patch.object(runtime, 'processes', return_value={}), \
                patch('deployment_health.control', return_value={'pid': 42, 'data_root': str(self.data)}) as request:
            runtime.stop()
        request.assert_called_once_with(self.data)

    def test_gui_shutdown_can_unlink_control_socket_before_supervisor_exits(self):
        runtime = NodeRuntime(self.data, self.binary, self.root / 'run.log')
        with patch('deployment_health.pause_service'), \
                patch('deployment_health.process_path', return_value=self.binary / 'zork'), \
                patch.object(runtime, 'processes', return_value={}), \
                patch('deployment_health.control', side_effect=[{'pid': 42, 'data_root': str(self.data)}, FileNotFoundError()]):
            runtime.stop()

    def test_candidate_extraction_rejects_traversal_links_and_special_files(self):
        for scenario in ('parent', 'absolute', 'symlink-parent', 'symlink-escape', 'hardlink', 'device'):
            with self.subTest(scenario=scenario):
                archive = self.root / (scenario + '.tar')
                with tarfile.open(archive, 'w') as tar:
                    member = tarfile.TarInfo({'parent': 'candidate/../../outside', 'absolute': '/outside'}.get(scenario, 'candidate/link'))
                    if scenario.startswith('symlink'):
                        member.type = tarfile.SYMTYPE
                        member.linkname = '../../outside' if scenario == 'symlink-escape' else 'directory'
                    elif scenario == 'hardlink':
                        member.type = tarfile.LNKTYPE
                        member.linkname = '/outside'
                    elif scenario == 'device':
                        member.type = tarfile.CHRTYPE
                    tar.addfile(member)
                    if scenario == 'symlink-parent':
                        tar.addfile(tarfile.TarInfo('candidate/link/file'))
                extracted = self.root / scenario
                extracted.mkdir()
                with self.assertRaises(RuntimeError):
                    deployment.extract_candidate(archive, extracted)
        self.assertFalse((self.root / 'outside').exists())

    def test_candidate_extraction_keeps_internal_framework_links(self):
        archive = self.root / 'good.tar'
        with tarfile.open(archive, 'w') as tar:
            member = tarfile.TarInfo('candidate/Versions/A/binary')
            member.size, member.mode = 4, 0o755
            tar.addfile(member, io.BytesIO(b'code'))
            member = tarfile.TarInfo('candidate/Versions/Current')
            member.type, member.linkname = tarfile.SYMTYPE, 'A'
            tar.addfile(member)
        extracted = self.root / 'good'
        extracted.mkdir()
        deployment.extract_candidate(archive, extracted)
        self.assertEqual((extracted / 'candidate/Versions/Current/binary').read_bytes(), b'code')

    def deployment_config(self):
        settings = {'repo': str(ROOT), 'channels': {
            'dev': {'node': {'data': str(self.data), 'payload': str(self.binary),
                             'health': {'profile': 'fixture', 'model': 'fixture'}}},
            'release': {'node': {'data': str(self.root / 'release/data'), 'payload': str(self.root / 'release/bin')}}}}
        atomic_json(self.root / 'deployment-config.json', settings)
        return settings

    def test_active_receipt_write_failure_also_restores_the_old_deployment(self):
        self.deployment_config()
        candidate = self.root / 'built'
        candidate.mkdir()
        deployment.copy_tree(self.candidate, candidate / 'payload')
        atomic_json(candidate / 'deployment.json', manifest(candidate / 'payload', {}, 'dev', 'node'))
        original_write = recovery.atomic_json
        def fail_receipt(path, value):
            if Path(path) == recovery.active_path(self.root, 'dev', 'node'):
                raise OSError(errno.ENOSPC, 'active receipt disk full')
            return original_write(path, value)
        with patch.object(recovery, 'runtime_for', return_value=self.runtime), patch.object(recovery, 'atomic_json', side_effect=fail_receipt):
            with self.assertRaises(OSError):
                recovery.apply_candidate(self.root, 'dev', candidate)
        self.unchanged()
        self.assertFalse((self.root / 'dev/node-pending.json').exists())

    def test_dev_data_cannot_overlap_release_binaries_or_another_release_kind(self):
        settings = self.deployment_config()
        settings['channels']['release']['app'] = {'data': str(self.root / 'release-client'), 'payload': str(self.data / 'release.app')}
        atomic_json(self.root / 'deployment-config.json', settings)
        with self.assertRaisesRegex(RuntimeError, 'overlaps'):
            recovery.channel_config(self.root, 'dev', 'node')

    def test_wrong_machine_is_rejected_before_opening_a_runtime(self):
        with self.assertRaisesRegex(RuntimeError, 'belongs to'):
            recovery.runtime_for({'host': 'not-this-device.invalid'}, self.root, 'dev', 'app')


if __name__ == '__main__':
    unittest.main(verbosity=2)
