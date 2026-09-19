#!/usr/bin/env python3
"""Restore checkout mtimes only for byte-identical inputs of a cached build."""
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import sys


def inputs(root):
    paths = subprocess.check_output(["git", "ls-files", "-z"], cwd=root).split(b"\0")
    for raw in paths:
        if not raw:
            continue
        name = os.fsdecode(raw)
        path = root / name
        try:
            before = path.lstat()
            if not stat.S_ISREG(before.st_mode):
                continue
            with path.open("rb") as source:
                digest = hashlib.file_digest(source, "sha256").hexdigest()
            after = path.lstat()
        except FileNotFoundError:
            continue
        if (before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (
            after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns
        ):
            raise RuntimeError(f"build input changed while hashing: {name}")
        yield name, path, [digest, stat.S_IMODE(after.st_mode), after.st_mtime_ns]


def run(root, action):
    manifest = root / "target" / "ci-source-stamps.json"
    if action == "capture":
        records = {name: stamp for name, _, stamp in inputs(root)}
        manifest.parent.mkdir(parents=True, exist_ok=True)
        temporary = manifest.with_suffix(".tmp")
        temporary.write_text(json.dumps({"version": 1, "files": records}))
        temporary.replace(manifest)
        print(f"Captured {len(records)} build input stamps")
        return
    try:
        saved = json.loads(manifest.read_text())
        if saved.get("version") != 1 or not isinstance(saved.get("files"), dict):
            raise ValueError("unsupported manifest")
    except (FileNotFoundError, ValueError, AttributeError):
        print("No compatible build input stamps; Cargo will validate the checkout")
        return
    restored = 0
    # Enumerate the current tracked files, never paths supplied by the cache.
    for name, path, current in inputs(root):
        previous = saved["files"].get(name)
        if (isinstance(previous, list) and len(previous) == 3
                and previous[:2] == current[:2] and type(previous[2]) is int
                and 0 <= previous[2] <= current[2]):
            os.utime(path, ns=(path.stat().st_atime_ns, previous[2]))
            restored += 1
    print(f"Restored {restored} byte-identical build input stamps")


if __name__ == "__main__":
    if len(sys.argv) != 2 or sys.argv[1] not in ("capture", "restore"):
        sys.exit("usage: ci_source_stamps.py capture|restore")
    run(Path(__file__).resolve().parents[2], sys.argv[1])
