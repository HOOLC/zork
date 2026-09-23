#!/usr/bin/env python3
"""Package the standalone desktop app."""
import argparse
import importlib.util
import hashlib
import json
import os
import plistlib
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import sys
sys.path.insert(0, str(Path(__file__).resolve().parent / "lib"))
from test_app_slot import app_slot, run_test
from build_env import build_environment
from cua_build import ensure_runtime, verify_runtime
def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()

# Helper identifiers follow the product name. This pass moves them to the project
# domain and to the Station wording, so an installed app re-registers its helpers
# once; the binary names stay as they are.
HELPERS = [('zork', 'ZorkSupervisor', 'supervisor'), ('zork-station', 'ZorkStation', 'station'),
           ('zork-service-watch', 'ZorkServiceWatch', 'service-watch')]
COMPONENTS = ['zork-gui', 'zork', 'zork-station', 'zork-agent', 'zork-gh']
RUNTIME_ALIASES = {'zork-service-watch': 'zork-station'}


def copy_binary(source, destination):
    if sys.platform == 'darwin':
        # A clone is an independent inode; signing never mutates the build input.
        subprocess.run(['cp', '-c', '-p', str(source.resolve()), str(destination)], check=True)
    else:
        shutil.copy2(source, destination)


def app_info(version, prefix='ing.zork', channel='release'):
    name = 'Zork Dev' if channel == 'dev' else 'Zork'
    icon = 'ZorkDev.icns' if channel == 'dev' else 'Zork.icns'
    return {'CFBundleIdentifier': prefix + '.desktop', 'CFBundleName': name,
            'CFBundleDisplayName': name, 'CFBundleIconFile': icon,
            'CFBundleExecutable': 'zork-gui', 'CFBundlePackageType': 'APPL',
            'CFBundleShortVersionString': version, 'CFBundleVersion': version,
            'LSMinimumSystemVersion': '26.0', 'NSHighResolutionCapable': True,
            'NSPrincipalClass': 'NSApplication',
            'NSLocalNetworkUsageDescription': '用于发现并连接同一网络中的已配对设备，同步消息和任务。',
            'NSBonjourServices': ['_zork-mesh-v1._udp', '_zork-enrollment-v1._udp']}


def verify_app(app):
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True)
    info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
    if info['CFBundleExecutable'] != 'zork-gui':
        raise RuntimeError('The GUI must be the signed main executable for macOS notifications')
    signature = subprocess.run(['codesign', '-dvv', str(app / 'Contents/MacOS/zork-gui')],
                               check=True, capture_output=True, text=True).stderr
    if ('Identifier=' + info['CFBundleIdentifier']) not in signature.splitlines() or 'Info.plist=not bound' in signature:
        raise RuntimeError('The GUI code signature must bind the application bundle identifier and Info.plist')


def sign_app(app, signer, identity):
    # Signing the bundle binds Info.plist and the product identity to the GUI.
    # A script that execs a separately signed GUI leaves UserNotifications seeing
    # two different application identities, even when --verify --deep succeeds.
    for binary in (app / 'Contents/MacOS').iterdir():
        if binary.name != 'zork-gui' and not binary.is_symlink():
            signer(binary, identity)
    signer(app, identity)
    verify_app(app)


