#!/usr/bin/env python3
"""Build, observe, promote and recover separate local release/dev deployments."""
import argparse
import json
import os
from pathlib import Path
import shutil
import socket
import sys
import time
import uuid

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS / 'lib'))
from deployment import (Transaction, atomic_json, check_links, copy_tree, digest, exclusive,
                        manifest, replace_directory, verify_manifest)
from deployment_build import build, promote_app, stability
from deployment_health import NodeRuntime


def config(root):
    return json.loads((root / 'deployment-config.json').read_text())


def channel_config(root, channel, kind):
    settings = config(root)['channels']
    chosen = settings[channel][kind]
    def host(value):
        return value.get('host', socket.gethostname()).lower().split('.')[0]
    def paths(value):
        return [Path(p).resolve() for p in (value['data'], value['payload'],
                *value.get('ancillary', []), *([value['preferences']] if value.get('preferences') else []))]
    for other in settings['release' if channel == 'dev' else 'dev'].values():
        if host(chosen) != host(other):
            continue
        for left in paths(chosen):
            for right in paths(other):
                if left.is_relative_to(right) or right.is_relative_to(left):
                    raise RuntimeError(f'release/dev storage overlaps: {left} and {right}')
    return chosen


def runtime_for(settings, root, channel, kind):
    host = settings.get('host')
    if host and host.lower().split('.')[0] != socket.gethostname().lower().split('.')[0]:
        raise RuntimeError(f'This deployment belongs to {host}; use update-macos-client.py --host on the build host')
    health = settings.get('health', {})
    if kind == 'app':
        from deployment_macos import AppRuntime
        return AppRuntime(dict(settings, channel=channel), root / channel / 'logs/app.log')
    return NodeRuntime(settings['data'], settings['payload'], root / channel / 'logs/station.log', **health)


def events_path(root, kind):
    return root / 'dev' / (kind + '-observations.json')


def record_observation(root, kind, candidate, passed, **details):
    path = events_path(root, kind)
    events = json.loads(path.read_text()) if path.exists() else []
    events.append({'at': time.time(), 'candidate': candidate, 'passed': passed, **details})
    atomic_json(path, events)


def load_candidate(directory):
    directory = Path(directory).resolve()
    record = json.loads((directory / 'deployment.json').read_text())
    payload = directory / ('Zork.app' if record['kind'] == 'app' else 'payload')
    if payload.is_symlink() or not payload.is_dir():
        raise RuntimeError('Candidate payload must be a self-contained directory')
    check_links([payload])
    verify_manifest(payload, record)
    return record, payload


def active_path(root, channel, kind):
    return root / channel / (kind + '-active.json')


def migrate_config(data):
    path = data / 'config.json'
    if not path.exists():
        return
    value = json.loads(path.read_text())
    bindings = value.get('bind', {})
    if 'gateway' in bindings:
        if 'station' in bindings and bindings['station'] != bindings['gateway']:
            raise RuntimeError('Conflicting gateway/station bindings; refusing migration')
        bindings['station'] = bindings.pop('gateway')
        atomic_json(path, value)


