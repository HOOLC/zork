#!/usr/bin/env python3
"""Prune Zork build storage on this machine (dry-run unless --apply).

- ``ZORK_BUILD_ROOT/isolated/<name>`` Cargo targets whose worktree is gone
  (owner marker, else the worktree directory name build_env.py derives the
  target from) or that nothing wrote for ``--days`` (default 14);
  ``ZORK_BUILD_ROOT/target`` only by age. ``isolated/deployment`` (the fixed
  deployment capture cache) and targets marked ``.zork-cache-keep`` are kept.
  Every target must carry Cargo's CACHEDIR.TAG and have no open files.
- Gitignored ``artifacts/`` entries older than ``--artifact-days`` (default 7)
  in this repository's worktrees; tracked files are never touched.
- ``git worktree prune`` for this repository.
- ``kache gc`` until the local kache store is back under its configured limit.

Only Zork paths are touched: this repository's worktrees, its build root and
the kache store configured for it. Builds start ``--apply`` automatically
at most once a day (``scripts/lib/build_maintenance.py``); there is no schedule.
"""
import argparse
from datetime import datetime, timezone
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/lib'))
from build_env import build_environment, clean_git_environment

FIXED_TARGETS = {'deployment'}
RECENT_GUARD = 3600


def load_budget():
    spec = importlib.util.spec_from_file_location('cache_budget', ROOT / 'scripts/build/cache_budget.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def human(size):
    for unit in ('B', 'K', 'M', 'G', 'T'):
        if abs(size) < 1024 or unit == 'T':
            return f'{size:.1f}{unit}' if unit != 'B' else f'{int(size)}B'
        size /= 1024.0


def git(*args, cwd=ROOT, check=True):
    return subprocess.run(['git', *args], cwd=cwd, env=clean_git_environment(os.environ),
                          capture_output=True, text=True, check=check)


def worktrees():
    """(path, prunable) for every worktree of this repository."""
    result, current = [], {}
    for line in git('worktree', 'list', '--porcelain').stdout.splitlines() + ['']:
        if not line:
            if current:
                result.append((Path(current['worktree']), 'prunable' in current))
            current = {}
            continue
        key, _, value = line.partition(' ')
        current[key] = value
    return result


def build_root():
    # Worktrees usually have no .env; the main checkout's machine settings apply.
    for checkout in dict.fromkeys([ROOT, main_checkout()]):
        configured = build_environment(checkout).get('ZORK_BUILD_ROOT')
        if configured:
            path = Path(configured).expanduser()
            return (path if path.is_absolute() else checkout / path).resolve()
    return None


def target_owner(target, name, live):
    marker = target / '.zork-cache-owner.json'
    if marker.is_file() and not marker.is_symlink():
        try:
            return Path(json.loads(marker.read_text())['worktree']), 'owner marker'
        except (OSError, ValueError, KeyError, TypeError):
            pass
    matches = [path for path in live if path.name == name]
    if matches:
        return matches[0], 'worktree name'
    return None, 'no worktree named ' + name


def plan_targets(root, days, now, budget, live):
    plans = []
    if root is None or not root.is_dir():
        return plans
    isolated = root / 'isolated'
    entries = []
    if isolated.is_dir() and not isolated.is_symlink():
        for entry in sorted(isolated.iterdir()):
            if entry.is_symlink() or not entry.is_dir():
                continue
            target = entry / 'target' if (entry / 'target').is_dir() and not (entry / '.rustc_info.json').exists() else entry
            entries.append((entry.name, entry, target))
    shared = root / 'target'
    if shared.is_dir() and not shared.is_symlink():
        entries.append(('target', shared, shared))
    for name, entry, target in entries:
        item = {'path': str(entry), 'name': name}
        if entry.parent == isolated and name in FIXED_TARGETS:
            item.update(action='keep', reason='fixed deployment capture cache')
        elif (target / '.zork-cache-keep').exists():
            item.update(action='keep', reason='.zork-cache-keep marker')
        elif not budget.cargo_tag(target):
            item.update(action='keep', reason='no Cargo CACHEDIR.TAG; inspect manually')
        else:
            latest = budget.latest_write(target)
            item['last_write'] = datetime.fromtimestamp(latest, timezone.utc).isoformat()
            owner, source = (None, 'shared target') if entry == shared else target_owner(target, name, live)
            orphan = entry != shared and (owner is None or not owner.exists()
                                          or owner.resolve() not in {p.resolve() for p in live if p.exists()})
            stale = now - latest > days * 86400
            if now - latest < RECENT_GUARD:
                item.update(action='keep', reason='written within the last hour')
            elif orphan:
                item.update(action='delete', reason=f'worktree gone ({source}: {owner or name})')
            elif stale:
                item.update(action='delete', reason=f'untouched for {int((now - latest) // 86400)} days')
            else:
                item.update(action='keep', reason=f'active worktree {owner}' if owner else 'recently used')
        plans.append(item)
    return plans


def newest_write(path):
    latest = path.lstat().st_mtime
    if path.is_dir() and not path.is_symlink():
        for folder, dirs, files in os.walk(path):
            for name in dirs + files:
                try:
                    latest = max(latest, os.lstat(os.path.join(folder, name)).st_mtime)
                except OSError:
                    pass
    return latest


def tree_size(path):
    total = 0
    try:
        if path.is_symlink() or not path.is_dir():
            return path.lstat().st_blocks * 512
    except OSError:
        return 0
    for folder, dirs, files in os.walk(path):
        for name in dirs + files:
            try:
                total += os.lstat(os.path.join(folder, name)).st_blocks * 512
            except OSError:
                pass
    return total


def plan_artifacts(live, days, now):
    plans = []
    for worktree, _ in live:
        folder = worktree / 'artifacts'
        if not folder.is_dir() or folder.is_symlink():
            continue
        if git('check-ignore', '-q', '--no-index', 'artifacts/', cwd=worktree, check=False).returncode != 0:
            plans.append({'path': str(folder), 'action': 'keep', 'reason': 'artifacts/ is not gitignored here'})
            continue
        for entry in sorted(folder.iterdir()):
            relative = str(entry.relative_to(worktree))
            item = {'path': str(entry)}
            if git('ls-files', '--', relative, cwd=worktree).stdout.strip():
                item.update(action='keep', reason='contains tracked files')
            else:
                age = now - newest_write(entry)
                if age > days * 86400:
                    item.update(action='delete', reason=f'untouched for {int(age // 86400)} days')
                else:
                    item.update(action='keep', reason='recent')
            plans.append(item)
    return plans


def kache_binary():
    for candidate in (shutil.which('kache'), str(Path.home() / '.local/bin/kache')):
        if candidate and Path(candidate).is_file():
            return candidate
    return None


def kache_usage(binary):
    """(used, limit) in bytes from `kache stats`, or None.

    `Store:` reports the logical size before deduplication; kache enforces its
    limit on the physical size (`Dedup: ... N GiB physical`), so prefer that.
    Otherwise a deduplicated store looks over its limit forever and every run
    would evict recent entries by age.
    """
    output = subprocess.run([binary, 'stats'], capture_output=True, text=True).stdout
    match = re.search(r'Store:\s+([\d.]+)\s*(\w+)\s*/\s*([\d.]+)\s*(\w+)', output)
    if not match:
        return None
    units = {'B': 1, 'KiB': 2**10, 'MiB': 2**20, 'GiB': 2**30, 'TiB': 2**40,
             'KB': 1e3, 'MB': 1e6, 'GB': 1e9, 'TB': 1e12}
    try:
        used = float(match.group(1)) * units[match.group(2)]
        limit = float(match.group(3)) * units[match.group(4)]
        physical = re.search(r'Dedup:.*?([\d.]+)\s*(\w+)\s+physical', output)
        if physical:
            used = float(physical.group(1)) * units[physical.group(2)]
        return (used, limit)
    except KeyError:
        return None


def kache_gc(binary, apply):
    before = kache_usage(binary)
    result = {'binary': binary, 'before': before}
    if not apply or before is None:
        result['action'] = 'gc' if before and before[0] > before[1] else 'none'
        return result
    runs = []
    for arguments in ([], ['--max-age', '14d'], ['--max-age', '7d'], ['--max-age', '3d']):
        usage = kache_usage(binary)
        if runs and usage and usage[0] <= usage[1]:
            break
        completed = subprocess.run([binary, 'gc', *arguments], capture_output=True, text=True)
        runs.append({'argv': ['gc', *arguments], 'exit': completed.returncode})
        if completed.returncode != 0:
            break
    result.update(runs=runs, after=kache_usage(binary))
    return result


def remove_target(root, item, budget):
    path = Path(item['path'])
    target = path / 'target' if (path / 'target').is_dir() and not (path / '.rustc_info.json').exists() else path
    if path.is_symlink() or not path.resolve().is_relative_to(root):
        raise RuntimeError('target changed; refusing')
    if not budget.cargo_tag(target) or (target / '.zork-cache-keep').exists() \
            or time.time() - budget.latest_write(target) < RECENT_GUARD:
        raise RuntimeError('target became active or unmarked; skipped')
    if not budget.idle(target):
        raise RuntimeError('target has open files; skipped')
    shutil.rmtree(path)


def run(args):
    now = time.time()
    live_entries = worktrees()
    live = [path for path, prunable in live_entries if not prunable and path.exists()]
    root = build_root()
    budget = load_budget()
    free_before = shutil.disk_usage(Path.home()).free
    report = {'observed_at': datetime.now(timezone.utc).isoformat(), 'apply': args.apply,
              'build_root': str(root) if root else None}
    lock = None
    if root and root.is_dir():
        lock = (root / '.zork-cache-gc.lock').open('a+')
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise SystemExit('Another build cache cleanup is running')
    try:
        targets = plan_targets(root, args.days, now, budget, live)
        artifacts = plan_artifacts([(p, False) for p in live], args.artifact_days, now)
        for item in targets + artifacts:
            if item['action'] == 'delete':
                item['bytes'] = tree_size(Path(item['path']))
        report['targets'], report['artifacts'] = targets, artifacts
        prune = git('worktree', 'prune', '--dry-run', '--verbose', check=False)
        report['worktree_prune'] = [line for line in (prune.stdout + prune.stderr).splitlines() if line.strip()]
        binary = kache_binary()
        errors, cleaned = [], []
        if args.apply:
            for item in targets:
                if item['action'] != 'delete':
                    continue
                try:
                    remove_target(root, item, budget)
                    cleaned.append(item['path'])
                    item['removed'] = True
                except Exception as error:
                    item['error'] = str(error)
                    errors.append(f"{item['path']}: {error}")
            for item in artifacts:
                if item['action'] != 'delete':
                    continue
                try:
                    path = Path(item['path'])
                    shutil.rmtree(path) if path.is_dir() and not path.is_symlink() else path.unlink()
                    item['removed'] = True
                except OSError as error:
                    item['error'] = str(error)
                    errors.append(f"{item['path']}: {error}")
            git('worktree', 'prune', '--verbose', check=False)
        report['kache'] = kache_gc(binary, args.apply) if binary else None
        report['errors'] = errors
        planned = sum(i.get('bytes', 0) for i in targets + artifacts if i['action'] == 'delete')
        removed = sum(i.get('bytes', 0) for i in targets + artifacts if i.get('removed'))
        report['planned_bytes'], report['removed_bytes'] = planned, removed
        report['free_before_bytes'] = free_before
        report['free_after_bytes'] = shutil.disk_usage(Path.home()).free
        if args.apply and root and cleaned:
            try:
                budget.record_cleanup(root, {'started_at': report['observed_at'],
                    'finished_at': datetime.now(timezone.utc).isoformat(), 'mode': 'prune-build-storage',
                    'cleaned': cleaned, 'failed': bool(errors),
                    'free_before_bytes': free_before, 'free_after_bytes': report['free_after_bytes']})
            except OSError:
                pass
    finally:
        if lock:
            lock.close()
    return report


def render(report):
    lines = [f"build root: {report['build_root']}  ({'APPLY' if report['apply'] else 'dry-run'})"]
    for title, key in (('Cargo targets', 'targets'), ('artifacts', 'artifacts')):
        lines.append(title + ':')
        for item in report[key]:
            size = human(item['bytes']) if 'bytes' in item else ''
            state = ' removed' if item.get('removed') else (' ERROR ' + item['error'] if item.get('error') else '')
            lines.append(f"  {item['action']:<6} {size:>8}  {item['path']}  [{item['reason']}]{state}")
    lines.append('git worktree prune: ' + ('; '.join(report['worktree_prune']) or 'nothing to prune'))
    kache = report.get('kache')
    if kache and kache.get('before'):
        used, limit = kache['before']
        lines.append(f"kache: {human(used)} / {human(limit)} ({used / limit:.0%})"
                     + (f" -> {human(kache['after'][0])}" if kache.get('after') else
                        (' -> would run kache gc' if kache.get('action') == 'gc' else ' (under limit)')))
    lines.append(f"{'removed' if report['apply'] else 'would remove'}: "
                 f"{human(report['removed_bytes'] if report['apply'] else report['planned_bytes'])}; "
                 f"free {human(report['free_before_bytes'])} -> {human(report['free_after_bytes'])}")
    lines += ['error: ' + e for e in report['errors']]
    return '\n'.join(lines)


def main_checkout():
    entries = worktrees()
    return entries[0][0] if entries else ROOT


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('--apply', action='store_true', help='Delete what the dry-run lists')
    parser.add_argument('--days', type=float, default=14, help='Remove Cargo targets untouched this long')
    parser.add_argument('--artifact-days', type=float, default=7, help='Remove artifacts/ entries untouched this long')
    parser.add_argument('--json', action='store_true')
    args = parser.parse_args(argv)
    report = run(args)
    print(json.dumps(report, indent=2) if args.json else render(report))
    if report['errors']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
