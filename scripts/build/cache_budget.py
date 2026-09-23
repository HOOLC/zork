#!/usr/bin/env python3
"""Preview or clean inactive Cargo targets under ZORK_BUILD_ROOT."""
import argparse
from datetime import datetime, timezone
import fcntl
import json
import math
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/lib'))
from build_env import build_environment, CACHEDIR_SIGNATURE


def budget(env):
    high = float(env.get('ZORK_BUILD_BUDGET_GIB') or 100)
    low = float(env.get('ZORK_BUILD_LOW_WATER_GIB') or 80)
    if not (math.isfinite(high) and math.isfinite(low) and 0 < low < high):
        raise ValueError('Require 0 < low-water < budget, both finite')
    return high * 2**30, low * 2**30


def size(path):
    return int(subprocess.check_output(['du', '-sk', str(path)], text=True).split()[0]) * 1024


def candidates(root):
    # Only conventional target roots, never arbitrary source/archive directories.
    choices = [root / 'target', root / 'android']
    isolated = root / 'isolated'
    if isolated.is_dir() and not isolated.is_symlink():
        for path in isolated.iterdir():
            if not path.is_dir() or path.is_symlink():
                continue
            if (path / '.rustc_info.json').is_file():
                choices.append(path)
            elif (path / 'target').is_dir() and not (path / 'target').is_symlink():
                choices.append(path / 'target')
    return [p for p in choices if not p.is_symlink() and p.is_dir()
            and (p / '.rustc_info.json').is_file()
            and not (p / '.zork-cache-keep').exists()]


def cargo_tag(path):
    tag = path / 'CACHEDIR.TAG'
    if tag.is_symlink() or not tag.is_file():
        return False
    with tag.open() as stream:
        return stream.readline().strip() == CACHEDIR_SIGNATURE


def released(path):
    marker = path / '.zork-cache-owner.json'
    if marker.is_symlink() or not marker.is_file():
        return False
    try:
        owner = Path(json.loads(marker.read_text())['worktree'])
    except (OSError, ValueError, KeyError, TypeError):
        return False
    return owner.is_absolute() and not owner.exists()


def latest_write(path):
    latest = path.stat().st_mtime
    for directory, _, files in os.walk(path):
        for name in files:
            latest = max(latest, (Path(directory) / name).lstat().st_mtime)
    return latest


def idle(path):
    # Fail closed if process inspection is unavailable or inconclusive.
    if not shutil.which('lsof'):
        raise RuntimeError('lsof is required for cleanup')
    result = subprocess.run(['lsof', '-nP', '+D', str(path)], capture_output=True, text=True)
    if result.returncode not in (0, 1) or result.stderr.strip():
        raise RuntimeError('Cannot reliably inspect open files; cleanup refused')
    return result.returncode == 1 and not result.stdout.strip()


def plan(root, total, high, low, min_age_hours, automatic):
    if total <= high:
        return []
    chosen = []
    remaining = total
    isolated = root / 'isolated'
    aged = sorted((latest_write(p), p) for p in candidates(root)
                 if cargo_tag(p) and (not automatic or
                    (p.is_relative_to(isolated) and released(p))))
    for modified, path in aged:
        if remaining <= low:
            break
        if time.time() - modified < min_age_hours * 3600:
            continue
        amount = size(path)
        chosen.append((path, amount))
        remaining -= amount
    return chosen


def released_targets(root):
    isolated = root / 'isolated'
    return [(path, size(path)) for path in candidates(root)
            if path.is_relative_to(isolated) and cargo_tag(path) and released(path)]


