#!/usr/bin/env python3
"""Retention for a deployment store: candidates, transactions, tools and backups.

Rules (docs/guides/release-dev-recovery.md, "存储保留"):

- Anything named by an active, previous or pending receipt of any channel, kind
  or device is kept, as is every candidate an unfinished transaction can restore.
- Per channel/kind/device the active candidate and the previous one (rollback)
  are kept; candidates newer than the deployed build (not yet deployed) are kept.
- Per channel/kind the latest accepted transaction (its snapshots undo the last
  switch) is kept; unfinished, failed or unreadable transactions are always kept;
  rolled-back transactions are kept for an investigation window.
- Superseded ``tools/recovery-*`` / ``tools/installer-*`` versions, stale
  ``.build-*`` staging directories, upload staging and ``backups/dev`` beyond
  the newest three are removed.
- A corrupt receipt, a pending (interrupted) transaction, an unreadable
  configuration or a store without any active receipt keeps everything.

Store copies of macOS apps are unregistered from LaunchServices before they are
deleted, and every copy that stays in the store is unregistered after each run.
New stores keep candidates and transactions below ``*.noindex`` directories so
Spotlight never registers the copies again; ``candidates``/``transactions``
remain as symlinks, so every recorded path keeps working.

Standard library only and Python 3.9 compatible: destination Macs run it with
the system python, and it can be streamed over SSH (``python3 - --root ...``).
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

CHANNELS = ('release', 'dev', 'test')
STORE_DIRS = ('candidates', 'transactions')
STAGING_PREFIXES = ('.build-', '.promote-', '.accept-')
TOOL_PREFIXES = ('recovery-', 'installer-')
KEEP_BACKUPS = 3
STAGING_GRACE = 3600
INSTALL_STAGING_GRACE = 6 * 3600
ROLLED_BACK_WINDOW = 7 * 24 * 3600
LSREGISTER = ('/System/Library/Frameworks/CoreServices.framework/Frameworks/'
              'LaunchServices.framework/Support/lsregister')


class Blocked(Exception):
    """The store cannot be classified safely; nothing is deleted."""


def read_json(path):
    return json.loads(Path(path).read_text())


def is_under(path, parent):
    path, parent = Path(path), Path(parent)
    return path == parent or parent in path.parents


def tree_size(path):
    total = 0
    path = Path(path)
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


def human(size):
    for unit in ('B', 'K', 'M', 'G', 'T'):
        if size < 1024 or unit == 'T':
            return f'{size:.1f}{unit}' if unit != 'B' else f'{size}B'
        size /= 1024.0


class LaunchServices:
    """lsregister wrapper; the runner is injectable so tests need no macOS."""

    def __init__(self, runner=None, lsregister=LSREGISTER, platform=sys.platform):
        self.run = runner or (lambda argv: subprocess.run(argv, capture_output=True, text=True))
        self.lsregister = lsregister
        self.available = platform == 'darwin' and (runner is not None or Path(lsregister).exists())

    @staticmethod
    def bundles(path):
        """Every bundle directory (Contents/Info.plist) at or below path, innermost first."""
        path = Path(path)
        found = []
        if path.is_symlink() or not path.is_dir():
            return found
        for folder, dirs, _ in os.walk(path):
            dirs[:] = [d for d in dirs if not os.path.islink(os.path.join(folder, d))]
            if os.path.basename(folder) == 'Contents' and os.path.isfile(os.path.join(folder, 'Info.plist')):
                found.append(Path(folder).parent)
        return sorted(set(found), key=lambda p: len(p.parts), reverse=True)

    def registered(self):
        if not self.available:
            return []
        result = self.run([self.lsregister, '-dump'])
        paths = []
        for line in (result.stdout or '').splitlines():
            match = re.match(r'^path:\s+(/.*?)(?:\s+\(0x[0-9a-fA-F]+\))?\s*$', line)
            if match:
                paths.append(match.group(1))
        return paths

    def unregister(self, paths):
        paths = [str(p) for p in paths]
        if not self.available or not paths:
            return []
        for start in range(0, len(paths), 64):
            self.run([self.lsregister, '-u', *paths[start:start + 64]])
        return paths


class Plan:
    def __init__(self, root):
        self.root = Path(root)
        self.keep = []      # (path, reason)
        self.delete = []    # (path, reason)
        self.blocked = None
        self.warnings = []
        self.migrations = []

    def kept(self, path, reason):
        self.keep.append((Path(path), reason))

    def remove(self, path, reason):
        self.delete.append((Path(path), reason))

    def report(self, sizes=True):
        def entry(path, reason):
            value = {'path': str(path), 'reason': reason}
            if sizes:
                value['bytes'] = tree_size(path)
            return value
        deleted = [entry(p, r) for p, r in self.delete]
        return {'root': str(self.root), 'blocked': self.blocked, 'warnings': self.warnings,
                'migrations': self.migrations,
                'delete': deleted, 'keep': [entry(p, r) for p, r in self.keep],
                'reclaim_bytes': sum(item.get('bytes', 0) for item in deleted)}


def collect_receipts(root):
    """All active/previous/pending receipts. Any unreadable receipt blocks retention."""
    receipts = []
    def add(path, role, device, channel, kind=None):
        try:
            value = read_json(path)
        except (OSError, ValueError) as error:
            raise Blocked(f'Unreadable receipt {path}: {error}')
        if not isinstance(value, dict):
            raise Blocked(f'Unexpected receipt {path}')
        if role == 'pending':
            if not isinstance(value.get('transaction'), str):
                raise Blocked(f'Pending receipt without transaction: {path}')
        else:
            record = value.get('record')
            if not isinstance(record, dict) or not isinstance(record.get('id'), str):
                raise Blocked(f'Receipt without a build record: {path}')
            kind = record.get('kind', kind)
            channel = record.get('channel', channel)
        receipts.append({'path': Path(path), 'role': role, 'device': device, 'channel': channel,
                         'kind': kind, 'value': value})
    for channel in CHANNELS:
        folder = root / channel
        if not folder.is_dir():
            continue
        for path in sorted(folder.glob('*-active.json')):
            add(path, 'active', 'local', channel, path.name[:-len('-active.json')])
        for path in sorted(folder.glob('*-previous.json')):
            add(path, 'previous', 'local', channel, path.name[:-len('-previous.json')])
        for path in sorted(folder.glob('*-pending.json')):
            add(path, 'pending', 'local', channel, path.name[:-len('-pending.json')])
    devices = root / 'devices'
    if devices.is_dir():
        for host in sorted(p for p in devices.iterdir() if p.is_dir()):
            for path in sorted(host.glob('*-active.json')):
                add(path, 'active', host.name, path.name[:-len('-active.json')], 'app')
            for path in sorted(host.glob('*-previous.json')):
                add(path, 'previous', host.name, path.name[:-len('-previous.json')], 'app')
            for path in sorted(host.glob('*-pending.json')):
                add(path, 'pending', host.name, path.name[:-len('-pending.json')], 'app')
    return receipts


def mtime(path):
    try:
        return Path(path).stat().st_mtime
    except OSError:
        return 0.0


def scan_candidates(store):
    items = []
    if not store.is_dir():
        return items
    for entry in sorted(store.iterdir()):
        if entry.is_symlink() or not entry.is_dir():
            continue
        item = {'path': entry, 'name': entry.name, 'mtime': mtime(entry)}
        if entry.name.startswith('.'):
            item['staging'] = entry.name.startswith(STAGING_PREFIXES)
            item['hidden'] = True
        else:
            try:
                record = read_json(entry / 'deployment.json')
                item.update(channel=record['channel'], kind=record['kind'], id=record.get('id', entry.name),
                            mtime=mtime(entry / 'deployment.json'))
            except (OSError, ValueError, KeyError, TypeError) as error:
                item['error'] = str(error)
        items.append(item)
    return items


def scan_transactions(store):
    items = []
    if not store.is_dir():
        return items
    for entry in sorted(store.iterdir()):
        if entry.is_symlink() or not entry.is_dir():
            continue
        item = {'path': entry, 'name': entry.name}
        try:
            journal = read_json(entry / 'journal.json')
            runtime = journal.get('runtime') or {}
            previous = runtime.get('previous_active') or {}
            item.update(phase=journal['phase'], created=float(journal.get('created_at') or 0),
                        updated=float(journal.get('updated_at') or journal.get('created_at') or 0),
                        channel=runtime.get('channel'), kind=runtime.get('kind'),
                        candidate=runtime.get('candidate_id'),
                        previous=(previous.get('record') or {}).get('id'))
        except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
            item['error'] = str(error)
        items.append(item)
    return items


def configured_paths(root):
    path = root / 'deployment-config.json'
    if not path.exists():
        return []
    try:
        settings = read_json(path)
        result = []
        for entries in settings['channels'].values():
            for value in entries.values():
                for key in ('data', 'payload', 'preferences'):
                    if value.get(key):
                        result.append(Path(value[key]).expanduser())
                result += [Path(p).expanduser() for p in value.get('ancillary', [])]
        return result
    except (OSError, ValueError, KeyError, TypeError, AttributeError) as error:
        raise Blocked(f'Unreadable deployment configuration: {error}')


def tool_references(root, receipts):
    """tools/<name> directories named by management wrappers or receipts."""
    names = set()
    pattern = re.compile(r'/tools/([^/\'"\s]+)')
    texts = []
    for wrapper in sorted((root / 'bin').glob('*')) if (root / 'bin').is_dir() else []:
        try:
            texts.append(wrapper.read_text(errors='replace'))
        except OSError:
            pass
    texts += [json.dumps(r['value']) for r in receipts]
    for text in texts:
        names.update(pattern.findall(text))
    return names


def basename_of(value):
    return Path(value).name if isinstance(value, str) and value else None


def plan(root, now=None):
    root = Path(root).expanduser()
    now = time.time() if now is None else now
    result = Plan(root)
    candidates_dir, transactions_dir = root / 'candidates', root / 'transactions'
    for name in STORE_DIRS:
        plain, hidden = root / name, root / (name + '.noindex')
        if plain.is_dir() and not plain.is_symlink() and not hidden.exists():
            result.migrations.append(f'{plain} -> {hidden} (symlink kept at {plain})')
    candidates = scan_candidates(candidates_dir)
    transactions = scan_transactions(transactions_dir)
    try:
        receipts = collect_receipts(root)
        configured_paths(root)
        pending = [r for r in receipts if r['role'] == 'pending']
        if pending:
            raise Blocked('Interrupted transaction requires recovery first: ' + ', '.join(str(r['path']) for r in pending))
        if not any(r['role'] == 'active' for r in receipts) and (
                any(not c.get('hidden') for c in candidates) or transactions):
            raise Blocked('No active receipt in this store; cannot tell which build is deployed')
    except Blocked as error:
        result.blocked = str(error)
        return result

    keep_candidates = {}   # id -> reason
    keep_transactions = {}  # name -> reason
    by_name = {t['name']: t for t in transactions}
    by_candidate = {c['name']: c for c in candidates if not c.get('hidden')}

    def keep_candidate(identifier, reason):
        if identifier and identifier not in keep_candidates:
            keep_candidates[identifier] = reason

    groups = {}
    for receipt in receipts:
        key = (receipt['channel'], receipt['kind'], receipt['device'])
        groups.setdefault(key, {}).setdefault(receipt['role'], []).append(receipt)
        value = receipt['value']
        label = f"{receipt['channel']}/{receipt['kind']} on {receipt['device']}"
        keep_candidate(value['record']['id'], f"{receipt['role']} {label}")
        keep_candidate(basename_of(value.get('candidate')), f"{receipt['role']} {label}")
        tx = basename_of(value.get('transaction'))
        if tx and receipt['device'] == 'local':
            keep_transactions.setdefault(tx, f"named by {receipt['role']} receipt {label}")
        elif tx and tx in by_name:
            keep_transactions.setdefault(tx, f"named by {receipt['role']} receipt {label}")

    # One previous build per channel/kind/device for rollback.
    for (channel, kind, device), roles in sorted(groups.items()):
        label = f'{channel}/{kind} on {device}'
        for active in roles.get('active', []):
            active_id = active['value']['record']['id']
            previous = None
            for receipt in roles.get('previous', []):
                if receipt['value']['record']['id'] != active_id:
                    previous = receipt['value']['record']['id']
            tx = by_name.get(basename_of(active['value'].get('transaction')) or '')
            if not previous and tx and tx.get('candidate') == active_id and tx.get('previous') != active_id:
                previous = tx.get('previous')
            if not previous:
                # No recorded predecessor: the newest older build of this channel/kind.
                reference = by_candidate.get(active_id, {}).get('mtime') or active['value'].get('accepted_at') or now
                older = [c for c in by_candidate.values()
                         if c.get('channel') == channel and c.get('kind') == kind
                         and c['name'] != active_id and c['mtime'] < reference]
                if older:
                    previous = max(older, key=lambda c: c['mtime'])['name']
            keep_candidate(previous, f'previous {label} (rollback)')

    # Transactions.
    latest_accepted = {}
    for tx in transactions:
        if 'error' in tx:
            continue
        if tx['phase'] == 'accepted':
            key = (tx['channel'], tx['kind'])
            if key not in latest_accepted or (tx['created'], tx['name']) > (latest_accepted[key]['created'], latest_accepted[key]['name']):
                latest_accepted[key] = tx
    for tx in transactions:
        name = tx['name']
        if 'error' in tx:
            reason = 'unreadable journal: ' + tx['error']
        elif name in keep_transactions:
            reason = keep_transactions[name]
        elif tx['phase'] not in ('accepted', 'rolled_back'):
            reason = f"unfinished or unrecovered ({tx['phase']})"
        elif tx['phase'] == 'accepted' and latest_accepted.get((tx['channel'], tx['kind'])) is tx:
            reason = f"latest accepted {tx['channel']}/{tx['kind']}"
        elif tx['phase'] == 'rolled_back' and now - tx['updated'] < ROLLED_BACK_WINDOW:
            reason = 'rolled back; failed-version data kept for investigation for 7 days'
        else:
            result.remove(tx['path'], f"superseded {tx['phase']} {tx.get('channel')}/{tx.get('kind')}")
            continue
        result.kept(tx['path'], reason)
        if 'error' not in tx:
            keep_candidate(tx.get('candidate'), f'restorable by kept transaction {name}')
            keep_candidate(tx.get('previous'), f'restorable by kept transaction {name}')

    # Candidates.
    deployed_at = {}
    for receipt in receipts:
        if receipt['role'] == 'active':
            key = (receipt['channel'], receipt['kind'])
            stamp = by_candidate.get(receipt['value']['record']['id'], {}).get('mtime') or receipt['value'].get('accepted_at') or 0
            deployed_at[key] = max(deployed_at.get(key, 0), float(stamp))
    for item in candidates:
        path = item['path']
        if item.get('hidden'):
            if item.get('staging') and now - item['mtime'] > STAGING_GRACE:
                result.remove(path, 'leftover staging directory')
            else:
                result.kept(path, 'staging directory in use' if item.get('staging') else 'unrecognised hidden entry')
            continue
        if 'error' in item:
            result.kept(path, 'unreadable deployment.json: ' + item['error'])
            continue
        key = (item['channel'], item['kind'])
        if item['name'] in keep_candidates:
            result.kept(path, keep_candidates[item['name']])
        elif now - item['mtime'] < STAGING_GRACE:
            result.kept(path, 'built within the last hour')
        elif key not in deployed_at:
            result.kept(path, f'no receipt for {key[0]}/{key[1]}; never deployed here')
        elif item['mtime'] > deployed_at[key]:
            result.kept(path, f'newer than the deployed {key[0]}/{key[1]} build (not yet deployed)')
        else:
            result.remove(path, f'superseded {key[0]}/{key[1]} candidate')

    # Management tools.
    referenced = tool_references(root, receipts)
    tools = root / 'tools'
    entries = sorted((p for p in tools.iterdir() if p.is_dir() and not p.is_symlink()), key=lambda p: p.name) if tools.is_dir() else []
    newest = {}
    for prefix in TOOL_PREFIXES:
        family = [p for p in entries if p.name.startswith(prefix)]
        if family:
            newest[prefix] = max(family, key=lambda p: (mtime(p), p.name))
    kept_paths = {path for path, _ in result.keep}
    kept_ids = set(keep_candidates) | {c['name'] for c in candidates if c['path'] in kept_paths}
    for entry in entries:
        prefix = next((p for p in TOOL_PREFIXES if entry.name.startswith(p)), None)
        if prefix is None:
            result.kept(entry, 'not a versioned management tool')
        elif entry.name in referenced:
            result.kept(entry, 'named by a management wrapper or receipt')
        elif newest.get(prefix) == entry:
            result.kept(entry, 'newest ' + prefix.rstrip('-'))
        elif prefix == 'installer-' and any(i.startswith(entry.name[len(prefix):]) for i in kept_ids):
            result.kept(entry, 'installer of a kept candidate')
        else:
            result.remove(entry, 'superseded ' + prefix.rstrip('-') + ' tools')

    # Legacy pre-transaction data snapshots: only the newest few are useful.
    backups = root / 'backups' / 'dev'
    if backups.is_dir():
        snapshots = sorted(p for p in backups.iterdir() if p.is_dir() and not p.is_symlink())
        for index, entry in enumerate(snapshots):
            if index < len(snapshots) - KEEP_BACKUPS:
                result.remove(entry, f'older than the newest {KEEP_BACKUPS} dev backups')
            else:
                result.kept(entry, f'one of the newest {KEEP_BACKUPS} dev backups')

    # Upload staging left by an interrupted update-macos-client.py.
    for entry in staging_dirs(root):
        if now - mtime(entry) > INSTALL_STAGING_GRACE:
            result.remove(entry, 'leftover upload staging')
        else:
            result.kept(entry, 'recent upload staging')
    return result


def staging_dirs(root):
    found = [p for p in root.glob('.install-*') if p.is_dir() and not p.is_symlink()]
    staging = root / 'staging.noindex'
    if staging.is_dir():
        found += [p for p in staging.iterdir() if p.is_dir() and not p.is_symlink()]
    return sorted(found)


def store_locations(root):
    """Directories whose app copies must never stay registered."""
    locations = []
    for name in STORE_DIRS:
        for candidate in (root / name, root / (name + '.noindex')):
            if candidate.is_dir():
                locations.append(candidate.resolve())
    locations += [p.resolve() for p in staging_dirs(root)]
    if (root / 'staging.noindex').is_dir():
        locations.append((root / 'staging.noindex').resolve())
    return sorted(set(locations))


def launch_services_targets(root, launch_services, protected=()):
    """Store bundles on disk plus registrations of store paths or deleted copies below root."""
    root = Path(root).expanduser()
    real_root = root.resolve()
    protected = [Path(p) for p in protected] + [Path(p).resolve() for p in protected]
    locations = store_locations(root)
    lexical = [root / n for n in STORE_DIRS] + [root / (n + '.noindex') for n in STORE_DIRS] + [root / 'staging.noindex']
    targets = set()
    for location in locations:
        targets.update(launch_services.bundles(location))
    for registered in launch_services.registered():
        path = Path(registered)
        if not (is_under(path, root) or is_under(path, real_root)):
            continue
        if any(is_under(path, p) for p in protected):
            continue
        if any(is_under(path, p) for p in locations + lexical) or path.name.startswith('.install-') \
                or any(part.startswith('.install-') for part in path.parts) or not path.exists():
            targets.add(path)
    return sorted(targets, key=lambda p: (len(p.parts), str(p)), reverse=True)


def sweep_launch_services(root, launch_services=None, protected=None):
    root = Path(root).expanduser()
    launch_services = launch_services or LaunchServices()
    if not launch_services.available:
        return []
    if protected is None:
        try:
            protected = configured_paths(root)
        except Blocked:
            protected = []
    return [str(p) for p in launch_services.unregister(launch_services_targets(root, launch_services, protected))]


def ensure_noindex_layout(root):
    """Move candidates/transactions below *.noindex and leave compatible symlinks."""
    root = Path(root).expanduser()
    moved = []
    for name in STORE_DIRS:
        plain, hidden = root / name, root / (name + '.noindex')
        if plain.is_symlink():
            continue
        if plain.is_dir():
            if hidden.exists():
                continue  # Both exist: leave an unexpected layout untouched.
            os.rename(plain, hidden)
            moved.append(str(plain))
        elif plain.exists():
            continue
        hidden.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.symlink(hidden.name, plain)
    return moved


def check_deletable(root, path, configured):
    root = Path(root).expanduser()
    allowed = set()
    for name in STORE_DIRS:
        for parent in (root / name, root / (name + '.noindex')):
            if parent.exists():
                allowed.add(parent.resolve())
    for parent in (root / 'tools', root / 'backups' / 'dev', root / 'staging.noindex', root):
        if parent.exists():
            allowed.add(parent.resolve())
    target = Path(path)
    parent = target.parent.resolve()
    if parent not in allowed or target.is_symlink():
        raise RuntimeError(f'Refusing to delete outside the deployment store: {target}')
    if parent == root.resolve() and not target.name.startswith('.install-'):
        raise RuntimeError(f'Refusing to delete a store directory: {target}')
    resolved = target.resolve()
    for configured_path in configured:
        configured_path = Path(configured_path).resolve()
        if is_under(configured_path, resolved) or is_under(resolved, configured_path):
            raise RuntimeError(f'Refusing to delete configured deployment storage: {target}')


def apply(result, launch_services=None):
    """Delete what the plan selected; unregister app copies first, then sweep."""
    root = result.root
    launch_services = launch_services or LaunchServices()
    outcome = {'deleted': [], 'errors': [], 'unregistered': [], 'migrated': []}
    configured = []
    if result.blocked is None:
        configured = configured_paths(root)
        for path, reason in result.delete:
            try:
                check_deletable(root, path, configured)
                outcome['unregistered'] += launch_services.unregister(launch_services.bundles(path))
                size = tree_size(path)
                shutil.rmtree(path)
                outcome['deleted'].append({'path': str(path), 'reason': reason, 'bytes': size})
            except Exception as error:  # Keep going; report every failure.
                outcome['errors'].append(f'{path}: {error}')
    else:
        try:
            configured = configured_paths(root)
        except Blocked:
            configured = []
    if result.blocked is None:
        try:
            outcome['migrated'] = ensure_noindex_layout(root)
        except OSError as error:
            outcome['errors'].append(f'noindex layout: {error}')
    try:
        outcome['unregistered'] += sweep_launch_services(root, launch_services, configured)
    except Exception as error:
        outcome['errors'].append(f'LaunchServices sweep: {error}')
    outcome['reclaimed_bytes'] = sum(item['bytes'] for item in outcome['deleted'])
    return outcome


def prune(root, do_apply=False, launch_services=None, now=None, sizes=True):
    """Plan (and optionally apply) retention. The caller holds the store lock."""
    root = Path(root).expanduser()
    result = plan(root, now=now)
    report = result.report(sizes=sizes)
    launch_services = launch_services or LaunchServices()
    if do_apply:
        report['applied'] = apply(result, launch_services)
    else:
        try:
            protected = configured_paths(root)
        except Blocked:
            protected = []
        report['would_unregister'] = [str(p) for p in launch_services_targets(root, launch_services, protected)] \
            if launch_services.available else []
        registered = set(launch_services.registered())
        report['registered_now'] = [p for p in report['would_unregister'] if p in registered]
    return report


def after_accept(root, launch_services=None):
    """Automatic retention after a successful deployment; never fails the deployment."""
    try:
        report = prune(root, do_apply=True, launch_services=launch_services, sizes=False)
        applied = report['applied']
        print(f"retention: removed {len(applied['deleted'])} store entries "
              f"({human(applied['reclaimed_bytes'])}), unregistered {len(applied['unregistered'])} app copies"
              + (f"; kept everything: {report['blocked']}" if report['blocked'] else ''), file=sys.stderr)
        for error in applied['errors']:
            print('retention warning: ' + error, file=sys.stderr)
        return report
    except Exception as error:
        print(f'retention warning: {type(error).__name__}: {error}', file=sys.stderr)
        return None


def safe_sweep(root, launch_services=None):
    try:
        return sweep_launch_services(root, launch_services)
    except Exception as error:
        print(f'LaunchServices sweep warning: {error}', file=sys.stderr)
        return []


class StoreLock:
    def __init__(self, root):
        self.path = Path(root).expanduser() / 'deployment.lock'

    def __enter__(self):
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.handle = self.path.open('a+')
        try:
            fcntl.flock(self.handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            self.handle.close()
            raise RuntimeError(f'Another deployment owns {self.path}') from None
        return self

    def __exit__(self, *exc):
        self.handle.close()


def render(report):
    lines = [f"store {report['root']}"]
    if report['blocked']:
        lines.append('BLOCKED (keeping everything): ' + report['blocked'])
    for migration in report.get('migrations', []):
        lines.append('noindex: ' + migration)
    for item in report['delete']:
        lines.append(f"  delete {human(item.get('bytes', 0)):>8}  {item['path']}  [{item['reason']}]")
    for item in report['keep']:
        lines.append(f"  keep   {human(item.get('bytes', 0)):>8}  {item['path']}  [{item['reason']}]")
    lines.append(f"reclaimable: {human(report['reclaim_bytes'])}")
    if 'would_unregister' in report:
        lines.append(f"LaunchServices: {len(report['registered_now'])} store app copies registered now; "
                     f"would unregister {len(report['would_unregister'])} (every bundle in the store plus stale entries)")
        lines += ['  registered: ' + p for p in report['registered_now']]
    if 'applied' in report:
        applied = report['applied']
        lines.append(f"applied: removed {len(applied['deleted'])}, reclaimed {human(applied['reclaimed_bytes'])}, "
                     f"unregistered {len(applied['unregistered'])}, migrated {applied['migrated']}")
        lines += ['  error: ' + e for e in applied['errors']]
    return '\n'.join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(description='Deployment store retention (dry-run unless --apply).')
    parser.add_argument('--root', type=Path, default=Path.home() / 'Zork')
    parser.add_argument('--apply', action='store_true', help='Delete the planned entries and unregister store app copies')
    parser.add_argument('--json', action='store_true')
    args = parser.parse_args(argv)
    root = args.root.expanduser()
    if not root.is_dir():
        raise SystemExit(f'No deployment store at {root}')
    if args.apply:
        with StoreLock(root):
            report = prune(root, do_apply=True)
    else:
        report = prune(root)
    print(json.dumps(report, ensure_ascii=False, indent=2) if args.json else render(report))
    if args.apply and report['applied']['errors']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