def stage_binaries(app, binaries, assets, version, launcher, prefix='ing.zork'):
    mac = app / 'Contents/MacOS'
    helpers = []
    for name in COMPONENTS:
        if name not in {item[0] for item in HELPERS}:
            copy_binary(binaries / name, mac / name)
    for name, bundle_name, role in HELPERS:
        display_name = 'Zork-Service-Watch' if name == 'zork-service-watch' else bundle_name.replace('Zork', 'Zork-', 1)
        bundle = app / 'Contents/Helpers' / (bundle_name + '.app')
        executable_dir = bundle / 'Contents/MacOS'
        resources = bundle / 'Contents/Resources'
        executable_dir.mkdir(parents=True)
        resources.mkdir()
        if name in RUNTIME_ALIASES:
            # Reuse Station's guard entry without duplicating the large binary.
            runtime = app / 'Contents/Helpers/ZorkStation.app/Contents/MacOS' / RUNTIME_ALIASES[name]
            (executable_dir / name).symlink_to(os.path.relpath(runtime, executable_dir))
        else:
            copy_binary(binaries / name, executable_dir / name)
        entry = 'ZorkHelperLauncher' if name in RUNTIME_ALIASES else name
        if entry == 'ZorkHelperLauncher':
            shutil.copy2(launcher, executable_dir / entry)
        shutil.copy2(assets / (bundle_name + '.icns'), resources / (bundle_name + '.icns'))
        with (bundle / 'Contents/Info.plist').open('wb') as output:
            plistlib.dump({'CFBundleIdentifier': prefix + '.desktop.' + role,
                          'CFBundleName': display_name,
                          'CFBundleDisplayName': display_name,
                          'CFBundleExecutable': entry,
                          **({'ZorkRuntimeExecutable': name} if entry == 'ZorkHelperLauncher' else {}),
                          'CFBundlePackageType': 'APPL',
                          'CFBundleIconFile': bundle_name + '.icns',
                          'CFBundleShortVersionString': version, 'CFBundleVersion': version,
                          'LSBackgroundOnly': True, 'LSMinimumSystemVersion': '26.0'}, output)
        # Keep the CLI entry points and sibling discovery used by the runtime.
        (mac / name).symlink_to(os.path.relpath(executable_dir / entry, mac))
        for sibling in [*COMPONENTS, *RUNTIME_ALIASES]:
            if sibling != name:
                (executable_dir / sibling).symlink_to(os.path.relpath(mac / sibling, executable_dir))
        helpers.append(bundle)
    return helpers

def build_app(args, repo, app):
    prefix = args.id_prefix
    channel = getattr(args, 'channel', 'release')
    if (prefix == 'ing.zork-dev' and channel != 'dev') or (prefix == 'ing.zork' and channel != 'release'):
        raise RuntimeError('Bundle identity and data channel must match; set --channel explicitly')
    version=json.loads((repo/'packages/zork/package.json').read_text())['version']
    mac=app/'Contents/MacOS';resources=app/'Contents/Resources'
    mac.mkdir(parents=True);resources.mkdir()
    icon = app_info(version, prefix, channel)['CFBundleIconFile']
    shutil.copyfile(repo/'crates/zork-ui/assets/app'/icon,resources/icon)
    (resources/'licenses').mkdir()
    shutil.copyfile(repo/'crates/zork-mesh/LICENSE.synchronicity',resources/'licenses/Synchronicity.txt')
    shutil.copyfile(repo/'crates/zork-ui/LICENSE.qrcode',resources/'licenses/QRCode.txt')
    shutil.copyfile(repo/'crates/zork-mesh/LICENSE.flate2',resources/'licenses/flate2.txt')
    binaries=args.bin_dir or Path(build_environment()['CARGO_TARGET_DIR'])/'debug'
    with tempfile.TemporaryDirectory(prefix='zork-helper-launcher-') as scratch:
        helper_launcher = Path(scratch) / 'ZorkHelperLauncher'
        subprocess.run(['clang', '-arch', 'arm64', '-mmacosx-version-min=26.0',
                        str(repo/'scripts/build/macos-helper-launcher.m'),
                        '-framework', 'AppKit', '-framework', 'ApplicationServices', '-o', str(helper_launcher)], check=True)
        helpers = stage_binaries(app, binaries, repo/'crates/zork-ui/assets/app', version, helper_launcher, prefix)
    spec = importlib.util.spec_from_file_location('browser_runtime', repo / 'scripts/lib/browser-runtime.py')
    browser_runtime = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(browser_runtime)
    signing_identity = browser_runtime.signing_identity()
    browser_runtime.stage_runtime((args.browser_bin_dir or binaries).resolve(), app / 'Contents/Helpers', prefix)
    cua = getattr(args, 'cua_runtime', None) or ensure_runtime(repo)
    record = getattr(args, 'build_record', None)
    build = json.loads(record.read_text()) if record else None
    if build is not None:
        expected = build.get('external_runtimes', {}).get('cua')
        if not isinstance(expected, dict):
            raise RuntimeError('Build record has no captured desktop runtime; rebuild the app candidate')
        verify_runtime(repo, cua, expected)
    spec = importlib.util.spec_from_file_location('cua_runtime', repo / 'scripts/lib/cua-runtime.py')
    cua_runtime = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(cua_runtime)
    cua_runtime.stage(repo, cua, app, prefix, browser_runtime.sign, signing_identity,
                      minimum_system_version=app_info(version, prefix, channel)['LSMinimumSystemVersion'])

    if args.services_config:
        services=json.loads(args.services_config.read_text())
        assert isinstance(services,dict) and not set(services)-{'relay_urls','relay_quic_port','discovery_url','quic_discovery_urls'}
        (resources/'services.json').write_text(json.dumps(services,indent=2)+'\n')
    with (app/'Contents/Info.plist').open('wb') as f:
        plistlib.dump(app_info(version, prefix, channel), f)
    (resources/'channel').write_text(channel+'\n')
    if build is not None:
        for name, expected in build['binaries'].items():
            if digest(binaries/name) != expected:
                raise RuntimeError('Binary differs from the captured Cargo build: '+name)
        (resources/'build.json').write_text(json.dumps(build, indent=2)+'\n')
    (resources/'README.txt').write_text('Zork desktop. The local node starts only when enabled. Keep Station running after quitting is available in Node settings; independently installed Stations outlive the client.\nPublic service defaults: services.json. Device overrides: services.json in the selected channel client data directory.\nZork accounts are optional and do not control Mesh connectivity. Model credentials are configured on each node.\n')
    for helper in helpers:
        # Native entries are the helper's main executable and are signed with
        # its Info.plist here. Service-watch reuses the already signed Station.
        browser_runtime.sign(helper, signing_identity)
    sign_app(app, browser_runtime.sign, signing_identity)


