#!/usr/bin/env python3
"""Build on this checkout and install a release/dev/test candidate on a Mac over SSH."""
import argparse
import importlib.util
import json
from pathlib import Path
import shlex
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts/lib'))
from channels import CHANNELS, app_name
from deployment import atomic_json, digest, exclusive, TOOL_FILES
from deployment_build import build, stability


def recovery_module():
    spec = importlib.util.spec_from_file_location('recovery', ROOT / 'scripts/dev/recovery.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--host', required=True)
    parser.add_argument('--control', help='Existing authenticated SSH control socket')
    parser.add_argument('--channel', choices=CHANNELS, default='test')
    parser.add_argument('--candidate', type=Path, help='Immutable candidate directory from recovery.py build/promote')
    parser.add_argument('--expected-build-id', help='Require exactly this whole candidate, including all helpers')
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument('--observe', action='store_true', help='Run a daily chat check of installed dev; do not install')
    mode.add_argument('--promote', action='store_true', help='Observe MBA dev and prepare its same-code release candidate')
    parser.add_argument('--switch', action='store_true', help='With --promote, also install the accepted release candidate')
    parser.add_argument('--root', type=Path, default=Path.home() / 'Zork', help='Local build store and receipt directory')
    parser.add_argument('--profile', required=True)
    parser.add_argument('--model', required=True)
    parser.add_argument('--thinking', default='off')
    args = parser.parse_args()
    if args.host.startswith('-'):
        parser.error('Invalid SSH destination')
    if args.channel == 'release' and not args.candidate and not args.promote:
        parser.error('release requires an observed promoted candidate; it is never rebuilt during installation')
    if args.switch and not args.promote:
        parser.error('--switch is only used with --promote')
    root = args.root.expanduser().resolve()
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    recovery = recovery_module()
    ssh = ['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10']
    if args.control:
        ssh += ['-S', args.control]
    ssh += [args.host]
    identity = subprocess.check_output(ssh + ['hostname'], text=True).strip()
    if not identity or any(c in identity for c in ('/', '\\', '\n')):
        raise RuntimeError('Invalid remote hostname')
    receipts = root / 'devices' / identity
    with exclusive(root / 'deployment.lock'), tempfile.TemporaryDirectory(prefix='zork-install-') as scratch:
        scratch = Path(scratch)
        remote = None
        try:
            if args.observe or args.promote or args.channel == 'release':
                health_args = ['dev', 'health', '--kind', 'app', '--profile', args.profile,
                               '--model', args.model, '--thinking', args.thinking]
                subprocess.run(ssh + ['"$HOME/Zork/bin/zork-node" ' + shlex.join(health_args)], check=True)
                events = json.loads(subprocess.check_output(ssh + ['cat "$HOME/Zork/dev/app-observations.json"']))
                active = json.loads(subprocess.check_output(ssh + ['cat "$HOME/Zork/dev/app-active.json"']))
                atomic_json(receipts / 'app-observations.json', events)
                atomic_json(receipts / 'dev-active.json', active)
                if args.observe:
                    print(json.dumps({'host': identity, 'candidate': active['record']['id'], 'observed': True}))
                    return
                if args.promote:
                    dev_candidate = args.candidate or root / 'candidates' / active['record']['id']
                    dev_record, _ = recovery.load_candidate(dev_candidate)
                    if dev_record != active['record']:
                        raise RuntimeError('Local candidate differs from the dev build actually running on the destination')
                    args.candidate = recovery.prepare_promotion(dev_candidate, root / 'candidates', ROOT, events)
                    args.channel = 'release'
                    if not args.switch:
                        print(json.dumps({'host': identity, 'candidate': str(args.candidate), 'installed': False}))
                        return
            if args.candidate:
                candidate = args.candidate.resolve()
            else:
                installed = app_name(args.channel) + '.app'
                command = ('p="$HOME/Applications/' + installed + '/Contents/Resources/services.json"; '
                           'if test -f "$p"; then cat "$p"; fi')
                services = subprocess.check_output(ssh + [command])
                path = None
                if services:
                    path = scratch / 'services.json'
                    path.write_bytes(services)
                candidate = build(ROOT, root / 'candidates', 'app', services=path, channel=args.channel)
            record, app = recovery.load_candidate(candidate)
            if record['kind'] != 'app' or record['channel'] != args.channel:
                raise RuntimeError('Candidate channel/kind does not match the installation request')
            if args.expected_build_id and record['id'] != args.expected_build_id:
                raise RuntimeError('Candidate changed after validation; refusing another build')
            if args.channel == 'release':
                promotion = json.loads((candidate / 'promotion.json').read_text())
                if promotion['promoted_from'] != active['record']['id'] or record['source'] != active['record']['source']:
                    raise RuntimeError('Release candidate differs from the observed destination dev build')
                stability(json.loads((receipts / 'app-observations.json').read_text()), promotion['promoted_from'])
            archive = scratch / 'app.tar.gz'
            with tarfile.open(archive, 'w:gz') as package:
                package.add(app, arcname='candidate/Zork.app')
                package.add(candidate / 'deployment.json', arcname='candidate/deployment.json')
                if (candidate / 'promotion.json').exists():
                    package.add(candidate / 'promotion.json', arcname='candidate/promotion.json')
            tools_archive = scratch / 'tools.tar.gz'
            with tarfile.open(tools_archive, 'w:gz') as package:
                for name in TOOL_FILES:
                    package.add(ROOT / 'scripts' / name, arcname='tools/scripts/' + name)
            remote = subprocess.check_output(ssh + ['mkdir -p "$HOME/Zork" && mktemp -d "$HOME/Zork/.install-XXXXXXXX"'], text=True).strip()
            if not remote or '\n' in remote:
                raise RuntimeError('Invalid remote staging directory')
            for local in (archive, tools_archive):
                with local.open('rb') as stream:
                    subprocess.run(ssh + ['cat > ' + shlex.quote(remote + '/' + local.name)], stdin=stream, check=True)
            subprocess.run(ssh + ['tar -xzf ' + shlex.quote(remote + '/tools.tar.gz') + ' -C ' + shlex.quote(remote)], check=True)
            command = ['python3', remote + '/tools/scripts/lib/install-macos-client.py', remote, digest(archive),
                       '--channel', args.channel, '--profile', args.profile, '--model', args.model, '--thinking', args.thinking]
            subprocess.run(ssh + [shlex.join(command)], check=True)
            result = json.loads(subprocess.check_output(ssh + ['cat ' + shlex.quote(remote + '/result.json')]))
            if result['record'] != record:
                raise RuntimeError('Installed receipt is for another candidate')
            result['host'] = identity
            atomic_json(receipts / (args.channel + '-active.json'), result)
            if args.channel == 'dev':
                path = receipts / 'app-observations.json'
                events = json.loads(path.read_text()) if path.exists() else []
                events.append({'at': result['accepted_at'], 'candidate': record['id'], 'passed': True, 'health': result['health']})
                atomic_json(path, events)
            print(json.dumps({'host': identity, 'candidate': record['id'], 'receipt': str(receipts / (args.channel + '-active.json')),
                              'recovery': shlex.join(result['recovery_argv'])}, ensure_ascii=False, indent=2))
        finally:
            if remote:
                # Transaction snapshots and failed-version data are outside upload staging.
                subprocess.run(ssh + ['rm -rf -- ' + shlex.quote(remote)], check=True)


if __name__ == '__main__':
    main()
