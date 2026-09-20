"""Capture Cargo-reported executables in a deployment-owned build namespace."""
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import time
import uuid

from build_env import build_environment, clean_git_environment
from deployment import atomic_json, clone_file, copy_tree, digest, exclusive, manifest, verify_manifest
from cua_build import ensure_runtime, verify_runtime

NODE_PACKAGES = ['zork', 'zork-station', 'zork-agent-server', 'zork-gh']
NODE_BINARIES = ['zork', 'zork-station', 'zork-agent', 'zork-gh']


def source_stamp(repo):
    def git(*args):
        return subprocess.check_output(['git', *args], cwd=repo, env=clean_git_environment(os.environ))
    changes = git('diff', '--binary', 'HEAD', '--')
    untracked = git('ls-files', '--others', '--exclude-standard', '-z').split(b'\0')
    inputs = hashlib.sha256(changes)
    for relative in sorted(p for p in untracked if p):
        path = Path(repo) / os.fsdecode(relative)
        inputs.update(relative + b'\0')
        inputs.update(os.fsencode(os.readlink(path)) if path.is_symlink() else path.read_bytes())
    return {'commit': git('rev-parse', 'HEAD').decode().strip(),
            'tree': git('rev-parse', 'HEAD^{tree}').decode().strip(),
            'dirty': bool(changes or any(untracked)), 'changes_sha256': inputs.hexdigest()}


def load_packager(repo):
    spec = importlib.util.spec_from_file_location('deployment_packager', Path(repo) / 'scripts/package-macos-client.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def build(repo, store, kind='node', profile='dev', services=None):
    repo = Path(repo).resolve()
    env = build_environment(repo)
    if kind == 'app' and not env.get('CEF_PATH'):
        # Reuse the already downloaded SDK. cef-dll-sys validates its archive
        # version against Cargo.lock; a new capture namespace need not redownload CEF.
        for name in ('debug', 'release'):
            runtime = Path(env['CARGO_TARGET_DIR']) / name / 'zork-browser-runtime'
            if runtime.is_file():
                result = subprocess.run([str(runtime), '--cef-dir'], env=env, capture_output=True, text=True, timeout=15)
                sdk = Path(result.stdout.strip())
                if result.returncode == 0 and (sdk / 'include/cef_api_versions.h').is_file():
                    env['CEF_PATH'] = str(sdk)
                    break
    # Ordinary worktrees share target/, whose top-level executables another
    # Cargo process can replace as soon as Cargo exits, before our pipe reader
    # has copied its last compiler-artifact. Keep one reusable namespace for
    # deployments, and hold its owner lock through capture and packaging.
    base = Path(env.get('ZORK_BUILD_ROOT', repo / 'target')).expanduser()
    if not base.is_absolute():
        base = repo / base
    target = base / 'isolated/deployment'
    env['CARGO_TARGET_DIR'] = str(target.resolve())
    with exclusive(target.parent / 'deployment-capture.lock'):
        return _build(repo, store, kind, profile, services, env)


def _build(repo, store, kind, profile, services, env):
    repo, store = Path(repo).resolve(), Path(store).resolve()
    store.mkdir(parents=True, exist_ok=True, mode=0o700)
    stage = store / ('.build-' + uuid.uuid4().hex)
    stage.mkdir(mode=0o700)
    raw = stage / 'bin'
    raw.mkdir()
    source = source_stamp(repo)
    env.update(CARGO_INCREMENTAL='0', CARGO_PROFILE_DEV_DEBUG='0', CARGO_BUILD_JOBS='4')
    if sys.platform == 'darwin' and Path('/opt/homebrew/bin/cargo').exists():
        env['PATH'] = '/opt/homebrew/bin:' + env['PATH']
    packages = NODE_PACKAGES + (['zork-gui', 'zork-browser-runtime'] if kind == 'app' else [])
    names = NODE_BINARIES + (['zork-gui', 'zork-browser-runtime', 'zork-browser-helper'] if kind == 'app' else [])
    command = ['cargo', 'build', '--locked', '--message-format=json-render-diagnostics']
    for package in packages:
        command += ['-p', package]
    if profile == 'release':
        command.append('--release')
    log = stage / 'build.log'
    try:
        with log.open('w') as output:
            process = subprocess.Popen(command, cwd=repo, env=env, stdout=subprocess.PIPE,
                                       stderr=output, text=True)
            try:
                for line in process.stdout:
                    output.write(line)
                    output.flush()
                    value = json.loads(line)
                    if value.get('reason') != 'compiler-artifact' or not value.get('executable'):
                        continue
                    name = value['target']['name']
                    if name in names:
                        source_binary = Path(value['executable'])
                        target = raw / name
                        target.unlink(missing_ok=True)
                        clone_file(source_binary, target)
                        if digest(source_binary) != digest(target):
                            raise RuntimeError('Build output changed during capture: ' + name)
                if process.wait() != 0:
                    raise RuntimeError(f'Cargo build failed; running deployment unchanged. See {log}')
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=30)
        if any(not (raw / name).is_file() for name in names):
            raise RuntimeError('Cargo did not report every required executable')
        if source_stamp(repo) != source:
            raise RuntimeError('Source changed during build; candidate rejected')
        cua_runtime = None
        if kind == 'app':
            runtime, runtime_record = verify_runtime(repo, ensure_runtime(repo, env))
            cua_runtime = stage / 'cua-runtime'
            copy_tree(runtime, cua_runtime)
            verify_runtime(repo, cua_runtime)
        record = {'schema': 1, 'source': source, 'profile': profile,
                  'rustc': subprocess.check_output(['rustc', '--version'], env=env, text=True).strip(),
                  'binaries': {name: digest(raw / name) for name in names}}
        if cua_runtime:
            record['external_runtimes'] = {'cua': runtime_record}
        record['id'] = hashlib.sha256(json.dumps(record, sort_keys=True).encode()).hexdigest()
        atomic_json(stage / 'build.json', record)
        payload = stage / ('Zork.app' if kind == 'app' else 'payload')
        if kind == 'app':
            packager = load_packager(repo)
            args = type('Options', (), {'bin_dir': raw, 'browser_bin_dir': raw,
                'id_prefix': 'ing.zork-dev', 'channel': 'dev', 'services_config': services,
                'build_record': stage / 'build.json', 'cua_runtime': cua_runtime})()
            # Personal builds deliberately use the same ad-hoc signing policy throughout.
            overrides = {'ZORK_CODESIGN_IDENTITY': '-'}
            if env.get('CEF_PATH'):
                overrides['CEF_PATH'] = env['CEF_PATH']
            previous = {name: os.environ.get(name) for name in overrides}
            os.environ.update(overrides)
            try:
                packager.build_app(args, repo, payload)
                from deployment_macos import validate_app
                validate_app(payload, 'dev')
                if json.loads((payload / 'Contents/Resources/build.json').read_text()) != record:
                    raise RuntimeError('Packager did not preserve the captured build provenance')
            finally:
                for name, value in previous.items():
                    if value is None:
                        os.environ.pop(name, None)
                    else:
                        os.environ[name] = value
        else:
            copy_tree(raw, payload)
        if source_stamp(repo) != source:
            raise RuntimeError('Source changed during packaging; candidate rejected')
        candidate = manifest(payload, record, 'dev', kind)
        atomic_json(stage / 'deployment.json', candidate)
        final = store / candidate['id']
        if final.exists():
            verify_manifest(final / payload.name, json.loads((final / 'deployment.json').read_text()))
            shutil.rmtree(stage)
        else:
            stage.rename(final)
        return final
    except BaseException:
        # Preserve the diagnostic log, not a partially built executable set.
        for name in ('bin', 'payload', 'Zork.app', 'cua-runtime'):
            if (stage / name).exists():
                shutil.rmtree(stage / name)
        raise


