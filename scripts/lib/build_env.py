"""Optional machine-local build settings. No shell evaluation of .env values."""
import argparse
import json
import os
from pathlib import Path
import shlex
import subprocess

ROOT = Path(__file__).resolve().parents[2]
KEYS = {'ZORK_BUILD_ROOT', 'ZORK_BUILD_BUDGET_GIB', 'ZORK_BUILD_LOW_WATER_GIB',
        'ZORK_BUILD_MOUNT', 'CARGO_TARGET_DIR', 'KACHE_CACHE_EXECUTABLES',
        'ZORK_ANDROID_DEBUG_KEYSTORE', 'ZORK_WASM_LD'}

# Repository context inherited from Git hooks must not redirect a dependency's
# git init/fetch/checkout into this repository. Keep transport/authentication
# settings such as GIT_SSH_COMMAND and GIT_ASKPASS available to dependency fetches.
GIT_CONTEXT = frozenset('''GIT_ALTERNATE_OBJECT_DIRECTORIES GIT_CONFIG
    GIT_CONFIG_PARAMETERS GIT_CONFIG_COUNT GIT_OBJECT_DIRECTORY GIT_DIR
    GIT_WORK_TREE GIT_IMPLICIT_WORK_TREE GIT_GRAFT_FILE GIT_INDEX_FILE
    GIT_NO_REPLACE_OBJECTS GIT_REPLACE_REF_BASE GIT_PREFIX GIT_SHALLOW_FILE
    GIT_COMMON_DIR'''.split())


def git_context_key(key):
    return key in GIT_CONTEXT or key.startswith(('GIT_CONFIG_KEY_', 'GIT_CONFIG_VALUE_'))


def settings(root=ROOT, environ=None):
    env = dict(os.environ if environ is None else environ)
    path = root / '.env'
    if path.exists():
        for line in path.read_text().splitlines():
            line = line.strip()
            if line.startswith('export '):
                line = line[7:]
            key, sep, raw = line.partition('=')
            key = key.strip()
            if sep and key in KEYS and key not in env:
                values = shlex.split(raw, comments=True)
                if len(values) > 1:
                    raise ValueError(f'{key}: quote paths containing spaces')
                env[key] = values[0] if values else ''
    return env


def build_environment(root=ROOT, environ=None, variant=None):
    env = {key: value for key, value in settings(root, environ).items()
           if not git_context_key(key)}
    mount = env.get('ZORK_BUILD_MOUNT')
    if mount and not os.path.ismount(Path(mount).expanduser()):
        raise ValueError('Configured ZORK_BUILD_MOUNT is not mounted')
    if not env.get('CARGO_TARGET_DIR'):
        configured = env.get('ZORK_BUILD_ROOT')
        base = Path(configured or root / 'target').expanduser()
        if not base.is_absolute():
            base = root / base
        if configured:
            base /= variant or 'target'
        elif variant:
            base /= variant
        env['CARGO_TARGET_DIR'] = str(base)
    else:
        target = Path(env['CARGO_TARGET_DIR']).expanduser()
        env['CARGO_TARGET_DIR'] = str(target if target.is_absolute() else root / target)
    for key in ('ZORK_ANDROID_DEBUG_KEYSTORE', 'ZORK_WASM_LD'):
        if env.get(key):
            path = Path(env[key]).expanduser()
            env[key] = str(path if path.is_absolute() else root / path)
    return env


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--shell', action='store_true', help='Print quoted exports for eval')
    parser.add_argument('--json', action='store_true', help='Show build settings only')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    env = build_environment()
    public = {k: env[k] for k in sorted(KEYS) if k in env}
    if args.shell:
        for key in sorted(os.environ):
            if git_context_key(key):
                print(f'unset {shlex.quote(key)}')
        for key, value in public.items():
            print(f'export {key}={shlex.quote(value)}')
    elif args.json:
        print(json.dumps(public, indent=2))
    else:
        command = args.command
        if command[:1] == ['--']:
            command = command[1:]
        if not command:
            parser.error('provide a command or --shell/--json')
        raise SystemExit(subprocess.call(command, cwd=ROOT, env=env))


if __name__ == '__main__':
    main()
