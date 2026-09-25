#!/usr/bin/env python3
"""Opportunistic build-storage pruning, triggered by builds.

After a successful build, ``after_build()`` starts ``prune-build-storage.py
--apply`` in the background at most once per ``ZORK_PRUNE_INTERVAL_HOURS``
(default 24). It is detached, low priority, and never changes the build's
result. ``ZORK_NO_PRUNE=1`` disables it. Run this file directly to trigger
the same throttled check from a wrapper such as ``zork-cargo``.
"""
import fcntl
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/lib'))
DEFAULT_ROOT = Path.home() / 'Library/Caches/zork-build'
LOG = Path.home() / 'Library/Logs/zork-prune-build-storage.log'


def build_root(env=None):
    try:
        from build_env import build_environment
        configured = build_environment(ROOT).get('ZORK_BUILD_ROOT')
    except Exception:
        configured = None
    configured = (env or os.environ).get('ZORK_BUILD_ROOT') or configured
    path = Path(configured).expanduser() if configured else DEFAULT_ROOT
    return path if path.is_absolute() else ROOT / path


def interval_seconds(env=None):
    try:
        hours = float((env or os.environ).get('ZORK_PRUNE_INTERVAL_HOURS') or 24)
    except ValueError:
        hours = 24
    return max(hours, 0) * 3600


def due(stamp, now, interval):
    try:
        return now - stamp.stat().st_mtime >= interval
    except FileNotFoundError:
        return True


def after_build(env=None, now=None, spawn=None):
    """Start a background prune if one is due. Returns True when started."""
    env = env or os.environ
    if env.get('ZORK_NO_PRUNE'):
        return False
    try:
        root = build_root(env)
        if not root.is_dir():
            return False
        stamp = root / '.last-prune'
        now = time.time() if now is None else now
        with open(root / '.prune.lock', 'a') as lock:
            try:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return False
            if not due(stamp, now, interval_seconds(env)):
                return False
            stamp.touch()
            os.utime(stamp, (now, now))
            (spawn or _spawn)()
            return True
    except Exception as error:  # never fail a build over housekeeping
        print(f'build storage prune not started: {error}', file=sys.stderr)
        return False


def _spawn():
    LOG.parent.mkdir(parents=True, exist_ok=True)
    log = open(LOG, 'a')
    python = '/opt/homebrew/bin/python3' if Path('/opt/homebrew/bin/python3').exists() else sys.executable
    env = dict(os.environ, ZORK_NO_PRUNE='1')
    env['PATH'] = '/opt/homebrew/bin:' + str(Path.home() / '.local/bin') + ':' + env.get('PATH', '/usr/bin:/bin')
    subprocess.Popen(['nice', '-n', '10', python, str(ROOT / 'scripts/prune-build-storage.py'), '--apply'],
                     cwd=ROOT, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log,
                     start_new_session=True)


if __name__ == '__main__':
    print('started' if after_build() else 'not due')