def main():
    parser=argparse.ArgumentParser(description='Update the one persistent app for this worktree')
    parser.add_argument('--services-config',type=Path,help='Public service defaults; no credentials')
    parser.add_argument('--channel',choices=['release','dev'],default='release',help='Signed data and identity channel')
    parser.add_argument('--id-prefix',help='Override bundle prefix for isolated test identities')
    parser.add_argument('--build-record',type=Path,help='Captured Cargo and native desktop runtime provenance and input digests')
    parser.add_argument('--bin-dir',type=Path)
    parser.add_argument('--browser-bin-dir',type=Path)
    parser.add_argument('--cua-runtime',type=Path,help='Verified native cua input; otherwise build/reuse the pinned runtime cache')
    parser.add_argument('--output',type=Path,help='Explicitly export an archive to this directory')
    parser.add_argument('--launch',action='store_true',help='Launch after update even if not previously running')
    parser.add_argument('--run',nargs=argparse.REMAINDER,help='Run tests against the updated {app}; retain the app afterward')
    args=parser.parse_args()
    args.id_prefix = args.id_prefix or ('ing.zork-dev' if args.channel == 'dev' else 'ing.zork')
    if args.channel == 'dev' and args.id_prefix == 'ing.zork':
        parser.error('dev cannot use the release bundle identity')
    if args.run == []:
        parser.error('--run requires a command')
    repo=Path(__file__).resolve().parents[1]
    with app_slot(repo) as slot:
        build_app(args, repo, slot.staged)
        if args.output:
            args.output.mkdir(parents=True,exist_ok=True)
            archive=args.output/'Zork-macOS-arm64.tar.gz'
            with tempfile.TemporaryDirectory(prefix='.package-',dir=args.output) as scratch:
                staged=Path(scratch)/archive.name
                with tarfile.open(staged,'w:gz') as tar:tar.add(slot.staged,arcname='Zork.app')
                digest=hashlib.sha256(staged.read_bytes()).hexdigest()
                os.replace(staged,archive)
            archive.with_suffix(archive.suffix+'.sha256').write_text(digest+'  '+archive.name+'\n')
            print(str(archive));print('SHA-256 '+digest)
        app=slot.publish(launch=args.launch, restart=not bool(args.run))
        if args.run:
            run_test(args.run, app)
        print('Worktree app: '+str(app))


if __name__=='__main__':main()
