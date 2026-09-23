"""Durable local deployment transactions. Callers own process and health policy."""
from contextlib import contextmanager
import ctypes
import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import stat
import sys
import time
import tarfile
import traceback
import uuid

TOOL_FILES = ('lib/channels.py', 'dev/recovery.py', 'lib/install-macos-client.py', 'lib/deployment.py',
              'lib/deployment_build.py', 'lib/deployment_health.py', 'lib/deployment_macos.py',
              'lib/build_env.py', 'lib/cua_build.py')


def digest(path):
    result = hashlib.sha256()
    with Path(path).open('rb') as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            result.update(chunk)
    return result.hexdigest()


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def atomic_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    temporary = path.with_name('.' + path.name + '-' + uuid.uuid4().hex)
    with open(temporary, 'x', opener=lambda p, flags: os.open(p, flags, 0o600)) as output:
        json.dump(value, output, ensure_ascii=False, indent=2)
        output.write('\n')
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)
    sync_directory(path.parent)


@contextmanager
def exclusive(path):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    with path.open('a+') as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise RuntimeError(f'Another deployment owns {path}') from None
        yield


def clone_file(source, target):
    """Clone only when supported; IO/permission failures are fatal."""
    if sys.platform == 'darwin':
        libc = ctypes.CDLL('libSystem.B.dylib', use_errno=True)
        if libc.clonefile(os.fsencode(source), os.fsencode(target), 0) == 0:
            return
        code = ctypes.get_errno()
        if code not in (errno.EXDEV, errno.ENOTSUP, errno.EINVAL):
            raise OSError(code, os.strerror(code), str(source))
    shutil.copy2(source, target)


def copy_tree(source, target):
    """Copy stopped storage, preserving links; sockets are recreated by the runtime."""
    source, target = Path(source), Path(target)
    if source.is_symlink():
        target.symlink_to(os.readlink(source))
    elif source.is_dir():
        target.mkdir(mode=stat.S_IMODE(source.stat().st_mode))
        for child in source.iterdir():
            copy_tree(child, target / child.name)
        shutil.copystat(source, target)
        sync_directory(target)
    elif source.is_file():
        clone_file(source, target)
        with target.open('rb') as handle:
            os.fsync(handle.fileno())
    elif not stat.S_ISSOCK(source.lstat().st_mode):
        raise RuntimeError(f'Unsupported storage entry: {source}')


def inventory(root):
    root = Path(root)
    if not root.exists() and not root.is_symlink():
        return None
    result = {}
    for path in [root, *sorted(root.rglob('*'))] if root.is_dir() else [root]:
        metadata = path.lstat()
        name = str(path.relative_to(root))
        mode = stat.S_IMODE(metadata.st_mode)
        if path.is_symlink():
            result[name] = ['link', os.readlink(path)]
        elif stat.S_ISREG(metadata.st_mode):
            result[name] = ['file', mode, metadata.st_size, digest(path)]
        elif stat.S_ISDIR(metadata.st_mode):
            result[name] = ['dir', mode]
        elif not stat.S_ISSOCK(metadata.st_mode):
            raise RuntimeError(f'Unsupported storage entry: {path}')
    return result


def independent_roots(paths):
    roots = sorted(dict.fromkeys(Path(path).absolute() for path in paths), key=lambda p: len(p.parts))
    result = []
    for path in roots:
        if path.is_symlink() or path.resolve() != path:
            raise RuntimeError(f'Deployment root must be canonical, not a symlink: {path}')
        if not any(path.is_relative_to(parent) for parent in result):
            result.append(path)
    return result


def check_links(roots):
    # External mutable storage cannot be restored by this transaction.
    for root in roots:
        for path in root.rglob('*') if root.exists() else []:
            if path.is_symlink() and not any(path.resolve().is_relative_to(p) for p in roots):
                raise RuntimeError(f'Storage link leaves the backed-up roots: {path}')


