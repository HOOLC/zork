#!/usr/bin/env python3
"""Build the native Zork design browser and optionally export component evidence."""
import argparse
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/lib"))
from build_env import build_environment


def package_app(binary: Path, destination: Path) -> None:
    if destination.exists():
        raise FileExistsError(f"Refusing to replace an existing design app: {destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    version = json.loads((ROOT / "packages/zork/package.json").read_text())["version"]
    with tempfile.TemporaryDirectory(prefix=".zork-design-pc-", dir=destination.parent) as scratch:
        staged = Path(scratch) / destination.name
        mac = staged / "Contents/MacOS"
        resources = staged / "Contents/Resources"
        mac.mkdir(parents=True)
        resources.mkdir()
        shutil.copy2(binary, mac / "zork-design-pc")
        shutil.copy2(ROOT / "crates/zork-ui/assets/app/ZorkDesign.icns", resources / "ZorkDesign.icns")
        info = {
            "CFBundleIdentifier": "ing.zork.design-pc",
            "CFBundleName": "Zork Design PC",
            "CFBundleDisplayName": "Zork Design PC",
            "CFBundleExecutable": "zork-design-pc",
            "CFBundleIconFile": "ZorkDesign.icns",
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": version,
            "CFBundleVersion": version,
            "LSApplicationCategoryType": "public.app-category.developer-tools",
            "LSMinimumSystemVersion": "26.0",
            "NSHighResolutionCapable": True,
        }
        with (staged / "Contents/Info.plist").open("wb") as output:
            plistlib.dump(info, output)
        subprocess.run(["codesign", "--force", "--sign", "-", str(staged)], check=True)
        subprocess.run(["codesign", "--verify", "--strict", str(staged)], check=True)
        os.replace(staged, destination)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--export", type=Path, help="Export native component snapshots to this directory")
    parser.add_argument("--verify", action="store_true", help="Exercise the built native app")
    parser.add_argument("--open", action="store_true", help="Open the native design window after building")
    parser.add_argument("--package-app", type=Path, help="Create a signed Zork Design PC.app at this new path")
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
    if args.package_app:
        package_app(binary, args.package_app.resolve())
        print("Packaged design app:", args.package_app.resolve())
    print("Native design app ready:", binary)
    if args.open:
        if args.package_app:
            subprocess.run(["open", str(args.package_app.resolve())], cwd=ROOT, env=env, check=True)
        else:
            subprocess.run([str(binary)], cwd=ROOT, env=env, check=True)


if __name__ == "__main__":
    main()