def apply_candidate(root, channel, candidate_dir):
    record, payload = load_candidate(candidate_dir)
    kind = record['kind']
    require_no_pending(root, channel, kind)
    if record['channel'] != channel:
        raise RuntimeError('Candidate belongs to another release/dev channel')
    settings = channel_config(root, channel, kind)
    if not settings.get('health', {}).get('profile') or not settings['health'].get('model'):
        raise RuntimeError('Configure an explicit health profile/model before switching')
    if channel == 'release':
        observe(root, 'dev', kind)
        promotion = json.loads((Path(candidate_dir) / 'promotion.json').read_text())
        parent = promotion['promoted_from']
        current_dev = json.loads(active_path(root, 'dev', kind).read_text())['record']
        if parent != current_dev['id'] or current_dev['source'] != record['source']:
            raise RuntimeError('Release accepts only the currently observed dev build')
        observed = json.loads(events_path(root, kind).read_text())
        stability(observed, parent)
    runtime = runtime_for(settings, root, channel, kind)
    if kind == 'app':
        from deployment_macos import validate_app
        validate_app(payload, channel, settings.get('id_prefix'))
    data, destination = Path(settings['data']), Path(settings['payload'])
    roots = [data, destination, *(Path(p) for p in settings.get('ancillary', []))]
    if settings.get('preferences'):
        roots.append(Path(settings['preferences']))
    if channel == 'dev':
        # Even reinstalling the same bytes is a new deployment, not continuous use.
        record_observation(root, kind, record['id'], False, reason='deployment started')
    transaction_dir = root / 'transactions' / (time.strftime('%Y%m%dT%H%M%S') + '-' + uuid.uuid4().hex[:10])
    previous = active_path(root, channel, kind)
    metadata = {'channel': channel, 'kind': kind, 'settings': settings,
                'previous_active': json.loads(previous.read_text()) if previous.exists() else None,
                'candidate': str(Path(candidate_dir).resolve()), 'candidate_id': record['id']}
    transaction = Transaction(transaction_dir, runtime, roots, metadata)
    atomic_json(root / channel / (kind + '-pending.json'), {'transaction': str(transaction_dir)})

    def activate(tx):
        # Configuration migration begins only after every snapshot was verified.
        migrate_config(data / 'node' if kind == 'app' else data)
        replace_directory(payload, destination, tx)
        if kind == 'node':
            atomic_json(destination / 'deployment.json', record)

    def accepted_health():
        verify_manifest(destination, record)
        evidence = runtime.health()
        evidence.update(runtime.verify_preserved(transaction.journal['original']))
        verify_manifest(destination, record)
        return evidence

    finalized = False
    try:
        evidence = transaction.run(activate, accepted_health)
        active = {'candidate': str(Path(candidate_dir).resolve()), 'record': record,
                  'transaction': str(transaction_dir), 'health': evidence, 'accepted_at': time.time()}
        atomic_json(active_path(root, channel, kind), active)
        if channel == 'dev':
            record_observation(root, kind, record['id'], True, health=evidence)
        finalized = True
        return active
    except BaseException as error:
        # Publishing the active pointer/observation is part of the transaction.
        # A full disk after the health check must not leave an unrecorded new app.
        if transaction.journal['phase'] == 'accepted':
            transaction.recover()
        if transaction.journal['phase'] == 'rolled_back':
            if metadata['previous_active']:
                atomic_json(previous, metadata['previous_active'])
            else:
                previous.unlink(missing_ok=True)
        if channel == 'dev':
            record_observation(root, kind, record['id'], False, error=str(error))
        finalized = transaction.journal['phase'] == 'rolled_back'
        raise
    finally:
        if finalized:
            (root / channel / (kind + '-pending.json')).unlink(missing_ok=True)


def observe(root, channel, kind, health=None):
    require_no_pending(root, channel, kind)
    path = active_path(root, channel, kind)
    if not path.exists():
        raise RuntimeError(f'No accepted {channel} {kind} build record; deploy and verify a captured candidate before starting its observation period')
    active = json.loads(path.read_text())
    settings = dict(channel_config(root, channel, kind))
    if health:
        settings['health'] = dict(settings.get('health', {}), **health)
    runtime = runtime_for(settings, root, channel, kind)
    candidate = active['record']['id']
    try:
        verify_manifest(Path(settings['payload']), active['record'])
        evidence = runtime.health()
        verify_manifest(Path(settings['payload']), active['record'])
    except BaseException as error:
        if channel == 'dev':
            record_observation(root, kind, candidate, False, error=str(error))
        raise
    if channel == 'dev':
        record_observation(root, kind, candidate, True, health=evidence)
    return evidence


def promote(root, kind, repo):
    # The live dev image and a fresh chat must agree before inspecting its week of receipts.
    observe(root, 'dev', kind)
    active = json.loads(active_path(root, 'dev', kind).read_text())
    observations = json.loads(events_path(root, kind).read_text())
    return prepare_promotion(active['candidate'], root / 'candidates', repo, observations)