def promote_app(repo, source, target, *, channel='release', prefix=None):
    """Change only bundle identity/signing; never rebuild code after its soak period."""
    packager = load_packager(repo)
    prefix = prefix or ('ing.zork-dev' if channel == 'dev' else 'ing.zork')
    packager.verify_app(source)
    copy_tree(source, target)
    old_prefix = plistlib.loads((source / 'Contents/Info.plist').read_bytes())['CFBundleIdentifier'].removesuffix('.desktop')
    for path in target.rglob('Info.plist'):
        info = plistlib.loads(path.read_bytes())
        for key, value in list(info.items()):
            if isinstance(value, str) and value.startswith(old_prefix):
                info[key] = prefix + value[len(old_prefix):]
        if path == target / 'Contents/Info.plist':
            info['CFBundleName'] = info['CFBundleDisplayName'] = 'Zork Dev' if channel == 'dev' else 'Zork'
        path.write_bytes(plistlib.dumps(info))
    (target / 'Contents/Resources/channel').write_text(channel + '\n')
    bundles = [p for p in target.rglob('*') if p.suffix in ('.app', '.framework') and not p.is_symlink()]
    for bundle in sorted(bundles, key=lambda p: len(p.parts), reverse=True):
        subprocess.run(['codesign', '--force', '--sign', '-', str(bundle)], check=True, capture_output=True)
    subprocess.run(['codesign', '--force', '--sign', '-', str(target)], check=True, capture_output=True)
    packager.verify_app(target)


def stability(events, candidate_id, now=None):
    """A week of successful daily observations, reset by any failure or version change."""
    now = time.time() if now is None else now
    streak = []
    for event in events:
        if event.get('candidate') != candidate_id or event.get('passed') is not True:
            streak = []
            continue
        stamp = event.get('at')
        if type(stamp) not in (int, float) or not math.isfinite(stamp) or stamp > now:
            raise RuntimeError('Invalid observation timestamp; cannot establish seven days')
        if streak and stamp <= streak[-1]:
            raise RuntimeError('Non-monotonic observations; cannot establish seven days')
        if streak and stamp - streak[-1] > 36 * 3600:
            streak = []
        streak.append(stamp)
    if not streak or now - streak[-1] > 24 * 3600 or streak[-1] - streak[0] < 7 * 24 * 3600:
        raise RuntimeError('Promotion requires the same accepted dev build for seven days, daily chat checks (gap <=36h), and a check within 24h')
    return {'candidate': candidate_id, 'first': streak[0], 'last': streak[-1], 'observations': len(streak)}
