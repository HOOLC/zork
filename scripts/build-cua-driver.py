#!/usr/bin/env python3
"""Build the pinned cua checkout using its lockfile; stage a verifiable packaging input."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
sys.path.insert(0, str(Path(__file__).resolve().parent / 'lib'))
from build_env import build_environment


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', required=True, type=Path, help='Clean trycua/cua checkout at the pinned revision')
    parser.add_argument('--target-dir', type=Path, help='Reuse an existing cua Cargo target directory')
    parser.add_argument('--output', required=True, type=Path, help='Runtime artifact directory for --cua-runtime')
    args = parser.parse_args()
    source = args.source.resolve()
    spec = importlib.util.spec_from_file_location('cua_runtime', Path(__file__).parent / 'lib/cua-runtime.py')
    runtime = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(runtime)
    env = build_environment()
    head = subprocess.check_output(['git', '-C', str(source), 'rev-parse', 'HEAD'], env=env, text=True).strip()
    dirty = subprocess.check_output(['git', '-C', str(source), 'status', '--porcelain', '--untracked-files=no'], env=env, text=True)
    if head != runtime.REVISION or dirty:
        raise SystemExit('cua source must be clean at ' + runtime.REVISION)
    # This is a different Cargo workspace/configuration, so it owns a separate cache.
    env['CARGO_TARGET_DIR'] = str(args.target_dir.resolve() if args.target_dir else Path(env['CARGO_TARGET_DIR']).parent / 'cua-driver-target')
    env.update(CARGO_INCREMENTAL='0', CARGO_PROFILE_DEV_DEBUG='0', CARGO_BUILD_JOBS='4')
    subprocess.run(['cargo', 'build', '--locked', '--release', '-p', 'cua-driver'],
                   cwd=source / 'libs/cua-driver/rust', env=env, check=True)
    args.output.mkdir(parents=True, exist_ok=True)
    shutil.copy2(Path(env['CARGO_TARGET_DIR']) / 'release/cua-driver', args.output / 'cua-driver')
    shutil.copy2(source / 'LICENSE.md', args.output / 'LICENSE.cua')
    manifest = {'version': runtime.VERSION, 'revision': head, 'repository': runtime.REPOSITORY,
                'sha256': {name: runtime.digest(args.output / name) for name in ('cua-driver', 'LICENSE.cua')}}
    (args.output / 'cua-driver.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print(args.output)


if __name__ == '__main__':
    main()
