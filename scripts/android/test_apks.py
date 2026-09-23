"""Reject daily-use APKs before any test installation occurs."""
import os
from pathlib import Path
import re
import subprocess


def require_test_apks(app, instrumentation):
    sdk = Path(os.environ.get('ANDROID_HOME', Path.home() / 'Library/Android/sdk'))
    tools = sorted((sdk / 'build-tools').glob('*/aapt'))
    if not tools:
        raise RuntimeError('Android build-tools/aapt is required to verify test APK identities')
    for apk, expected in ((app, 'ing.zork.android.test'),
                          (instrumentation, 'ing.zork.android.test.test')):
        output = subprocess.check_output([str(tools[-1]), 'dump', 'badging', str(apk)], text=True)
        match = re.search(r"^package: name='([^']+)'", output, re.M)
        if not match or match[1] != expected:
            raise RuntimeError(f'Refusing non-test APK: {apk}; expected {expected}')