def prepare_promotion(candidate, store, repo, observations):
    source_record, source = load_candidate(candidate)
    kind = source_record['kind']
    if source_record['channel'] != 'dev':
        raise RuntimeError('Only a dev candidate can be promoted')
    accepted = stability(observations, source_record['id'])
    stage = store / ('.promote-' + uuid.uuid4().hex)
    stage.mkdir(parents=True, mode=0o700)
    payload = stage / source.name
    if kind == 'app':
        promote_app(repo, source, payload)
    else:
        copy_tree(source, payload)
    # Same captured Cargo build; no --release rebuild can silently change the tested code.
    record = manifest(payload, source_record['source'], 'release', kind)
    atomic_json(stage / 'deployment.json', record)
    atomic_json(stage / 'promotion.json', {'promoted_from': source_record['id'], 'stability': accepted})
    final = stage.with_name(record['id'])
    if final.exists():
        load_candidate(final)
        shutil.rmtree(stage)
    else:
        stage.rename(final)
    return final


def recover(root, directory):
    journal = json.loads((directory / 'journal.json').read_text())
    metadata = journal['runtime']
    runtime = runtime_for(metadata['settings'], root, metadata['channel'], metadata['kind'])
    transaction = Transaction.load(directory, runtime)
    transaction.recover()
    active = active_path(root, metadata['channel'], metadata['kind'])
    if metadata.get('previous_active'):
        atomic_json(active, metadata['previous_active'])
    else:
        active.unlink(missing_ok=True)
    if metadata['channel'] == 'dev':
        record_observation(root, metadata['kind'], metadata['candidate_id'], False, reason='manual recovery')
    pending = root / metadata['channel'] / (metadata['kind'] + '-pending.json')
    if pending.exists() and json.loads(pending.read_text())['transaction'] == str(directory):
        pending.unlink()
    return transaction.journal['recovery_health']


def require_no_pending(root, channel, kind):
    path = root / channel / (kind + '-pending.json')
    if path.exists():
        raise RuntimeError('An interrupted transaction requires recovery first: ' + path.read_text().strip())


