#!/usr/bin/env python3
"""Install a prepared macOS client on a destination host through SSH stdin."""
import hashlib
import json
import os
import signal
from contextlib import closing
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import time
from urllib.request import urlopen


def run(argv):
    subprocess.run(argv, check=True)


def register_app(app):
    # Refresh nested Launch Services records explicitly after replacement,
    # before the GUI can launch its browser through the final installed paths.
    tool = '/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister'
    bundles = [app, *sorted(app.rglob('*.app'))]
    run([tool, '-f', *(str(bundle.resolve()) for bundle in bundles)])


def processes(app):
    prefix = str(app / 'Contents') + '/'
    return [line for line in subprocess.check_output(['ps', '-axo', 'command='], text=True).splitlines()
            if line.startswith(prefix)]


def wait_for(check, description, seconds=30):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if check():
            return
        time.sleep(0.25)
    raise RuntimeError(description)


def quit_app(app):
    if processes(app):
        run(['osascript', '-e', 'tell application id "ing.zork.desktop" to quit'])
        try:
            wait_for(lambda: not processes(app), 'Zork 未完成退出；未替换应用', seconds=5)
        except RuntimeError:
            # Some GUI builds keep running after the Apple event. Terminate
            # only this installed GUI; its supervisor then shuts down normally.
            gui = str(app / 'Contents/MacOS/zork-gui')
            for line in subprocess.check_output(['ps', '-axo', 'pid=,command='], text=True).splitlines():
                fields = line.strip().split(None, 1)
                if len(fields) == 2 and (fields[1] == gui or fields[1].startswith(gui + ' ')):
                    try:
                        os.kill(int(fields[0]), signal.SIGTERM)
                    except ProcessLookupError:
                        pass
            wait_for(lambda: not processes(app), 'Zork 未完成退出；未替换应用')


def verify(app, expect_node):
    gui = str(app / 'Contents/MacOS/zork-gui')
    wait_for(lambda: any(line == gui or line.startswith(gui + ' ') for line in processes(app)), '新版界面未启动')
    time.sleep(1)
    if not any(line == gui or line.startswith(gui + ' ') for line in processes(app)):
        raise RuntimeError('新版界面启动后退出')
    if expect_node:
        def healthy():
            try:
                config = json.loads((Path.home() / 'Library/Application Support/Zork/client/node/config.json').read_text())
                bind = config['bind']['runtime']
                host, port = bind.rsplit(':', 1)
                if host in ('0.0.0.0', '[::]'):
                    bind = '127.0.0.1:' + port
                with urlopen('http://' + bind + '/readyz', timeout=1) as response:
                    return response.status == 200
            except (OSError, ValueError, KeyError):
                return False
        wait_for(healthy, '本机节点未恢复就绪', seconds=90)


def local_node_enabled(database, fallback):
    if not database.exists():
        return fallback
    # A running client may be closing/reopening its WAL sidecars. Retry the
    # read-only preflight, and release the connection before quitting the app.
    for attempt in range(3):
        try:
            with closing(sqlite3.connect(database.as_uri() + '?mode=ro', uri=True)) as db:
                row = db.execute("SELECT value FROM cache WHERE node='device' AND key='local-node-enabled'").fetchone()
                return json.loads(row[0]) if row else any(
                    json.loads(value).get('local') for (value,) in db.execute('SELECT value FROM nodes'))
        except sqlite3.OperationalError:
            if attempt == 2:
                raise
            time.sleep(0.25)


def main():
    stage = Path(sys.argv[1])
    archive = stage / 'app.tar.gz'
    if hashlib.sha256(archive.read_bytes()).hexdigest() != sys.argv[2]:
        raise RuntimeError('安装包校验失败；未修改应用')
    run(['tar', '-xzf', str(archive), '-C', str(stage)])
    incoming = stage / 'Zork.app'
    run(['codesign', '--verify', '--deep', '--strict', str(incoming)])
    app = Path.home() / 'Applications/Zork.app'
    app.parent.mkdir(parents=True, exist_ok=True)
    running = processes(app)
    expect_node = any(
        executable in line
        for line in running
        for executable in ('/zork-station ', '/zork-gateway ')
    )
    database = Path.home() / 'Library/Application Support/Zork/client/client.db'
    expect_node = local_node_enabled(database, expect_node)
    quit_app(app)
    previous_app = stage / 'previous.app'
    if previous_app.exists():
        raise RuntimeError('安装暂存目录已有旧应用；未替换应用')
    previous = app.exists()
    if previous:
        app.rename(previous_app)
    try:
        shutil.move(str(incoming), str(app))
        run(['codesign', '--verify', '--deep', '--strict', str(app)])
        register_app(app)
        run(['open', str(app)])
        verify(app, expect_node)
    except Exception:
        # Restore the bundle only; client data and configuration are never copied or reset.
        quit_app(app)
        if app.exists():
            shutil.move(str(app), str(stage / 'failed.app'))
        if previous:
            previous_app.rename(app)
            register_app(app)
            if running:
                run(['open', str(app)])
        raise
    if previous:
        shutil.rmtree(previous_app)
    print('MBA 已更新并启动' + ('，本机节点就绪。' if expect_node else '。'), flush=True)
    print('旧版应用已清理，未保留历史备份。', flush=True)


if __name__ == '__main__':
    main()
