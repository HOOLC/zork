#!/usr/bin/env python3
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

from lib.ci_source_stamps import run


class SourceStamps(unittest.TestCase):
    def setUp(self):
        self.fixture = tempfile.TemporaryDirectory()
        self.addCleanup(self.fixture.cleanup)
        self.root = Path(self.fixture.name)
        subprocess.run(["git", "init", "-q", self.root], check=True)
        self.source = self.root / "source.rs"
        self.source.write_text("old source")
        os.utime(self.source, ns=(1_000_000_000, 1_000_000_000))
        subprocess.run(["git", "add", "source.rs"], cwd=self.root, check=True)
        run(self.root, "capture")

    def checkout_time(self):
        os.utime(self.source, ns=(9_000_000_000, 9_000_000_000))

    def test_identical_checkout_recovers_cached_time(self):
        self.checkout_time()
        run(self.root, "restore")
        self.assertEqual(self.source.stat().st_mtime_ns, 1_000_000_000)

    def test_same_size_changed_input_keeps_fresh_time(self):
        self.source.write_text("new source")
        self.checkout_time()
        run(self.root, "restore")
        self.assertEqual(self.source.stat().st_mtime_ns, 9_000_000_000)

    def test_permissions_are_part_of_input_identity(self):
        self.source.chmod(0o755)
        self.checkout_time()
        run(self.root, "restore")
        self.assertEqual(self.source.stat().st_mtime_ns, 9_000_000_000)

    def test_symlink_replacement_does_not_touch_external_input(self):
        external = self.root / "outside.rs"
        external.write_text("old source")
        os.utime(external, ns=(9_000_000_000, 9_000_000_000))
        self.source.unlink()
        self.source.symlink_to(external)
        run(self.root, "restore")
        self.assertEqual(external.stat().st_mtime_ns, 9_000_000_000)


if __name__ == "__main__":
    unittest.main()