def install_tools(root, repo):
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    installed = root / 'tools' / ('recovery-' + uuid.uuid4().hex[:10])
    (installed / 'dev').mkdir(parents=True)
    (installed / 'lib').mkdir()
    shutil.copy2(SCRIPTS / 'dev/recovery.py', installed / 'dev/recovery.py')
    for name in ('deployment.py', 'deployment_build.py', 'deployment_health.py', 'deployment_macos.py', 'build_env.py'):
        shutil.copy2(SCRIPTS / 'lib' / name, installed / 'lib' / name)
    binaries = root / 'bin'
    binaries.mkdir(exist_ok=True)
    for name, prefix in {'zork-node': ['node'], 'zork-dev-build': ['build'],
                         'zork-promote': ['promote'], 'zork-app-build': ['build', '--kind', 'app']}.items():
        wrapper = '#!/usr/bin/env python3\nimport os, sys\n'
        wrapper += f'args = sys.argv[1:]\nbase = {prefix!r}\n'
        if name in ('zork-dev-build', 'zork-app-build', 'zork-promote'):
            wrapper += "if args and not args[0].startswith('-'):\n    base += ['--repo', args.pop(0)]\n"
            wrapper += "args = ['--switch' if a == '--yes' else a for a in args]\n"
        wrapper += f"os.execv(sys.executable, [sys.executable, {str(installed / 'dev/recovery.py')!r}, '--root', {str(root)!r}, *base, *args])\n"
        temporary = binaries / ('.' + name)
        temporary.write_text(wrapper)
        temporary.chmod(0o755)
        temporary.replace(binaries / name)
    path = root / 'deployment-config.json'
    if not path.exists():
        channels = {}
        for channel in ('release', 'dev'):
            channels[channel] = {
                'node': {'data': str(root / channel / 'data'), 'payload': str(root / channel / 'bin'), 'health': {}},
                'app': {'data': str(root / ('client-dev' if channel == 'dev' else 'client-release')),
                        'payload': str(root / 'apps' / ('Zork Dev.app' if channel == 'dev' else 'Zork.app')), 'health': {}}}
        atomic_json(path, {'repo': str(repo), 'channels': channels})
    return {'tools': str(installed), 'config': str(path)}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=Path, default=Path.home() / 'Zork')
    commands = parser.add_subparsers(dest='command', required=True)
    install = commands.add_parser('install-tools')
    install.add_argument('--repo', type=Path, default=SCRIPTS.parent)
    for action in ('build', 'promote'):
        command = commands.add_parser(action)
        command.add_argument('--repo', type=Path)
        command.add_argument('--kind', choices=('node', 'app'), default='node')
        command.add_argument('--switch', action='store_true')
        if action == 'build':
            command.add_argument('--profile', choices=('dev', 'release'), default='dev')
            command.add_argument('--services-config', type=Path)
    apply = commands.add_parser('apply')
    apply.add_argument('channel', choices=('release', 'dev'))
    apply.add_argument('candidate', type=Path)
    node = commands.add_parser('node')
    node.add_argument('channel', choices=('release', 'dev'))
    node.add_argument('action', choices=('start', 'stop', 'status', 'health', 'logs'))
    node.add_argument('--kind', choices=('node', 'app'), default='node')
    node.add_argument('--profile', help='Health Profile override for this check')
    node.add_argument('--model', help='Health model override for this check')
    node.add_argument('--thinking', help='Health thinking override for this check')
    recovery = commands.add_parser('recover')
    recovery.add_argument('transaction', type=Path)
    args = parser.parse_args(argv)
    health = {name: getattr(args, name) for name in ('profile', 'model', 'thinking')
              if args.command == 'node' and getattr(args, name) is not None}
    if health and args.action not in ('start', 'health'):
        parser.error('Health overrides apply only to start/health')
    root = args.root.expanduser().resolve()
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    with exclusive(root / 'deployment.lock'):
        if args.command == 'install-tools':
            result = install_tools(root, args.repo.resolve())
        elif args.command in ('build', 'promote'):
            repo = (args.repo or Path(config(root)['repo'])).resolve()
            channel = 'dev' if args.command == 'build' else 'release'
            require_no_pending(root, channel, args.kind)
            candidate = (build(repo, root / 'candidates', args.kind, args.profile, args.services_config)
                         if args.command == 'build' else promote(root, args.kind, repo))
            result = apply_candidate(root, channel, candidate) if args.switch else {'candidate': str(candidate)}
        elif args.command == 'apply':
            record, _ = load_candidate(args.candidate)
            require_no_pending(root, args.channel, record['kind'])
            result = apply_candidate(root, args.channel, args.candidate)
        elif args.command == 'recover':
            result = recover(root, args.transaction.resolve())
        else:
            settings = dict(channel_config(root, args.channel, args.kind))
            if health:
                settings['health'] = dict(settings.get('health', {}), **health)
            runtime = runtime_for(settings, root, args.channel, args.kind)
            if args.action in ('start', 'stop', 'health'):
                require_no_pending(root, args.channel, args.kind)
            if args.action == 'start':
                before = runtime.capture()
                if not before['running']:
                    runtime.start(before, candidate=True)
                result = runtime.health()
            elif args.action == 'stop':
                runtime.stop()
                result = {'stopped': True}
            elif args.action == 'health':
                result = observe(root, args.channel, args.kind, health)
            elif args.action == 'logs':
                result = {'log': str(runtime.log), 'tail': runtime.log.read_text(errors='replace')[-8000:] if runtime.log.exists() else ''}
            else:
                result = runtime.capture()
                path = active_path(root, args.channel, args.kind)
                result['accepted_build'] = json.loads(path.read_text())['record'] if path.exists() else None
                if result['running']:
                    try:
                        if path.exists():
                            verify_manifest(Path(settings['payload']), json.loads(path.read_text())['record'])
                        result['live'] = runtime.health(chat=False, wait_for_ready=False)
                    except Exception as error:
                        result['error'] = str(error)
        if isinstance(result, dict) and 'record' in result:
            result = {key: result[key] for key in ('candidate', 'transaction', 'health')}
        print(json.dumps(result, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print(f'ERROR: {error}', file=sys.stderr)
        raise SystemExit(1)
