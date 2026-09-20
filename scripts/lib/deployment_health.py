"""Bind deployment acceptance to the running image, storage and a full chat reply."""
import ctypes
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import subprocess
import sys
import time
import uuid
from urllib.request import Request, urlopen

from deployment import atomic_json, digest


def wait(check, description, seconds=60):
    deadline, last = time.monotonic() + seconds, 'not observed'
    while time.monotonic() < deadline:
        try:
            value = check()
            if value:
                return value
        except (OSError, ValueError, KeyError, RuntimeError) as error:
            last = str(error)
        time.sleep(.2)
    raise RuntimeError(f'{description}: {last}')


def control(data, command='status'):
    with socket.socket(socket.AF_UNIX) as stream:
        stream.settimeout(2)
        stream.connect(str(Path(data) / 'run/sup.sock'))
        stream.sendall((command + '\n').encode())
        with stream.makefile('r') as reader:
            value = reader.readline()
    if command == 'status':
        return json.loads(value)
    if value.strip() != 'ok':
        raise RuntimeError('Supervisor rejected ' + command)


def process_path(pid):
    if sys.platform == 'darwin':
        buffer = ctypes.create_string_buffer(4096)
        lib = ctypes.CDLL('/usr/lib/libproc.dylib')
        if lib.proc_pidpath(int(pid), buffer, len(buffer)) <= 0:
            return None
        return Path(os.fsdecode(buffer.value))
    try:
        return Path(os.readlink(f'/proc/{int(pid)}/exe'))
    except FileNotFoundError:
        return None


def running_image(pid, path, expected):
    path = Path(path).resolve(strict=True)
    if process_path(pid) != path or digest(path) != expected:
        raise RuntimeError(f'PID {pid} is not running the accepted image: {path}')
    if sys.platform == 'darwin':
        result = subprocess.run(['lsof', '-nP', '-a', '-p', str(pid), '-d', 'txt', '-Fni'],
                                check=True, capture_output=True, text=True)
        current_inode = None
        for line in result.stdout.splitlines():
            if line.startswith('i'):
                current_inode = int(line[1:])
            if line == 'n' + str(path) and current_inode == path.stat().st_ino:
                return {'pid': pid, 'path': str(path), 'sha256': expected, 'inode': current_inode}
        raise RuntimeError(f'PID {pid} still maps a replaced executable inode')
    if os.stat(f'/proc/{pid}/exe').st_ino != path.stat().st_ino:
        raise RuntimeError(f'PID {pid} maps an old executable')
    return {'pid': pid, 'path': str(path), 'sha256': expected}


def service_label(data):
    value = 0xcbf29ce484222325
    for byte in os.fsencode(Path(data).resolve()):
        value = ((value ^ byte) * 0x100000001b3) & ((1 << 64) - 1)
    return f'com.zork.node.{value:016x}'


def service_loaded(data):
    label = service_label(data)
    command = (['launchctl', 'print', f'gui/{os.getuid()}/{label}'] if sys.platform == 'darwin'
               else ['systemctl', '--user', 'is-active', label + '.service'])
    return subprocess.run(command, capture_output=True).returncode == 0


def pause_service(data):
    if service_loaded(data):
        label = service_label(data)
        command = (['launchctl', 'bootout', f'gui/{os.getuid()}/{label}'] if sys.platform == 'darwin'
                   else ['systemctl', '--user', 'stop', label + '.service'])
        subprocess.run(command, check=True, capture_output=True)
    # This intentionally leaves service.json and the login plist unchanged.


def renew_restored_epochs(data):
    """Offline restore of the existing v1 source format; business rows stay intact."""
    changed = []
    for path in sorted((data / 'chats').glob('*/.zork/source.json')):
        value = json.loads(path.read_text())
        if value.get('format') != 1 or value.get('chat_id') != path.parent.parent.name or not value.get('epoch'):
            raise RuntimeError('Unknown restored Chat source format; keep stopped: ' + str(path))
        value['epoch'] = uuid.uuid4().hex
        atomic_json(path, value)
        changed.append(value['chat_id'])
    database = data / 'state/station.sqlite'
    projection = None
    if database.exists():
        with closing(sqlite3.connect(database.as_uri() + '?mode=rw', uri=True)) as db:
            tables = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
            if 'sync_meta' in tables:
                db.execute('PRAGMA foreign_keys=ON')
                db.execute('PRAGMA synchronous=FULL')
                with db:
                    db.execute('UPDATE sync_meta SET epoch=? WHERE singleton=1', (uuid.uuid4().hex,))
                    db.execute('DELETE FROM sync_exports')
                row = db.execute('SELECT i.owner,m.epoch,m.sequence FROM sync_identity i,sync_meta m').fetchone()
                if row:
                    projection = dict(zip(('owner', 'epoch', 'sequence'), row))
                db.execute('PRAGMA wal_checkpoint(TRUNCATE)')
        if projection:
            atomic_json(data / 'state/sync-lineage.json', projection)
    return {'chat_sources': changed, 'projection_epoch': projection['epoch'] if projection else None}


