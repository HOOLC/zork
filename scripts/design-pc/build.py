#!/usr/bin/env python3
"""Build the native Zork design browser and optionally export component evidence."""
import argparse
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/lib"))
from build_env import build_environment


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--export", type=Path, help="Export native component snapshots to this directory")
    parser.add_argument("--verify", action="store_true", help="Exercise the built native app")
    parser.add_argument("--open", action="store_true", help="Open the native design window after building")
    args = parser.parse_args()
    env = dict(build_environment(), CARGO_INCREMENTAL="0", CARGO_PROFILE_DEV_DEBUG="0", CARGO_BUILD_JOBS="4")
    subprocess.run([sys.executable, str(ROOT / "scripts/design-pc/generate_assets.py")], cwd=ROOT, check=True)
    subprocess.run([sys.executable, str(ROOT / "scripts/design-pc/test_package.py")], cwd=ROOT, check=True)
    subprocess.run(["cargo", "build", "--locked", "-p", "zork-gui", "--features", "headless-bench", "--bin", "zork-design-pc"], cwd=ROOT, env=env, check=True)
    binary = Path(env["CARGO_TARGET_DIR"]) / "debug/zork-design-pc"
    if args.export:
        subprocess.run([str(binary), "--export", str(args.export.resolve())], cwd=ROOT, env=env, check=True)
    if args.verify:
        subprocess.run([sys.executable, str(ROOT / "scripts/design-pc/test_native_workbench.py"), "--binary", str(binary)], cwd=ROOT, env=env, check=True)
    print("Native design app ready:", binary)
    if args.open:
        subprocess.run([str(binary)], cwd=ROOT, env=env, check=True)


if __name__ == "__main__":
    main()
