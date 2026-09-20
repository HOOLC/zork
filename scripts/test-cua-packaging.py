#!/usr/bin/env python3
"""Native host packaging checks; no desktop capture, input injection or permission prompts."""
import importlib.util
import json
from pathlib import Path
import plistlib
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('cua_runtime', ROOT / 'scripts/lib/cua-runtime.py')
cua = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cua)


class Packaging(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix='zork-cua-package-test-')
        self.root = Path(self.scratch.name)
        self.runtime = self.root / 'runtime'
        self.runtime.mkdir()
        (self.runtime / 'cua-driver').write_text('#!/bin/sh\nexit 0\n')
        (self.runtime / 'cua-driver').chmod(0o755)
        (self.runtime / 'LICENSE.cua').write_text('TEST fixture only\n')
        self.manifest = {'version':cua.VERSION, 'revision':cua.REVISION, 'repository':cua.REPOSITORY,
                         'sha256':{name:cua.digest(self.runtime/name) for name in ('cua-driver','LICENSE.cua')}}
        (self.runtime / 'cua-driver.json').write_text(json.dumps(self.manifest))
        self.app = self.root / 'Zork.app'
        for directory in ('Contents/MacOS', 'Contents/Helpers/ZorkStation.app/Contents/MacOS'):
            (self.app / directory).mkdir(parents=True)

    def tearDown(self):
        self.scratch.cleanup()

    @staticmethod
    def sign(path, identity):
        subprocess.run(['codesign', '--force', '--sign', identity, str(path)], check=True, capture_output=True)

    def test_identity_endpoint_and_input_integrity(self):
        bundle = cua.stage(ROOT, self.runtime, self.app, 'ing.zork.cua-fixture', self.sign, '-')
        info = plistlib.loads((bundle / 'Contents/Info.plist').read_bytes())
        self.assertEqual(info['CFBundleIdentifier'], 'ing.zork.cua-fixture.desktop.computer')
        executable = bundle / 'Contents/MacOS/zork-cua-host'
        endpoint = json.loads(subprocess.check_output([str(executable), '--endpoint']))
        self.assertEqual(endpoint['bundle_id'], info['CFBundleIdentifier'])
        self.assertEqual(endpoint['driver_version'], cua.VERSION)
        self.assertLess(len(endpoint['socket'].encode()), 104)
        self.assertFalse(Path(endpoint['socket']).exists())
        for directory in ('Contents/MacOS', 'Contents/Helpers/ZorkStation.app/Contents/MacOS'):
            self.assertEqual((self.app / directory / 'zork-cua-host').resolve(), executable.resolve())
        self.assertEqual(cua.digest(self.runtime/'cua-driver'), self.manifest['sha256']['cua-driver'])
        signature = subprocess.run(['codesign', '-dvv', str(executable)], capture_output=True, text=True, check=True).stderr
        self.assertIn('Identifier='+info['CFBundleIdentifier'], signature)
        self.assertNotIn('Info.plist=not bound', signature)
        self.assertEqual((bundle / 'Contents/Resources/LICENSE.cua').read_bytes(), (self.runtime / 'LICENSE.cua').read_bytes())

    def test_tampered_runtime_rejected_before_staging(self):
        (self.runtime / 'cua-driver').write_text('changed')
        with self.assertRaisesRegex(RuntimeError, 'digest mismatch'):
            cua.stage(ROOT, self.runtime, self.app, 'test', self.sign, '-')
        self.assertFalse((self.app/'Contents/Helpers/ZorkDesktopControl.app').exists())

    def test_wrong_revision_rejected(self):
        self.manifest['revision'] = 'unverified'
        (self.runtime / 'cua-driver.json').write_text(json.dumps(self.manifest))
        with self.assertRaisesRegex(RuntimeError, 'provenance/version mismatch'):
            cua.stage(ROOT, self.runtime, self.app, 'test', self.sign, '-')


if __name__ == '__main__':
    unittest.main()
