"""Stage the pinned native desktop driver in a separately signed permission host."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess

VERSION = '0.28.2'
REVISION = 'b7f7e2d8714609853a29c7d049140bc46aec0954'
REPOSITORY = 'https://github.com/trycua/cua'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def stage(repo, runtime, app, prefix, sign, identity):
    runtime = Path(runtime)
    manifest = json.loads((runtime / 'cua-driver.json').read_text())
    if (manifest.get('version'), manifest.get('revision'), manifest.get('repository')) != (VERSION, REVISION, REPOSITORY):
        raise RuntimeError('cua runtime provenance/version mismatch; rebuild with scripts/build-cua-driver.py')
    for name in ('cua-driver', 'LICENSE.cua'):
        if digest(runtime / name) != manifest['sha256'][name]:
            raise RuntimeError('cua runtime digest mismatch: ' + name)
    bundle = app / 'Contents/Helpers/ZorkDesktopControl.app'
    mac = bundle / 'Contents/MacOS'
    resources = bundle / 'Contents/Resources'
    mac.mkdir(parents=True)
    resources.mkdir()
    # Separate inode: signing must not modify the input artifact.
    shutil.copy2(runtime / 'cua-driver', mac / 'cua-driver')
    shutil.copy2(runtime / 'LICENSE.cua', resources / 'LICENSE.cua')
    shutil.copy2(runtime / 'cua-driver.json', resources / 'cua-driver.json')
    subprocess.run(['clang', '-arch', 'arm64', '-mmacosx-version-min=26.0', '-fobjc-arc', '-fblocks',
                    str(repo / 'scripts/build/macos-cua-host.m'), '-framework', 'AppKit',
                    '-framework', 'ApplicationServices', '-o', str(mac / 'zork-cua-host')], check=True)
    info = {'CFBundleIdentifier': prefix + '.desktop.computer', 'CFBundleName': 'Zork Desktop Control',
            'CFBundleDisplayName': 'Zork Desktop Control', 'CFBundleExecutable': 'zork-cua-host',
            'CFBundlePackageType': 'APPL', 'CFBundleVersion': VERSION, 'CFBundleShortVersionString': VERSION,
            'LSUIElement': True, 'LSMinimumSystemVersion': '26.0', 'ZorkCuaVersion': VERSION,
            'NSScreenCaptureUsageDescription': 'Allow Zork to observe the desktop for tasks you request.',
            'NSAppleEventsUsageDescription': 'Allow Zork to control apps for tasks you request.'}
    (bundle / 'Contents/Info.plist').write_bytes(plistlib.dumps(info))
    sign(mac / 'cua-driver', identity)
    sign(bundle, identity)
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(bundle)], check=True)
    for directory in (app / 'Contents/MacOS', app / 'Contents/Helpers/ZorkStation.app/Contents/MacOS'):
        if directory.is_dir():
            (directory / 'zork-cua-host').symlink_to(os.path.relpath(mac / 'zork-cua-host', directory))
    return bundle