class Transaction:
    """A journal and verified snapshot survive both exceptions and process death.

    No caller mutation is allowed before ``backed_up``. A failed version is moved
    aside during restore, so its new history is retained separately for inspection.
    """
    def __init__(self, directory, runtime, roots, metadata):
        self.directory = Path(directory)
        self.runtime = runtime
        self.roots = independent_roots(roots)
        if any(self.directory.resolve().is_relative_to(p) for p in self.roots):
            raise RuntimeError('Recovery journal must live outside the deployed data')
        self.directory.mkdir(parents=True, mode=0o700)
        self.journal = {'schema': 1, 'phase': 'prepared', 'created_at': time.time(),
                        'runtime': metadata, 'roots': [str(p) for p in self.roots],
                        'original': runtime.capture(), 'snapshots': {}}
        self.save('prepared')

    @classmethod
    def load(cls, directory, runtime):
        transaction = cls.__new__(cls)
        transaction.directory, transaction.runtime = Path(directory), runtime
        transaction.journal = json.loads((transaction.directory / 'journal.json').read_text())
        transaction.roots = independent_roots(transaction.journal['roots'])
        return transaction

    def save(self, phase, **fields):
        updated = dict(self.journal, phase=phase, updated_at=time.time(), **fields)
        atomic_json(self.directory / 'journal.json', updated)
        self.journal = updated

    def backup(self):
        check_links(self.roots)
        for index, root in enumerate(self.roots):
            before = inventory(root)
            snapshot = self.directory / f'snapshot-{index}'
            if before is not None:
                copy_tree(root, snapshot)
            if inventory(root) != before or inventory(snapshot) != before:
                raise RuntimeError(f'Snapshot verification failed: {root}')
            self.journal['snapshots'][str(root)] = before
        self.save('backed_up', backup_complete=True)

    def run(self, apply, health):
        mutated = False
        try:
            self.save('stopping')
            self.runtime.stop()
            self.save('stopped')
            self.backup()
            self.save('applying')
            mutated = True
            apply(self)
            self.save('starting')
            self.runtime.start(self.journal['original'], candidate=True)
            self.save('checking')
            evidence = health()
            self.save('accepted', health=evidence)
            return evidence
        except BaseException as error:
            try:
                self.save('failed', error=f'{type(error).__name__}: {error}')
            except OSError:
                # A full backup disk must not prevent restarting the unchanged
                # original. The last durable journal still describes recovery.
                pass
            try:
                if mutated:
                    self.recover()
                else:
                    original = self.journal['original']
                    self.runtime.start(original, candidate=False)
                    evidence = self.runtime.verify_original(original)
                    self.save('rolled_back', recovery_health=evidence)
            except BaseException as recovery_error:
                try:
                    self.save('recovery_failed', recovery_error=f'{type(recovery_error).__name__}: {recovery_error}',
                              recovery_traceback=traceback.format_exc())
                except OSError:
                    pass
                raise RuntimeError(f'Deployment and recovery failed; retain {self.directory}: {recovery_error}') from error
            raise

    def recover(self):
        # Verify all snapshots before stopping anything or replacing live paths.
        if self.journal.get('backup_complete'):
            for index, root in enumerate(self.roots):
                expected = self.journal['snapshots'][str(root)]
                if inventory(self.directory / f'snapshot-{index}') != expected:
                    raise RuntimeError(f'Recovery snapshot is damaged: {root}')
            self.save('restoring')
            self.runtime.stop()
            for index, root in enumerate(self.roots):
                snapshot = self.directory / f'snapshot-{index}'
                root.parent.mkdir(parents=True, exist_ok=True)
                restored = root.with_name('.' + root.name + '-restore-' + self.directory.name)
                displaced = self.directory / f'failed-{index}'
                if restored.exists():
                    shutil.rmtree(restored) if restored.is_dir() else restored.unlink()
                if snapshot.exists():
                    copy_tree(snapshot, restored)
                    if inventory(restored) != self.journal['snapshots'][str(root)]:
                        raise RuntimeError(f'Restored copy differs: {root}')
                if root.exists():
                    # Each retry retains the version it encountered; never delete history.
                    if displaced.exists():
                        displaced = displaced.with_name(displaced.name + '-' + uuid.uuid4().hex)
                    shutil.move(str(root), displaced)
                if restored.exists():
                    os.replace(restored, root)
                sync_directory(root.parent)
            lineage = self.runtime.after_restore()
            self.save('restored', lineage=lineage)
        self.save('restarting_original')
        original = self.journal['original']
        self.runtime.start(original, candidate=False)
        evidence = self.runtime.verify_original(original)
        self.save('rolled_back', recovery_health=evidence)


def replace_directory(source, destination, transaction):
    destination = Path(destination)
    incoming = destination.with_name('.' + destination.name + '-incoming-' + transaction.directory.name)
    if incoming.exists():
        raise RuntimeError(f'Unresolved staged payload: {incoming}')
    incoming.parent.mkdir(parents=True, exist_ok=True)
    copy_tree(source, incoming)
    if inventory(incoming) != inventory(source):
        raise RuntimeError('Staged payload changed while being copied')
    if destination.exists():
        shutil.move(str(destination), transaction.directory / 'displaced-payload')
    os.replace(incoming, destination)
    sync_directory(destination.parent)


def manifest(payload, source, channel, kind):
    files = inventory(payload)
    files.pop('deployment.json', None)
    value = {'schema': 1, 'source': source, 'channel': channel, 'kind': kind, 'files': files}
    value['id'] = hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()
    return value


def verify_manifest(payload, record):
    actual = manifest(payload, record['source'], record['channel'], record['kind'])
    if actual != record:
        raise RuntimeError('Payload does not match its recorded build; rebuild before deployment')
    return record


def extract_candidate(archive, destination):
    """Extract our self-contained candidate format, including on macOS Python 3.9.

    Links are created last and cannot be archive parents. No tarfile extraction
    method can write through a symlink or restore a device/owner from the archive.
    """
    destination = Path(destination).resolve()
    if any(destination.iterdir()):
        raise RuntimeError('Candidate extraction requires an empty staging directory')
    with tarfile.open(archive) as package:
        members = package.getmembers()
        names, links, directories = set(), [], []
        for member in members:
            name = member.name.rstrip('/')
            parts = PurePosixPath(name).parts
            if (not parts or parts[0] != 'candidate' or any(p in ('.', '..') for p in parts)
                    or name.startswith('/') or name in names
                    or not (member.isfile() or member.isdir() or member.issym())):
                raise RuntimeError('Unexpected archive entry: ' + member.name)
            names.add(name)
            if member.issym():
                links.append((member, destination.joinpath(*parts)))
        for member, _ in links:
            if member.linkname.startswith('/') or any(name.startswith(member.name.rstrip('/') + '/') for name in names):
                raise RuntimeError('Archive link is absolute or is used as a parent: ' + member.name)
        for member in members:
            path = destination.joinpath(*PurePosixPath(member.name).parts)
            if member.isdir():
                path.mkdir(parents=True, exist_ok=True)
                directories.append((path, member.mode & 0o777))
            elif member.isfile():
                path.parent.mkdir(parents=True, exist_ok=True)
                with package.extractfile(member) as source, path.open('xb') as output:
                    shutil.copyfileobj(source, output)
                path.chmod(member.mode & 0o777)
        for member, path in links:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.symlink_to(member.linkname)
        for _, path in links:
            if not path.resolve().is_relative_to(destination / 'candidate'):
                raise RuntimeError('Archive link leaves the candidate: ' + str(path))
        for path, mode in reversed(directories):
            path.chmod(mode)
