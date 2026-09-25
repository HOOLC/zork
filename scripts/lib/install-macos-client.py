#!/usr/bin/env python3
"""Install a verified channel candidate with a complete, recoverable data transaction."""
import argparse
import importlib.util
import json
from pathlib import Path
import shutil

from channels import CHANNELS, app_name, client_root
from deployment import atomic_json, digest, exclusive, extract_candidate
from deployment_macos import validate_app
import deployment_retention as retention


def recovery_module():
    path = Path(__file__).resolve().parents[1] / 'dev/recovery.py'
    spec = importlib.util.spec_from_file_location('recovery', path)
    recovery = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(recovery)
    return recovery


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('stage', type=Path)
    parser.add_argument('sha256')
    parser.add_argument('--channel', choices=CHANNELS, required=True)
    parser.add_argument('--root', type=Path, default=Path.home() / 'Zork')
    parser.add_argument('--profile', required=True)
    parser.add_argument('--model', required=True)
    parser.add_argument('--thinking', default='off')
    args = parser.parse_args()
    stage, root = args.stage.resolve(), args.root.expanduser().resolve()
    archive = stage / 'app.tar.gz'
    if digest(archive) != args.sha256:
        raise RuntimeError('Installation archive digest differs; app untouched')
    unpacked = stage / 'unpacked'
    unpacked.mkdir()
    extract_candidate(archive, unpacked)
    incoming = unpacked / 'candidate'
    recovery = recovery_module()
    record, app = recovery.load_candidate(incoming)
    if record['kind'] != 'app' or record['channel'] != args.channel:
        raise RuntimeError('Installation candidate has the wrong channel or product kind')
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    with exclusive(root / 'deployment.lock'):
        settings_path = root / 'deployment-config.json'
        if settings_path.exists():
            settings = json.loads(settings_path.read_text())
        else:
            settings = {'repo': '', 'channels': {name: {} for name in CHANNELS}}
        for channel in CHANNELS:
            settings['channels'].setdefault(channel, {})
            data = client_root(channel)
            settings['channels'][channel].setdefault('app', {
                'data': str(data),
                'preferences': str((data if channel != 'release' else data.parent) / 'preferences.json'),
                'ancillary': [] if channel != 'release' else [str(data.parent / 'preferences.json'), str(data.parent / 'logs/client.log')],
                'payload': str(Path.home() / 'Applications' / (app_name(channel) + '.app'))})
        selected = settings['channels'][args.channel]['app']
        selected.setdefault('health', {}).update(profile=args.profile, model=args.model, thinking=args.thinking)
        validate_app(app, args.channel, selected.get('id_prefix'))
        atomic_json(settings_path, settings)
        recovery.channel_config(root, args.channel, 'app')
        recovery.require_no_pending(root, args.channel, 'app')
        # Store copies live below *.noindex so Spotlight never registers them.
        retention.ensure_noindex_layout(root)
        candidate = root / 'candidates' / record['id']
        candidate.parent.mkdir(parents=True, exist_ok=True)
        if candidate.exists():
            recovery.load_candidate(candidate)
        else:
            shutil.move(incoming, candidate)
        # Install the independent recovery entry before any live mutation, including
        # a failed first deployment; recovery never depends on the failed GUI.
        tools = root / 'tools' / ('installer-' + record['id'][:16])
        if not tools.exists():
            shutil.copytree(Path(__file__).resolve().parents[1], tools)
        recovery.install_tools(root, Path(settings['repo'] or '.'))
        try:
            result = recovery.apply_candidate(root, args.channel, candidate)
        except BaseException:
            # A rollback moves the failed bundle into the transaction; keep it unregistered.
            retention.safe_sweep(root)
            raise
        result['recovery_argv'] = ['python3', str(tools / 'dev/recovery.py'), '--root', str(root),
                                   'recover', result['transaction']]
        atomic_json(stage / 'result.json', result)
        # Accepted: prune superseded candidates/transactions/tools and unregister store app copies.
        retention.after_accept(root)
    print(json.dumps({key: result[key] for key in ('candidate', 'transaction', 'health', 'recovery_argv')}, ensure_ascii=False, indent=2))


if __name__ == '__main__':
    main()