def record_cleanup(root, result):
    line = (json.dumps(result, sort_keys=True) + '\n').encode()
    fd = os.open(root / '.zork-cache-cleanups.jsonl', os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX)
        os.write(fd, line)
    finally:
        os.close(fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--apply', action='store_true')
    parser.add_argument('--auto', action='store_true',
        help='Clean released, old, inactive isolated targets; suitable for unattended runs')
    parser.add_argument('--reclaim-released', action='store_true',
        help='Clean released isolated targets immediately, independent of budget and age')
    parser.add_argument('--dry-run', action='store_true', help='Preview an automatic cleanup without cleaning')
    parser.add_argument('--min-age-hours', type=float, default=24)
    args = parser.parse_args()
    if sum((args.apply, args.auto, args.reclaim_released)) > 1:
        parser.error('choose one cleanup mode')
    if args.dry_run and not (args.auto or args.reclaim_released):
        parser.error('--dry-run requires --auto or --reclaim-released')
    if not math.isfinite(args.min_age_hours) or args.min_age_hours < 0:
        parser.error('min age must be finite and nonnegative')
    env = build_environment()
    if not env.get('ZORK_BUILD_ROOT'):
        parser.error('Set ZORK_BUILD_ROOT to a dedicated cache root first')
    root = Path(env['ZORK_BUILD_ROOT']).expanduser()
    root = (root if root.is_absolute() else ROOT / root).resolve()
    if root in (Path('/'), Path.home(), ROOT.resolve()) or ROOT.resolve().is_relative_to(root):
        parser.error('Build root must be a dedicated cache directory')
    high, low = budget(env)
    if not root.is_dir():
        print(json.dumps({'root': str(root), 'bytes': 0, 'candidates': []})); return
    with (root / '.zork-cache-gc.lock').open('a+') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | (fcntl.LOCK_NB if args.auto or args.reclaim_released else 0))
        except BlockingIOError:
            print(json.dumps({'root': str(root), 'status': 'another cleanup is running'}))
            return
        if args.reclaim_released:
            chosen = released_targets(root)
            if not chosen:
                print(json.dumps({'root': str(root), 'observed_at': datetime.now(timezone.utc).isoformat(),
                                  'mode': 'reclaim-preview' if args.dry_run else 'reclaim',
                                  'free_bytes': shutil.disk_usage(root).free, 'candidates': []}, indent=2))
                return
        total = size(root)
        free_before = shutil.disk_usage(root).free
        if not args.reclaim_released:
            chosen = plan(root, total, high, low, args.min_age_hours, args.auto)
        untagged = [str(path) for path in candidates(root)
                    if (not (args.auto or args.reclaim_released) or path.is_relative_to(root / 'isolated'))
                    and not cargo_tag(path)]
        mode = ('reclaim-preview' if args.dry_run else 'reclaim') if args.reclaim_released else \
            'auto-preview' if args.dry_run else 'auto' if args.auto else 'apply' if args.apply else 'preview'
        observed_at = datetime.now(timezone.utc).isoformat()
        print(json.dumps({'root': str(root), 'observed_at': observed_at,
                          'bytes': total, 'free_bytes': free_before, 'budget_bytes': high,
                          'low_water_bytes': low, 'mode': mode,
                          'untagged_targets': untagged,
                          'candidates': [{'path': str(p), 'bytes': n} for p, n in chosen]}, indent=2), flush=True)
        if args.dry_run or not (args.apply or args.auto or args.reclaim_released):
            return
        failed = False
        cleaned = []
        for path, _ in chosen:
            if not args.reclaim_released and size(root) <= low:
                break
            if path.is_symlink() or not path.resolve().is_relative_to(root):
                raise RuntimeError('Candidate changed; refusing cleanup')
            if not cargo_tag(path) or ((args.auto or args.reclaim_released) and not released(path)) or \
                    (path / '.zork-cache-keep').exists() or \
                    (not args.reclaim_released and time.time() - latest_write(path) < args.min_age_hours * 3600) or \
                    not idle(path):
                print('Skipped active/recent target:', path); continue
            try:
                subprocess.run(['cargo', 'clean', '--target-dir', str(path)], cwd=ROOT, env=env, check=True)
                cleaned.append(str(path))
            except subprocess.CalledProcessError as error:
                failed = True
                print(f'Skipped target rejected by Cargo: {path}: {error}', file=sys.stderr)
        remaining = size(root)
        print('Remaining bytes:', remaining)
        free_after = shutil.disk_usage(root).free
        result = {'started_at': observed_at, 'finished_at': datetime.now(timezone.utc).isoformat(),
                  'mode': mode, 'bytes_before': total, 'bytes_after': remaining,
                  'free_before_bytes': free_before, 'free_after_bytes': free_after,
                  'cleaned': cleaned, 'failed': failed}
        try:
            record_cleanup(root, result)
        except OSError as error:
            print(f'Cache cleanup record unavailable: {error}', file=sys.stderr)
        print(json.dumps(result))
        if failed:
            raise SystemExit(1)


if __name__ == '__main__':
    main()
