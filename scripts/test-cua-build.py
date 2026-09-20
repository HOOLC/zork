"""Exercise cached runtime resolution without network, input injection or TCC changes."""
import json
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/lib'))
import cua_build


class RuntimeInputs(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        self.spec = cua_build.specification(ROOT)
        self.env = {'ZORK_BUILD_ROOT': str(self.root), 'ZORK_CUA_SOURCE': str(self.root / 'source'),
                    'ZORK_CUA_TARGET_DIR': str(self.root / 'existing-cua-target'), 'GIT_DIR': '/wrong/repository'}
        self.runtime = self.root / 'cua/runtime' / self.spec.REVISION / platform.machine()

    def stage(self, directory):
        directory.mkdir(parents=True, exist_ok=True)
        (directory / 'cua-driver').write_bytes(b'non-executable fixture')
        (directory / 'LICENSE.cua').write_text('fixture license')
        record = {'version': self.spec.VERSION, 'revision': self.spec.REVISION, 'repository': self.spec.REPOSITORY,
                  'sha256': {name: self.spec.digest(directory / name) for name in ('cua-driver', 'LICENSE.cua')}}
        (directory / 'cua-driver.json').write_text(json.dumps(record))

    def test_verified_cache_is_reused_without_network_or_build(self):
        self.stage(self.runtime)
        with patch.object(cua_build.subprocess, 'run', side_effect=AssertionError('unexpected build')):
            self.assertEqual(cua_build.ensure_runtime(ROOT, self.env), self.runtime.resolve())

    def test_damaged_cache_is_not_executed_or_silently_replaced(self):
        self.stage(self.runtime)
        (self.runtime / 'cua-driver').write_bytes(b'changed')
        with patch.object(cua_build.subprocess, 'run', side_effect=AssertionError('unexpected build')):
            with self.assertRaisesRegex(RuntimeError, 'input changed'):
                cua_build.ensure_runtime(ROOT, self.env)

    def test_valid_runtime_cannot_replace_another_captured_input(self):
        self.stage(self.runtime)
        _, record = cua_build.verify_runtime(ROOT, self.runtime)
        record['sha256']['cua-driver'] = 'another-build'
        with self.assertRaisesRegex(RuntimeError, 'differs from the captured build'):
            cua_build.verify_runtime(ROOT, self.runtime, record)

    def test_configured_source_and_target_produce_a_verified_atomic_input(self):
        def build(argv, **kwargs):
            self.assertEqual(argv[argv.index('--source') + 1], self.env['ZORK_CUA_SOURCE'])
            self.assertEqual(argv[argv.index('--target-dir') + 1], self.env['ZORK_CUA_TARGET_DIR'])
            self.assertNotIn('GIT_DIR', kwargs['env'])
            self.stage(Path(argv[argv.index('--output') + 1]))
        with patch.object(cua_build.subprocess, 'run', side_effect=build):
            self.assertEqual(cua_build.ensure_runtime(ROOT, self.env), self.runtime)
        self.assertFalse(self.runtime.with_name(self.runtime.name + '.pending').exists())

    def test_failed_build_never_becomes_a_runtime(self):
        with patch.object(cua_build.subprocess, 'run', side_effect=subprocess.CalledProcessError(1, 'cargo')):
            with self.assertRaises(subprocess.CalledProcessError):
                cua_build.ensure_runtime(ROOT, self.env)
        self.assertFalse(self.runtime.exists())


if __name__ == '__main__':
    unittest.main()
