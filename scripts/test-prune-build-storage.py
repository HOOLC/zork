#!/usr/bin/env python3
"""Selection rules of scripts/prune-build-storage.py, without touching real caches."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('prune_build_storage', ROOT / 'scripts/prune-build-storage.py')
prune = importlib.util.module_from_spec(spec)
spec.loader.exec_module(prune)
budget = prune.load_budget()
DAY = 86400


def age_tree(path, days):
    stamp = time.time() - days * DAY
    for folder, dirs, files in os.walk(path):
        for name in dirs + files:
            os.utime(os.path.join(folder, name), (stamp, stamp), follow_symlinks=False)
    os.utime(path, (stamp, stamp))


class TargetSelectionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='zork-prune-build-')
        base = Path(self.temporary.name)
        self.root = base / 'zork-build'
        self.live = [base / 'zork', base / 'worktrees/alive', base / 'worktrees/marked']
        for path in self.live:
            path.mkdir(parents=True)

    def tearDown(self):
        self.temporary.cleanup()

    def target(self, name, days, owner=None, tag=True, keep=False, nested=False):
        entry = self.root / 'isolated' / name
        target = entry / 'target' if nested else entry
        (target / 'debug/deps').mkdir(parents=True)
        (target / 'debug/deps/libx.rlib').write_bytes(b'x' * 4096)
        (target / '.rustc_info.json').write_text('{}')
        if tag:
            (target / 'CACHEDIR.TAG').write_text(budget.CACHEDIR_SIGNATURE + '\n')
        if owner:
            (target / '.zork-cache-owner.json').write_text(json.dumps({'worktree': str(owner)}))
        if keep:
            (target / '.zork-cache-keep').write_text('')
        age_tree(entry, days)
        return entry

    def plan(self, days=14):
        items = prune.plan_targets(self.root, days, time.time(), budget, self.live)
        return {item['name']: item for item in items}

    def test_orphaned_stale_and_protected_targets(self):
        self.target('alive', 1)
        self.target('marked-dir', 1, owner=self.live[2])           # marker names a live worktree
        self.target('gone-by-name', 1)                             # no worktree with this name
        self.target('gone-by-marker', 1, owner=Path(self.temporary.name) / 'worktrees/removed')
        self.target('old-alive', 30, nested=True)
        (self.live[0].parent / 'worktrees/old-alive').mkdir()
        self.live.append(self.live[0].parent / 'worktrees/old-alive')
        self.target('deployment', 60)
        self.target('kept', 60, keep=True)
        self.target('untagged', 60, tag=False)
        self.target('fresh-orphan', 0)
        plan = self.plan()
        action = {name: item['action'] for name, item in plan.items()}
        self.assertEqual(action['alive'], 'keep')
        self.assertEqual(action['marked-dir'], 'keep')
        self.assertEqual(action['gone-by-name'], 'delete')
        self.assertIn('no worktree named', plan['gone-by-name']['reason'])
        self.assertEqual(action['gone-by-marker'], 'delete')
        self.assertEqual(action['old-alive'], 'delete')
        self.assertIn('untouched for 30 days', plan['old-alive']['reason'])
        self.assertEqual(action['deployment'], 'keep')
        self.assertEqual(action['kept'], 'keep')
        self.assertEqual(action['untagged'], 'keep')
        self.assertEqual(action['fresh-orphan'], 'keep')

    def test_shared_target_only_by_age(self):
        shared = self.root / 'target'
        (shared / 'debug').mkdir(parents=True)
        (shared / '.rustc_info.json').write_text('{}')
        (shared / 'CACHEDIR.TAG').write_text(budget.CACHEDIR_SIGNATURE + '\n')
        age_tree(shared, 3)
        self.assertEqual(self.plan()['target']['action'], 'keep')
        age_tree(shared, 20)
        self.assertEqual(self.plan()['target']['action'], 'delete')

    def test_removal_rechecks_open_files(self):
        entry = self.target('gone', 3)
        item = {'path': str(entry)}
        with patch.object(budget, 'idle', return_value=False):
            with self.assertRaises(RuntimeError):
                prune.remove_target(self.root.resolve(), item, budget)
        self.assertTrue(entry.exists())
        with patch.object(budget, 'idle', return_value=True):
            prune.remove_target(self.root.resolve(), {'path': str(entry.resolve())}, budget)
        self.assertFalse(entry.exists())


class ArtifactSelectionTests(unittest.TestCase):
    def test_only_old_untracked_ignored_entries(self):
        with tempfile.TemporaryDirectory(prefix='zork-prune-artifacts-') as folder:
            repo = Path(folder)
            env = dict(os.environ, GIT_AUTHOR_NAME='t', GIT_AUTHOR_EMAIL='t@t', GIT_COMMITTER_NAME='t', GIT_COMMITTER_EMAIL='t@t')
            subprocess.run(['git', 'init', '-q'], cwd=repo, check=True)
            (repo / '.gitignore').write_text('artifacts/\n')
            (repo / 'artifacts/tracked').mkdir(parents=True)
            (repo / 'artifacts/tracked/keep.txt').write_text('tracked')
            subprocess.run(['git', 'add', '.gitignore'], cwd=repo, check=True)
            subprocess.run(['git', 'add', '-f', 'artifacts/tracked/keep.txt'], cwd=repo, check=True)
            subprocess.run(['git', 'commit', '-qm', 'init'], cwd=repo, check=True, env=env)
            for name, days in (('old-run', 10), ('new-run', 1)):
                (repo / 'artifacts' / name).mkdir()
                (repo / 'artifacts' / name / 'out.log').write_text('x')
                age_tree(repo / 'artifacts' / name, days)
            (repo / 'artifacts/old-inner-new').mkdir()
            (repo / 'artifacts/old-inner-new/fresh.log').write_text('x')
            os.utime(repo / 'artifacts/old-inner-new', (time.time() - 30 * DAY,) * 2)
            age_tree(repo / 'artifacts/tracked', 30)
            items = {Path(i['path']).name: i['action'] for i in prune.plan_artifacts([(repo, False)], 7, time.time())}
            self.assertEqual(items, {'old-run': 'delete', 'new-run': 'keep', 'tracked': 'keep', 'old-inner-new': 'keep'})


class KacheTests(unittest.TestCase):
    def test_usage_parsing(self):
        output = 'Store:      24.7 GiB / 20.0 GiB (12557 entries, 124%)\n'
        with patch.object(prune.subprocess, 'run', return_value=type('R', (), {'stdout': output})()):
            used, limit = prune.kache_usage('kache')
        self.assertAlmostEqual(used / 2**30, 24.7)
        self.assertAlmostEqual(limit / 2**30, 20.0)
        with patch.object(prune.subprocess, 'run', return_value=type('R', (), {'stdout': output})()):
            self.assertEqual(prune.kache_gc('kache', apply=False)['action'], 'gc')

    def test_usage_prefers_physical_size_after_dedup(self):
        output = ('Store:      24.7 GiB / 20.0 GiB (12557 entries, 124%)\n'
                  'Dedup:      27266 unique blobs, 19.9 GiB physical, 19.7% savings\n')
        with patch.object(prune.subprocess, 'run', return_value=type('R', (), {'stdout': output})()):
            used, limit = prune.kache_usage('kache')
        self.assertAlmostEqual(used / 2**30, 19.9)
        with patch.object(prune.subprocess, 'run', return_value=type('R', (), {'stdout': output})()):
            self.assertEqual(prune.kache_gc('kache', apply=False)['action'], 'none')


if __name__ == '__main__':
    unittest.main()
