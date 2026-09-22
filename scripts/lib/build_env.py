"""Optional machine-local build settings. No shell evaluation of .env values."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import shlex
import subprocess

ROOT = Path(__file__).resolve().parents[2]
CACHEDIR_SIGNATURE = 'Signature: 8a477f597d28d172789f06886806bc55'
KEYS = {'ZORK_BUILD_ROOT', 'ZORK_BUILD_BUDGET_GIB', 'ZORK_BUILD_LOW_WATER_GIB',
        'ZORK_BUILD_MOUNT', 'CARGO_TARGET_DIR', 'KACHE_CACHE_EXECUTABLES',
        'ZORK_ANDROID_DEBUG_KEYSTORE', 'ZORK_WASM_LD', 'ZORK_CUA_SOURCE', 'ZORK_CUA_TARGET_DIR'}

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


def clean_git_environment(environ):
    return {key: value for key, value in environ.items() if not git_context_key(key)}


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
    env = clean_git_environment(settings(root, environ))
    mount = env.get('ZORK_BUILD_MOUNT')
    if mount and not os.path.ismount(Path(mount).expanduser()):
        raise ValueError('Configured ZORK_BUILD_MOUNT is not mounted')
    if not env.get('CARGO_TARGET_DIR'):
        configured = env.get('ZORK_BUILD_ROOT')
        base = Path(configured or root / 'target').expanduser()
        if not base.is_absolute():
            base = root / base
        if configured:
            if variant:
                base /= variant
            elif (root / '.git').is_file():
                base /= f'isolated/{root.name}'
            else:
                base /= 'target'
        elif variant:
            base /= variant
        env['CARGO_TARGET_DIR'] = str(base)
    else:
        target = Path(env['CARGO_TARGET_DIR']).expanduser()
        env['CARGO_TARGET_DIR'] = str(target if target.is_absolute() else root / target)
    for key in ('ZORK_ANDROID_DEBUG_KEYSTORE', 'ZORK_WASM_LD', 'ZORK_CUA_SOURCE', 'ZORK_CUA_TARGET_DIR'):
        if env.get(key):
            path = Path(env[key]).expanduser()
            env[key] = str(path if path.is_absolute() else root / path)
    return env


def register_isolated_target(env, root=ROOT):
    """Bind a worktree-named isolated target to its source worktree."""
    configured = env.get('ZORK_BUILD_ROOT')
    if not configured:
        return
    cache_root = Path(configured).expanduser()
    cache_root = (cache_root if cache_root.is_absolute() else root / cache_root).resolve()
    target = Path(env['CARGO_TARGET_DIR']).resolve()
    worktree = root.resolve()
    isolated = cache_root / 'isolated'
    try:
        relative = target.relative_to(isolated)
    except ValueError:
        return
    if relative.parts not in ((worktree.name,), (worktree.name, 'target')):
        return
    cache_root.mkdir(parents=True, exist_ok=True)
    with (cache_root / '.zork-cache-gc.lock').open('a+') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            target.mkdir(parents=True)
        except FileExistsError:
            pass
        else:
            (target / 'CACHEDIR.TAG').write_text(
                CACHEDIR_SIGNATURE + '\n# Isolated Cargo target created by Zork build_env.py.\n')
        marker = target / '.zork-cache-owner.json'
        owner = {'worktree': str(worktree)}
        if marker.exists():
            previous = json.loads(marker.read_text())
            if previous == owner:
                return
            if Path(previous['worktree']).exists():
                raise ValueError(f'Isolated target belongs to another worktree: {target}')
        pending = marker.with_name(marker.name + f'.{os.getpid()}.tmp')
        try:
            pending.write_text(json.dumps(owner) + '\n')
            pending.replace(marker)
        finally:
            pending.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--shell', action='store_true', help='Print quoted exports for eval')
    parser.add_argument('--json', action='store_true', help='Show build settings only')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    env = build_environment()
    if args.shell or args.command:
        register_isolated_target(env)
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