def message_stamp(message):
    return hashlib.sha256(json.dumps({key: message.get(key) for key in ('id', 'role', 'content')},
                                     sort_keys=True).encode()).hexdigest()


class NodeRuntime:
    def __init__(self, data, binaries, log, profile=None, model=None, thinking='off', timeout=90):
        self.data, self.binaries, self.log = Path(data), Path(binaries), Path(log)
        self.profile, self.model, self.thinking, self.timeout = profile, model, thinking, timeout

    def processes(self):
        result = {}
        for line in subprocess.check_output(['ps', '-axo', 'pid=,command='], text=True).splitlines():
            fields = line.strip().split(None, 1)
            if len(fields) != 2:
                continue
            pid, command = int(fields[0]), fields[1]
            root_arg = '--data ' + str(self.data)
            at = command.find(root_arg)
            if at < 0 or (command[at + len(root_arg):] and not command[at + len(root_arg):].startswith(' ')):
                continue
            image = process_path(pid)
            if image and image in {p.resolve() for p in (self.binaries / 'zork', self.binaries / 'zork-station')}:
                result[pid] = str(image)
        return result

    def capture(self):
        service = self.data / 'service.json'
        result = {'running': bool(self.processes()), 'service_loaded': service_loaded(self.data),
                'service': json.loads(service.read_text()) if service.exists() else {},
                'images': {name: digest(self.binaries / name) for name in ('zork', 'zork-station')
                           if (self.binaries / name).exists()}}
        result['history_anchors'] = []
        if result['running']:
            try:
                result['origin'] = self.request('/v1/node/mesh')['origin']
                for path in sorted((self.data / 'chats').glob('*/.zork/source.json')):
                    chat = json.loads(path.read_text())['chat_id']
                    page = self.request('/v1/im/sessions/' + chat + '/messages?limit=1')
                    if page['items']:
                        message = page['items'][-1]
                        result['history_anchors'].append({'chat_id': chat, 'message_id': message['id'], 'sha256': message_stamp(message)})
            except OSError as error:
                # A broken dev runtime can still be replaced/restored from its
                # stopped full snapshot. Do not label this a healthy baseline.
                result['baseline_unavailable'] = str(error)
        return result

    def after_restore(self):
        return renew_restored_epochs(self.data)

    def verify_preserved(self, original):
        if original.get('origin') and self.request('/v1/node/mesh')['origin'] != original['origin']:
            raise RuntimeError('Deployment changed the existing Mesh identity')
        for anchor in original.get('history_anchors', []):
            page = self.request('/v1/im/sessions/' + anchor['chat_id'] + '/messages?limit=100')
            if not any(message['id'] == anchor['message_id'] and message_stamp(message) == anchor['sha256']
                       for message in page['items']):
                raise RuntimeError('Existing chat history no longer matches its pre-deployment anchor')
        return {'history_anchors_checked': len(original.get('history_anchors', []))}

    def stop(self):
        pause_service(self.data)
        try:
            status = control(self.data)
        except (OSError, ValueError):
            status = None
        if status is not None:
            image = process_path(status['pid'])
            if Path(status['data_root']).resolve() != self.data.resolve() or (
                    image is not None and image != (self.binaries / 'zork').resolve()):
                raise RuntimeError('Refusing to stop an unrelated supervisor')
            if image is not None:
                try:
                    control(self.data, 'stop')
                except OSError:
                    # The GUI may close its supervisor lease between the status
                    # reply and this request, unlinking the socket before exit.
                    # The exact owned images below still receive SIGTERM and
                    # must all disappear before a snapshot is permitted.
                    current = process_path(status['pid'])
                    if current is not None and current != image:
                        raise RuntimeError('Supervisor PID changed ownership during shutdown')
        # Covers an orphan Station left by an older launcher. No pkill or PID-file trust.
        for pid, image in self.processes().items():
            if process_path(pid) == Path(image):
                try:
                    os.kill(pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
        wait(lambda: not self.processes(), 'Owned node did not stop; no snapshot taken', self.timeout)

    def start(self, original, candidate=False):
        if not candidate and not original['running'] and not original['service_loaded']:
            return
        if self.processes():
            if candidate:
                raise RuntimeError('A node appeared during cutover; refusing to adopt an unverified process')
            self.health(original['images'], chat=False)
            return
        self.log.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        env = {k: v for k, v in os.environ.items() if not k.startswith('ZORK_')}
        env['ZORK_REGISTRY_DIR'] = str(self.data / 'registry')
        if original['service_loaded']:
            args = ['service', 'install', '--data', str(self.data)]
            if original['service'].get('start_at_login'):
                args.append('--at-login')
            subprocess.run([str(self.binaries / 'zork'), *args], env=env, check=True, capture_output=True)
        else:
            with self.log.open('ab') as output:
                subprocess.Popen([str(self.binaries / 'zork'), 'start', '--data', str(self.data)],
                                 env=env, stdin=subprocess.DEVNULL, stdout=output, stderr=output,
                                 start_new_session=True)

    def request(self, path, body=None, agent=False):
        config = json.loads((self.data / 'config.json').read_text())
        token = config.get('admin', {}).get('token')
        if not token:
            token = json.loads((self.data / 'run/node-token.json').read_text())
        address = config['bind']['agent' if agent else 'runtime']
        host, port = address.rsplit(':', 1)
        if host in ('0.0.0.0', '[::]'):
            host = '127.0.0.1'
        if host not in ('127.0.0.1', '[::1]', 'localhost'):
            raise RuntimeError('Deployment health accepts only this host\'s loopback API')
        request = Request('http://' + host + ':' + port + path,
                          data=None if body is None else json.dumps(body).encode(),
                          headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'})
        with urlopen(request, timeout=10) as response:
            return json.load(response)

    def ready(self, images):
        supervisor = control(self.data)
        station, agent = self.request('/readyz'), self.request('/readyz', agent=True)
        info = self.request('/v1/node/status')
        pid = int((self.data / 'run/zork-station.pid').read_text())
        if (supervisor.get('protocol') != 1 or supervisor.get('agent_mode') != 'embedded'
                or Path(supervisor['data_root']).resolve() != self.data.resolve()
                or info.get('protocol') != 1 or Path(info['data_root']).resolve() != self.data.resolve()
                or station.get('ok') is not True or agent.get('ok') is not True
                or agent.get('embedded') is not True or station.get('pid') != pid or agent.get('pid') != pid):
            raise RuntimeError('Supervisor, Station and embedded Agent identity/readiness disagree')
        return {'supervisor': running_image(supervisor['pid'], self.binaries / 'zork', images['zork']),
                'station': running_image(pid, self.binaries / 'zork-station', images['zork-station']),
                'origin': self.request('/v1/node/mesh')['origin']}

    def chat(self):
        if not self.profile or not self.model:
            raise RuntimeError('Chat acceptance requires an explicit health profile and model')
        workspace = self.data / 'health-workspace'
        workspace.mkdir(exist_ok=True)
        created = self.request('/v1/im/sessions', {'profile_id': self.profile, 'model': self.model,
            'thinking': self.thinking, 'workspace': str(workspace)})
        chat = created['session_id']
        nonce = 'zork-health-' + uuid.uuid4().hex
        route = '/v1/im/sessions/' + chat + '/messages'
        prompt = ('Deployment health check in this isolated health conversation. Use tool.help if needed, '
                  'then call chat.post_message to reply to chat ' + chat + ' with exactly: ' + nonce +
                  '. Do not run shell commands, inspect files, or send to any other chat.')
        sent = self.request(route, {'content': prompt, 'request_id': nonce})['message']

        def returned():
            page = self.request(route)['items']
            echo = next((m for m in page if m['id'] == sent['id'] and m.get('role') == 'user'), None)
            reply = next((m for m in page if m['id'] != sent['id'] and m.get('role') == 'assistant'
                          and m.get('content', '').strip().rstrip('.') == nonce), None)
            return {'chat_id': chat, 'request_id': sent['id'], 'reply_id': reply['id'], 'nonce': nonce,
                    'profile': self.profile, 'model': self.model} if echo and reply else None

        return wait(returned, 'Agent reply did not return to the authoritative chat', self.timeout)

    def health(self, images=None, chat=True, wait_for_ready=True):
        images = images or {name: digest(self.binaries / name) for name in ('zork', 'zork-station')}
        result = (wait(lambda: self.ready(images), 'Node failed identity/version/readiness acceptance', self.timeout)
                  if wait_for_ready else self.ready(images))
        if chat:
            result['chat'] = self.chat()
            # Recheck after the round trip; a supervisor restart must not hide a stale runtime.
            result.update(self.ready(images))
        return result

    def verify_original(self, original):
        if original['running'] or original['service_loaded']:
            result = self.health(original['images'], chat=bool(self.profile and self.model))
            result.update(self.verify_preserved(original))
            return result
        if self.processes():
            raise RuntimeError('Previously stopped node was unexpectedly started during rollback')
        return {'stopped': True}
