#!/usr/bin/env python3
"""Desktop delivery checks using fixed-input tests and isolated backend fixtures."""
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]

sys.path.insert(0, str(ROOT / 'scripts/lib'))
from build_env import build_environment


def main():
    env = dict(build_environment(), CARGO_INCREMENTAL='0', CARGO_PROFILE_DEV_DEBUG='0', CARGO_BUILD_JOBS='4')
    commands = [
        [sys.executable, str(ROOT / 'scripts/storybook/test_package.py')],
        ['cargo', 'run', '--locked', '-p', 'zork-gui', '--features', 'headless-bench',
         '--bin', 'zork-gui-render-bench', '--', str(ROOT / 'artifacts/headless-render/latest')],
        ['cargo', 'test', '--locked', '-p', 'zork-gui', '--features', 'headless-bench',
         '--test', 'headless_selection', '--test', 'headless_loading',
         '--test', 'headless_profile_quota'],
        ['cargo', 'test', '--locked', '-p', 'zork-gui', '--features', 'native-blur-bench',
         '--test', 'headless_modals'],
        ['cargo', 'test', '--locked', '-p', 'zork-ui', '-p', 'zork-gui'],
        [sys.executable, str(ROOT / 'scripts/test-native-installer.py')],
        [sys.executable, str(ROOT / 'scripts/test-macos-client-update.py')],
        ['cargo', 'build', '--locked', '-p', 'zork-gui', '-p', 'zork',
         '-p', 'zork-station', '-p', 'zork-agent-server', '-p', 'zork-gh'],
        [sys.executable, str(ROOT / 'scripts/test-gui-contracts.py')],
    ]
    for command in commands:
        subprocess.run(command, cwd=ROOT, env=env, check=True)
    print('Desktop headless checks passed; no GUI test windows were opened.', flush=True)


if __name__ == '__main__':
    main()
