#!/usr/bin/env python3
"""Verify backend reuse, stale artifact rejection and container build boundaries."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).parent / "lib"))
import smoke_mesh_artifact as artifact


def elf(path):
    path.write_bytes(b"\x7fELF\x02\x01" + bytes(12) + b"\xb7\x00" + b"fixture")
    path.chmod(0o755)


class ArtifactTests(unittest.TestCase):
    def test_only_backend_closure_changes_invalidate_reuse(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            subprocess.run(["git", "init", "-q", str(root)], check=True)
            packages, nodes = [], []
            for name in ["zork-station", "zork-agent-server", "core", "ui", "test-helper"]:
                directory = root / "crates" / name
                directory.mkdir(parents=True)
                (directory / "Cargo.toml").write_text(name)
                (directory / "lib.rs").write_text("original")
                packages.append({"id": name, "name": name, "version": "1.0.0", "source": None,
                                 "manifest_path": str(directory / "Cargo.toml")})
                deps = [{"pkg": "core", "dep_kinds": [{"kind": None}]}] if name == "zork-station" else []
                if name == "core":
                    deps = [{"pkg": "test-helper", "dep_kinds": [{"kind": "dev"}]}]
                nodes.append({"id": name, "deps": deps})
            for name in ["Cargo.toml", "Cargo.lock"]:
                (root / name).write_text("manifest")
            subprocess.run(["git", "-C", str(root), "add", "."], check=True)
            metadata = {"packages": packages, "resolve": {"nodes": nodes}}
            stamp = lambda: artifact.source_inputs(root, metadata)["sha256"]
            original = stamp()
            for name in ["ui", "test-helper"]:
                (root / "crates" / name / "lib.rs").write_text("changed")
            self.assertEqual(stamp(), original)
            source = root / "crates/core/lib.rs"
            source.write_text("changed")
            self.assertNotEqual(stamp(), original)
            source.write_text("original")
            added = root / "crates/core/new.rs"
            added.write_text("untracked backend input")
            self.assertNotEqual(stamp(), original)
            added.unlink()
            source.unlink()
            self.assertNotEqual(stamp(), original)
            source.write_text("original")
            (root / "Cargo.lock").write_text("new dependency")
            self.assertNotEqual(stamp(), original)

    def record(self, directory):
        for name in artifact.BINARIES:
            elf(directory / name)
        record = {"version": 1, "profile": "release", "runtime_image": "sha256:fixed",
                  "source_sha256": "backend", "binaries": artifact.binary_digests(directory)}
        (directory / "build.json").write_text(json.dumps(record))
        return record

    def test_rejects_stale_backend_tampered_binary_and_macos_executable(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            record = self.record(directory)
            self.assertEqual(artifact.validate(directory, directory, {"sha256": "backend"}), record)
            with self.assertRaisesRegex(RuntimeError, "inputs changed"):
                artifact.validate(directory, directory, {"sha256": "other"})
            binary = directory / "zork-station"
            binary.write_bytes(binary.read_bytes() + b"changed")
            with self.assertRaisesRegex(RuntimeError, "changed after capture"):
                artifact.validate(directory, directory, {"sha256": "backend"})
            binary.write_bytes(b"\xcf\xfa\xed\xfe" + bytes(20))
            with self.assertRaisesRegex(RuntimeError, "Linux arm64 ELF"):
                artifact.binary_digests(directory)

    def test_unchanged_inputs_do_not_start_a_container_or_compile(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp) / "bin"
            directory.mkdir()
            expected = self.record(directory)
            with patch.object(artifact, "source_inputs", return_value={"sha256": "backend"}), \
                 patch.object(artifact, "image_info", return_value={"Id": "sha256:fixed"}), \
                 patch.object(artifact.subprocess, "run") as run:
                self.assertEqual(artifact.prepare(Path(temp), directory), expected)
                run.assert_not_called()

    def test_changed_inputs_compile_in_existing_image_without_docker_build(self):
        with tempfile.TemporaryDirectory() as temp:
            root, directory = Path(temp), Path(temp) / "bin"
            calls = []
            def run(command, **kwargs):
                calls.append(command)
                if command[:2] == ["docker", "run"]:
                    mount = next(arg for arg in command if arg.endswith(",target=/out"))
                    stage = Path(mount.split("source=", 1)[1].split(",target=", 1)[0])
                    for name in artifact.BINARIES:
                        elf(stage / name)
            with patch.object(artifact, "source_inputs", return_value={"sha256": "backend", "packages": ["core@1.0"]}), \
                 patch.object(artifact, "image_info", return_value={"Id": "sha256:fixed"}), \
                 patch.object(artifact.subprocess, "run", side_effect=run):
                artifact.prepare(root, directory)
            self.assertFalse(any(command[:2] == ["docker", "build"] for command in calls))
            command = next(command for command in calls if command[:2] == ["docker", "run"])
            self.assertIn(f"type=bind,source={root.resolve()},target=/src,readonly", command)
            self.assertIn("sha256:fixed", command)
            self.assertIn("cargo build --locked --release", command[-1])
            self.assertEqual(artifact.validate(root, directory, {"sha256": "backend"})["runtime_image"], "sha256:fixed")


if __name__ == "__main__":
    unittest.main()
