#!/usr/bin/env python3
"""Run a test controller in the logged-in macOS GUI session (including from SSH).

Only this temporary test controller uses Launch Services. Its browser children
use the production direct-process transport and inherit the GUI security session.
"""
import argparse
import json
import os
from pathlib import Path
import plistlib
import shlex
import signal
import subprocess
import sys
import tempfile
import time

RUNNER = '''import json, os, subprocess, sys
from pathlib import Path
root = Path(sys.argv[1])
spec = json.loads((root / 'command.json').read_text())
with (root / 'output.log').open('wb') as output:
    child = subprocess.Popen(spec['argv'], cwd=spec['cwd'], stdout=output,
                             stderr=subprocess.STDOUT, start_new_session=True)
    (root / 'pid').write_text(str(child.pid))
    (root / 'result').write_text(str(child.wait()))
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--timeout', type=float, default=120)
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command
    if command[:1] == ['--']:
        command = command[1:]
    if not command:
        parser.error('a test command is required')
    if sys.platform != 'darwin':
        return subprocess.call(command)
    with tempfile.TemporaryDirectory(prefix='zork-gui-test-') as temporary:
        root = Path(temporary)
        (root / 'command.json').write_text(json.dumps({'argv': command, 'cwd': os.getcwd()}))
        (root / 'runner.py').write_text(RUNNER)
        app = root / 'TestController.app'
        mac = app / 'Contents/MacOS'
        mac.mkdir(parents=True)
        executable = mac / 'TestController'
        executable.write_text('#!/bin/sh\nexec ' + shlex.join([
            sys.executable, str(root / 'runner.py'), str(root)]) + '\n')
        executable.chmod(0o700)
        with (app / 'Contents/Info.plist').open('wb') as output:
            plistlib.dump({'CFBundleIdentifier': 'ing.zork.test.controller',
                          'CFBundleExecutable': executable.name,
                          'CFBundlePackageType': 'APPL', 'LSUIElement': True}, output)
        subprocess.run(['codesign', '--force', '--sign', '-', str(app)],
                       check=True, capture_output=True)
        try:
            subprocess.run(['open', '-n', '-W', '-g', '-a', str(app)],
                           check=True, timeout=args.timeout)
            if not (root / 'result').exists():
                raise RuntimeError('GUI test controller did not return a result')
            return int((root / 'result').read_text())
        finally:
            # Own only this test's process group, including any browser it left.
            if (root / 'pid').exists():
                group = int((root / 'pid').read_text())
                for sig in (signal.SIGTERM, signal.SIGKILL):
                    try:
                        os.killpg(group, sig)
                    except ProcessLookupError:
                        break
                    if sig == signal.SIGTERM:
                        time.sleep(.5)
            if (root / 'output.log').exists():
                print((root / 'output.log').read_text(errors='replace'), end='')
            subprocess.run([
                '/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister',
                '-u', str(app)], capture_output=True)


if __name__ == '__main__':
    raise SystemExit(main())
