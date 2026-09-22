"""macOS application lifecycle adapter for the shared recovery transaction."""
from contextlib import closing
import json
import os
from pathlib import Path
import plistlib
import signal
import sqlite3
import subprocess

from deployment import digest
from deployment_health import NodeRuntime, process_path, running_image, wait, pause_service

LSREGISTER = '/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister'


def validate_app(app, channel, prefix=None):
    subprocess.run(['codesign', '--verify', '--deep', '--strict', str(app)], check=True, capture_output=True)
    info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
    prefix = prefix or ('ing.zork-dev' if channel == 'dev' else 'ing.zork')
    if (info.get('CFBundleIdentifier') != prefix + '.desktop'
            or info.get('CFBundleName') != ('Zork Dev' if channel == 'dev' else 'Zork')
            or info.get('CFBundleExecutable') != 'zork-gui'
            or info.get('LSEnvironment')
            or (app / 'Contents/Resources/channel').read_text().strip() != channel):
        raise RuntimeError('App identity or data channel differs from the requested installation')
    for path in (app / 'Contents/Helpers').rglob('Info.plist'):
        if path.parent.name != 'Contents' or path.parent.parent.suffix != '.app':
            continue
        helper = plistlib.loads(path.read_bytes()).get('CFBundleIdentifier')
        if helper and not helper.startswith(prefix + '.'):
            raise RuntimeError('Nested helper belongs to another release/dev identity: ' + helper)
    signature = subprocess.run(['codesign', '-dvv', str(app / 'Contents/MacOS/zork-gui')],
                               check=True, capture_output=True, text=True).stderr
    if ('Identifier=' + info['CFBundleIdentifier']) not in signature.splitlines() or 'Info.plist=not bound' in signature:
        raise RuntimeError('GUI signature is not bound to the bundle identity')


def node_enabled(client, fallback=False):
    path = client / 'client.db'
    if not path.exists():
        return fallback
    with closing(sqlite3.connect(path.as_uri() + '?mode=ro', uri=True)) as db:
        row = db.execute("SELECT value FROM cache WHERE node='device' AND key='local-node-enabled'").fetchone()
        return json.loads(row[0]) if row else fallback


class AppRuntime:
    def __init__(self, settings, log):
        self.settings = settings
        self.app, self.data, self.log = Path(settings['payload']), Path(settings['data']), Path(log)
        self.node = NodeRuntime(self.data / 'node', self.app / 'Contents/MacOS', log.with_name('node.log'),
                                **settings.get('health', {}))
        self.timeout = self.node.timeout

    def processes(self):
        result = {}
        for line in subprocess.check_output(['ps', '-axo', 'pid=,command='], text=True).splitlines():
            fields = line.strip().split(None, 1)
            if len(fields) != 2 or not fields[1].startswith(str(self.app / 'Contents') + '/'):
                continue
            pid = int(fields[0])
            image = process_path(pid)
            if image and image.is_relative_to(self.app / 'Contents'):
                result[pid] = str(image)
        return result

    def capture(self):
        node = self.node.capture()
        gui = self.app / 'Contents/MacOS/zork-gui'
        return {'running': bool(self.processes()), 'node': node,
                'node_enabled': node_enabled(self.data, node['running']),
                'gui_sha256': digest(gui) if gui.exists() else None}

    def stop(self):
        # Unload only this data root's watcher without rewriting its settings.
        pause_service(self.node.data)
        gui = (self.app / 'Contents/MacOS/zork-gui').resolve()
        for pid, image in self.processes().items():
            if Path(image) == gui and process_path(pid) == gui:
                try:
                    os.kill(pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
        wait(lambda: gui not in (Path(image) for image in self.processes().values()),
             'Installed GUI did not close; no snapshot taken', self.timeout)
        self.node.stop()
        for pid, image in self.processes().items():
            if process_path(pid) == Path(image):
                try:
                    os.kill(pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
        wait(lambda: not self.processes(), 'Installed app did not stop; left unchanged', self.timeout)

    def start(self, original, candidate=False):
        if not candidate and not original['running'] and not original['node']['running'] and not original['node']['service_loaded']:
            return
        if candidate:
            validate_app(self.app, self.settings['channel'], self.settings.get('id_prefix'))
        if original['node']['service_loaded'] or (not candidate and original['node']['running']):
            self.node.start(original['node'], candidate=candidate)
        if candidate or original['running']:
            subprocess.run([LSREGISTER, '-f', str(self.app),
                            *(str(p) for p in sorted(self.app.rglob('*.app')))], check=True, capture_output=True)
            self.log.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            env = {key: value for key, value in os.environ.items() if not key.startswith('ZORK_')}
            env.update(ZORK_CLIENT_DATA=str(self.data),
                       ZORK_GUI_PREFERENCES_PATH=self.settings.get('preferences', str(self.data / 'preferences.json')),
                       ZORK_REGISTRY_DIR=str(self.data / 'nodes'))
            # The signed native main executable keeps its bundle identity. Explicit
            # paths also support isolated installation drills without global HOME changes.
            with self.log.open('ab') as output:
                subprocess.Popen([str(self.app / 'Contents/MacOS/zork-gui')], env=env,
                                 stdin=subprocess.DEVNULL, stdout=output, stderr=output, start_new_session=True)

    def gui(self, expected=None, wait_for_ready=True):
        gui = (self.app / 'Contents/MacOS/zork-gui').resolve()
        expected = expected or digest(gui)
        def check():
            pids = [pid for pid, image in self.processes().items() if Path(image) == gui]
            if len(pids) != 1:
                return False
            return running_image(pids[0], gui, expected)
        if wait_for_ready:
            return wait(check, 'GUI did not adopt the accepted bundle', self.timeout)
        result = check()
        if not result:
            raise RuntimeError('Expected one running GUI')
        return result

    def health(self, chat=True, wait_for_ready=True):
        result = {'gui': self.gui(wait_for_ready=wait_for_ready)}
        enabled = node_enabled(self.data, bool(self.node.processes()))
        if chat and not enabled:
            raise RuntimeError('Full chat acceptance requires the configured local node to be enabled')
        if enabled:
            result['node'] = self.node.health(chat=chat, wait_for_ready=wait_for_ready)
        result['gui'] = self.gui(result['gui']['sha256'], wait_for_ready=wait_for_ready)
        return result

    def after_restore(self):
        return self.node.after_restore()

    def verify_preserved(self, original):
        return self.node.verify_preserved(original['node'])

    def verify_original(self, original):
        if not original['running'] and not original['node']['running'] and not original['node']['service_loaded']:
            if self.processes():
                raise RuntimeError('Previously stopped app started during rollback')
            return {'stopped': True}
        evidence = {}
        if original['running']:
            evidence['gui'] = self.gui(original['gui_sha256'])
        if original['node']['running'] or original['node']['service_loaded']:
            evidence['node'] = self.node.verify_original(original['node'])
        return evidence
