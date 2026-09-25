#!/usr/bin/env python3
import importlib.util, os, sys, tempfile, time, unittest
from pathlib import Path
spec = importlib.util.spec_from_file_location('build_maintenance', Path(__file__).parent / 'lib/build_maintenance.py')
bm = importlib.util.module_from_spec(spec); spec.loader.exec_module(bm)


class AfterBuild(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.root = Path(self.dir.name)
        self.env = {'ZORK_BUILD_ROOT': str(self.root)}
        self.calls = []

    def tearDown(self):
        self.dir.cleanup()

    def run_once(self, now, env=None):
        return bm.after_build(env or self.env, now=now, spawn=lambda: self.calls.append(now))

    def test_first_build_starts_prune_then_throttles_for_a_day(self):
        self.assertTrue(self.run_once(1000))
        self.assertFalse(self.run_once(1000 + 3600))
        self.assertTrue(self.run_once(1000 + 24 * 3600))
        self.assertEqual(self.calls, [1000, 1000 + 24 * 3600])

    def test_interval_and_opt_out(self):
        env = dict(self.env, ZORK_PRUNE_INTERVAL_HOURS='1')
        self.assertTrue(self.run_once(0, env))
        self.assertTrue(self.run_once(3600, env))
        self.assertFalse(self.run_once(99999, dict(self.env, ZORK_NO_PRUNE='1')))
        self.assertEqual(len(self.calls), 2)

    def test_missing_build_root_and_spawn_failure_never_raise(self):
        self.assertFalse(bm.after_build({'ZORK_BUILD_ROOT': str(self.root / 'absent')}, now=0, spawn=lambda: self.calls.append(0)))
        def boom():
            raise OSError('no python')
        self.assertFalse(bm.after_build(self.env, now=0, spawn=boom))

    def test_concurrent_holder_skips(self):
        import fcntl
        with open(self.root / '.prune.lock', 'a') as held:
            fcntl.flock(held, fcntl.LOCK_EX)
            self.assertFalse(self.run_once(0))
        self.assertEqual(self.calls, [])


if __name__ == '__main__':
    unittest.main()
