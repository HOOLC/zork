#!/usr/bin/env python3
"""Prepare reusable Linux smoke binaries; never build a Docker image."""
import argparse
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/lib"))
from smoke_mesh_artifact import DEFAULT_IMAGE, prepare

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / ".tmp/critical-mesh")
    parser.add_argument("--runtime-image", default=DEFAULT_IMAGE, help="Existing Linux arm64 Rust image")
    parser.add_argument("--from-image", help="Import a current-HEAD Station image once, without compiling")
    args = parser.parse_args()
    prepare(ROOT, args.output, args.runtime_image, args.from_image)
