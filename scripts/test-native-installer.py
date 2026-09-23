#!/usr/bin/env python3
"""Offline executable tests for bootstrap integrity, selection and release completeness."""
import hashlib
import functools
import http.server
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import threading
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('release', ROOT / 'scripts/build/native-release.py')
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='zork release tests ')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.version = release.version()
        self.assets = self.root / 'download' / f'v{self.version}'
        self.assets.mkdir(parents=True)
        latest = self.root / 'latest/download'
        latest.mkdir(parents=True)
        (latest / 'VERSION').write_text(self.version + '\n')
        self.tools = self.root / 'tools'
        self.tools.mkdir()
        self.marker = self.root / 'executed'
        self.env = dict(os.environ, PATH=f'{self.tools}:/usr/bin:/bin',
                        ZORK_TEST_ARGS=str(self.marker), TMPDIR=str(self.root))
        self.tool('uname', 'case "$1" in -s) echo Darwin;; -m) echo arm64;; esac')
        self.tool('sw_vers', 'echo 26.0')
        self.tool('getconf', 'echo "glibc 2.39"')
        self.archive()

    def tool(self, name, body):
        path = self.tools / name
        path.write_text('#!/bin/sh\n' + body + '\n')
        path.chmod(0o755)

    def archive(self, key='darwin-arm64', omit=None, extra=None, wrong_version=False):
        archive = self.assets / f'zork-{self.version}-{key}.tar.gz'
        with tarfile.open(archive, 'w:gz') as tar:
            for name in release.MEMBERS:
                if name == omit:
                    continue
                data = b'license\n'
                if name in release.COMPONENTS:
                    data = b'#!/bin/sh\nprintf "%s\\n" "$@" > "$ZORK_TEST_ARGS"\nexit "${ZORK_TEST_EXIT:-0}"\n'
                if name == 'VERSION':
                    data = (('0.0.0' if wrong_version else self.version) + '\n').encode()
                info = tarfile.TarInfo(name)
                info.mode = 0o755 if name in release.COMPONENTS else 0o644
                info.size = len(data)
                tar.addfile(info, io.BytesIO(data))
            if extra:
                tar.addfile(extra)
        self.write_manifest(key, archive)
        return archive

    def write_manifest(self, key, archive):
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        row = f'{key}\t{archive.name}\t{digest}\t{release.PLATFORMS[key]}\n'
        (self.assets / 'manifest.tsv').write_text(row)
        (self.assets / f'{key}.tsv').write_text(row)

    def run_installer(self, args=(), success=True):
        result = subprocess.run(['/bin/sh', str(ROOT / 'scripts/install.sh'),
                                 '--base-url', self.root.as_uri(), *args],
                                env=self.env, capture_output=True, text=True, timeout=15)
        self.assertEqual(result.returncode == 0, success, result.stderr)
        self.assertFalse(list(self.root.glob('zork-install.*')), 'Bootstrap temporary files leaked')
        if not success:
            self.assertFalse(self.marker.exists(), 'Unverified code executed')
        return result

    def test_download_only_verifies_without_executing(self):
        destination = self.root / 'staged bundle'
        self.run_installer(['--download-only', str(destination)])
        self.assertFalse(self.marker.exists())
        self.assertEqual({p.name for p in destination.iterdir()}, set(release.MEMBERS))
        self.assertEqual((destination / 'VERSION').read_text().strip(), self.version)
        self.run_installer(['--download-only', str(destination)], success=False)

    def test_http_source_requires_explicit_test_channel(self):
        class QuietHandler(http.server.SimpleHTTPRequestHandler):
            def log_message(self, *_):
                pass
        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), functools.partial(QuietHandler, directory=str(self.root)))
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            base = ['--base-url', f'http://127.0.0.1:{server.server_port}', '--version', self.version]
            self.run_installer(base, success=False)
            self.run_installer([*base, '--allow-http-test', '--', 'install', '--channel', 'release'], success=False)
            self.run_installer([*base, '--allow-http-test', '--download-only', str(self.root / 'test-bundle'), '--', 'install', '--channel', 'test'])
            self.assertFalse(self.marker.exists())
        finally:
            server.shutdown()
            server.server_close()
            thread.join()

    def test_download_only_rejects_corruption_without_creating_destination(self):
        destination = self.root / 'staged'
        archive = self.archive()
        archive.write_bytes(archive.read_bytes() + b'corrupt')
        self.run_installer(['--download-only', str(destination)], success=False)
        self.assertFalse(destination.exists())

    def test_latest_default_install_without_node(self):
        self.run_installer()
        self.assertEqual(self.marker.read_text(), 'install\n')

    def test_all_platforms_and_pinned_argument_forwarding(self):
        for key in release.PLATFORMS:
            with self.subTest(platform=key):
                system, arch = key.split('-')
                self.tool('uname', f'case "$1" in -s) echo {"Darwin" if system == "darwin" else "Linux"};; -m) echo {"x86_64" if arch == "x64" else "aarch64"};; esac')
                self.archive(key)
                self.run_installer(['--version', 'v' + self.version, '--', 'mesh', 'join', "ticket'with spaces", '--data', '/tmp/with spaces'])
                self.assertEqual(self.marker.read_text(), "mesh\njoin\nticket'with spaces\n--data\n/tmp/with spaces\n")

    def test_corrupt_download_never_executes(self):
        archive = self.archive()
        archive.write_bytes(archive.read_bytes() + b'corruption')
        self.assertIn('SHA-256 mismatch', self.run_installer(success=False).stderr)

    def test_missing_component(self):
        self.archive(omit='zork-agent')
        self.assertIn('Unexpected package contents', self.run_installer(success=False).stderr)

    def test_path_traversal(self):
        self.archive(extra=tarfile.TarInfo('../escaped'))
        self.run_installer(success=False)
        self.assertFalse((self.root / 'escaped').exists())

    def test_symlink_rejected_even_with_valid_member_names(self):
        link = tarfile.TarInfo('zork-agent')
        link.type = tarfile.SYMTYPE
        link.linkname = '/bin/sh'
        self.archive(omit='zork-agent', extra=link)
        self.assertIn('links or special files', self.run_installer(success=False).stderr)

    def test_archive_version_mismatch(self):
        self.archive(wrong_version=True)
        self.assertIn('Package version mismatch', self.run_installer(success=False).stderr)

    def test_old_macos(self):
        self.tool('sw_vers', 'echo 14.7')
        self.assertIn('macOS 15.0', self.run_installer(success=False).stderr)

    def test_old_glibc_and_musl(self):
        self.tool('uname', 'case "$1" in -s) echo Linux;; -m) echo arm64;; esac')
        self.archive('linux-arm64')
        self.tool('getconf', 'echo "glibc 2.38"')
        self.assertIn('glibc 2.39', self.run_installer(success=False).stderr)
        self.tool('getconf', 'exit 1')
        self.assertIn('musl/Alpine', self.run_installer(success=False).stderr)

    def test_duplicate_platform_metadata(self):
        manifest = self.assets / 'manifest.tsv'
        manifest.write_text(manifest.read_text() * 2)
        self.assertIn('exactly one package', self.run_installer(success=False).stderr)

    def test_unsupported_cpu_and_invalid_version(self):
        self.tool('uname', 'case "$1" in -s) echo Linux;; -m) echo riscv64;; esac')
        self.assertIn('Supported CPUs', self.run_installer(success=False).stderr)
        self.tool('uname', 'case "$1" in -s) echo Darwin;; -m) echo arm64;; esac')
        self.run_installer(['--version', '../../unsafe'], success=False)

    def test_compatible_station_reused_without_download(self):
        self.tool('zork', 'if [ "$1" = capabilities ]; then echo \'{"mesh_join":1}\'; else printf "%s\\n" "$@" > "$ZORK_TEST_ARGS"; fi')
        (self.root / 'latest/download/VERSION').unlink()
        self.run_installer(['--', 'mesh', 'join', 'invitation'])
        self.assertEqual(self.marker.read_text(), 'mesh\njoin\ninvitation\n')

    def test_native_exit_status_propagates_and_cleans_up(self):
        self.env['ZORK_TEST_EXIT'] = '42'
        result = subprocess.run(['/bin/sh', str(ROOT / 'scripts/install.sh'), '--base-url', self.root.as_uri()],
                                env=self.env, capture_output=True, timeout=15)
        self.assertEqual(result.returncode, 42)
        self.assertFalse(list(self.root.glob('zork-install.*')))

    def test_assembly_requires_all_platforms_and_matching_checksums(self):
        args = SimpleNamespace(output=self.assets)
        with self.assertRaises(FileNotFoundError):
            release.assemble(args)
        for key in release.PLATFORMS:
            self.archive(key)
        release.assemble(args)
        self.assertEqual(len((self.assets / 'manifest.tsv').read_text().splitlines()), 4)
        self.assertEqual(len((self.assets / 'SHA256SUMS').read_text().splitlines()), 7)
        (self.assets / f'zork-{self.version}-linux-x64.tar.gz').write_bytes(b'corrupt')
        with self.assertRaises(ValueError):
            release.assemble(args)

    def test_stage_never_falls_back_to_debug(self):
        with patch.object(release, 'ROOT', self.root), \
                patch.object(release, 'version', return_value=self.version), \
                patch.object(release, 'build_environment', return_value={'CARGO_TARGET_DIR': str(self.root / 'target')}):
            with self.assertRaisesRegex(ValueError, 'Missing release binary'):
                release.stage(SimpleNamespace(output=self.assets))


if __name__ == '__main__':
    unittest.main(verbosity=2)
