"""Resolve one verified native cua runtime for normal macOS app packaging."""
import fcntl
import importlib.util
import json
from pathlib import Path
import platform
import subprocess
import sys

from build_env import build_environment


def specification(repo):
    spec = importlib.util.spec_from_file_location('cua_runtime', Path(repo) / 'scripts/lib/cua-runtime.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def verify_runtime(repo, runtime):
    spec = specification(repo)
    runtime = Path(runtime).resolve()
    record = json.loads((runtime / 'cua-driver.json').read_text())
    if (record.get('version'), record.get('revision'), record.get('repository')) != (spec.VERSION, spec.REVISION, spec.REPOSITORY):
        raise RuntimeError('Native cua runtime does not match the pinned source')
    for name in ('cua-driver', 'LICENSE.cua'):
        if spec.digest(runtime / name) != record.get('sha256', {}).get(name):
            raise RuntimeError('Native cua runtime input changed: ' + name)
    return runtime, record


def ensure_runtime(repo, environ=None):
    repo = Path(repo).resolve()
    env = build_environment(repo, environ)
    base = Path(env.get('ZORK_BUILD_ROOT', repo / 'target')).expanduser()
    if not base.is_absolute():
        base = repo / base
    cache = base / 'cua'
    cache.mkdir(parents=True, exist_ok=True, mode=0o700)
    spec = specification(repo)
    runtime = cache / 'runtime' / spec.REVISION / platform.machine()
    with (cache / 'build.lock').open('a+') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        if runtime.exists():
            # An invalid existing input is an error, never an executable fallback.
            return verify_runtime(repo, runtime)[0]
        source = Path(env.get('ZORK_CUA_SOURCE', cache / 'source'))
        if 'ZORK_CUA_SOURCE' not in env:
            source.mkdir(parents=True, exist_ok=True)
            if not (source / '.git').exists():
                if any(source.iterdir()):
                    raise RuntimeError('Native cua source cache is not an owned Git checkout')
                subprocess.run(['git', 'init', '-q', str(source)], env=env, check=True)
                subprocess.run(['git', 'remote', 'add', 'origin', spec.REPOSITORY], cwd=source, env=env, check=True)
            remote = subprocess.check_output(['git', 'remote', 'get-url', 'origin'], cwd=source, env=env, text=True).strip()
            if remote != spec.REPOSITORY:
                raise RuntimeError('Native cua source cache has an unexpected origin')
            dirty = subprocess.check_output(['git', 'status', '--porcelain'], cwd=source, env=env, text=True)
            if dirty:
                raise RuntimeError('Native cua source cache has local work; left unchanged')
            subprocess.run(['git', 'fetch', '--depth', '1', 'origin', spec.REVISION], cwd=source, env=env, check=True)
            subprocess.run(['git', 'checkout', '--detach', spec.REVISION], cwd=source, env=env, check=True)
        runtime.parent.mkdir(parents=True, exist_ok=True)
        pending = runtime.with_name(runtime.name + '.pending')
        target = Path(env.get('ZORK_CUA_TARGET_DIR', base / 'cua-driver-target'))
        # Keep a failed input in this same staging slot for diagnosis/retry.
        subprocess.run([sys.executable, str(repo / 'scripts/build-cua-driver.py'),
                        '--source', str(source), '--target-dir', str(target), '--output', str(pending)],
                       cwd=repo, env=env, check=True)
        verify_runtime(repo, pending)
        pending.rename(runtime)
        return runtime
