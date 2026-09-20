"""Stage the pinned native desktop driver in a separately signed permission host."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess

VERSION = '0.28.2'
REVISION = 'b7f7e2d8714609853a29c7d049140bc46aec0954'
REPOSITORY = 'https://github.com/trycua/cua'
# Pinned source: scripts/_install-rust.sh has both Darwin targets;
# rust/.cargo/config.toml sets the default link target to macOS 13.
# Capability-specific availability (e.g. recording on 15) does not redefine
# the GUI product's platform promise. Read actual per-slice load commands.
UPSTREAM_ARCHITECTURES = {'arm64', 'x86_64'}


def version(value):
    if not re.fullmatch(r'[0-9]+(?:\.[0-9]+){0,2}', str(value)):
        raise RuntimeError('Invalid macOS minimum version: ' + str(value))
    parts = tuple(map(int, str(value).split('.')))
    return parts + (0,) * (3-len(parts))


def macho_requirements(binary):
    archs = subprocess.check_output(['lipo', '-archs', str(binary)], text=True).split()
    if not archs or not set(archs) <= UPSTREAM_ARCHITECTURES:
        raise RuntimeError('Unsupported cua/GUI Mach-O architecture: ' + repr(archs))
    result = {}
    for arch in archs:
        output = subprocess.check_output(['xcrun', 'vtool', '-arch', arch, '-show-build', str(binary)], text=True)
        if 'cmd LC_BUILD_VERSION' in output and re.search(r'platform\s+MACOS\b', output):
            minimum = re.search(r'\bminos\s+([0-9.]+)', output)
        elif 'cmd LC_VERSION_MIN_MACOSX' in output:
            minimum = re.search(r'\bversion\s+([0-9.]+)', output)
        else:
            minimum = None
        if minimum is None:
            raise RuntimeError(f'Missing macOS deployment target: {binary} ({arch})')
        result[arch] = minimum.group(1)
    return result


def compatibility(gui, driver, candidate_minimum):
    gui_targets, driver_targets = macho_requirements(gui), macho_requirements(driver)
    missing = gui_targets.keys() - driver_targets.keys()
    if missing:
        raise RuntimeError('cua driver missing GUI architecture(s): ' + ', '.join(sorted(missing)))
    minimum = max([*gui_targets.values(),
                   *(driver_targets[arch] for arch in gui_targets)], key=version)
    if version(minimum) > version(candidate_minimum):
        raise RuntimeError(f'cua/GUI requires macOS {minimum}, candidate promises {candidate_minimum}; rebuild compatible inputs or explicitly change the candidate requirement')
    return sorted(gui_targets), str(candidate_minimum)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def stage(repo, runtime, app, prefix, sign, identity, *, minimum_system_version=None):
    runtime = Path(runtime)
    manifest = json.loads((runtime / 'cua-driver.json').read_text())
    if (manifest.get('version'), manifest.get('revision'), manifest.get('repository')) != (VERSION, REVISION, REPOSITORY):
        raise RuntimeError('cua runtime provenance/version mismatch; rebuild with scripts/build-cua-driver.py')
    for name in ('cua-driver', 'LICENSE.cua'):
        if digest(runtime / name) != manifest['sha256'][name]:
            raise RuntimeError('cua runtime digest mismatch: ' + name)
    if minimum_system_version is None:
        minimum_system_version = plistlib.loads((app / 'Contents/Info.plist').read_bytes())['LSMinimumSystemVersion']
    archs, minimum = compatibility(app / 'Contents/MacOS/zork-gui', runtime / 'cua-driver', minimum_system_version)
    bundle = app / 'Contents/Helpers/ZorkDesktopControl.app'
    mac = bundle / 'Contents/MacOS'
    resources = bundle / 'Contents/Resources'
    mac.mkdir(parents=True)
    resources.mkdir()
    # Separate inode: signing must not modify the input artifact.
    shutil.copy2(runtime / 'cua-driver', mac / 'cua-driver')
    shutil.copy2(runtime / 'LICENSE.cua', resources / 'LICENSE.cua')
    shutil.copy2(runtime / 'cua-driver.json', resources / 'cua-driver.json')
    architecture_flags = [flag for arch in archs for flag in ('-arch', arch)]
    subprocess.run(['clang', *architecture_flags, '-mmacosx-version-min='+minimum, '-fobjc-arc', '-fblocks',
                    str(repo / 'scripts/build/macos-cua-host.m'), '-framework', 'AppKit',
                    '-framework', 'ApplicationServices', '-o', str(mac / 'zork-cua-host')], check=True)
    host_targets = macho_requirements(mac / 'zork-cua-host')
    if set(host_targets) != set(archs) or any(version(value) > version(minimum) for value in host_targets.values()):
        raise RuntimeError('Compiled cua host does not satisfy candidate architectures/minimum')
    info = {'CFBundleIdentifier': prefix + '.desktop.computer', 'CFBundleName': 'Zork Desktop Control',
            'CFBundleDisplayName': 'Zork Desktop Control', 'CFBundleExecutable': 'zork-cua-host',
            'CFBundlePackageType': 'APPL', 'CFBundleVersion': VERSION, 'CFBundleShortVersionString': VERSION,
            'LSUIElement': True, 'LSMinimumSystemVersion': minimum, 'ZorkCuaVersion': VERSION,
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
