#!/usr/bin/env python3
"""Exercise the desktop clear-data button against disposable local app data."""
import argparse
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import subprocess
import tempfile
import time
import urllib.error
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    args.output.mkdir(parents=True, exist_ok=True)
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    token = f"data-reset-fixture-{os.getpid()}"
    base = f"http://127.0.0.1:{port}"

    def request(path, value=None):
        req = urllib.request.Request(base + path, headers={"Authorization": "Bearer " + token,
            "Content-Type": "application/json"}, data=None if value is None else json.dumps(value).encode())
        with urllib.request.urlopen(req, timeout=4) as response:
            data = response.read()
            return data if path == "/v1/screenshot" else json.loads(data)

    def wait(check, message):
        end = time.monotonic() + 35
        while time.monotonic() < end:
            try:
                if check():
                    return
            except (OSError, ValueError):
                pass
            time.sleep(.1)
        raise RuntimeError(message)

    def click(identity):
        request("/v1/actions", {"type": "click", "target": {"element_id": identity}})

    with tempfile.TemporaryDirectory(prefix="zork-client-reset-") as directory:
        root = Path(directory) / "client"
        profile = root / "node/profiles/old.json"
        profile.parent.mkdir(parents=True)
        profile.write_text('{"fixture":"old configuration"}')
        (root / "old-message").write_text("old local data")
        outside = Path(directory) / "unrelated"
        outside.write_text("keep")
        env = dict(os.environ, ZORK_CLIENT_DATA=str(root), ZORK_GUI_PREFERENCES_PATH=str(root / "preferences.json"),
            ZORK_GUI_TEST_WINDOW_SIZE="1000x720", ZORK_GUI_TEST_DRIVE_FRAMES="1", SEED="0")
        with (args.output / "desktop-reset.log").open("wb") as log:
            process = subprocess.Popen([str(binary), "--dev", "--dev-port", str(port), "--dev-token", token],
                env=env, stdout=log, stderr=log, start_new_session=True)
            try:
                wait(lambda: request("/health")["status"] == "ok", "client did not start")
                wait(lambda: any(e["id"] == "client_data" for e in request("/v1/elements")["elements"]), "data settings not available")
                click("client_data")
                wait(lambda: any(e["id"] == "clear-client-data" for e in request("/v1/elements")["elements"]), "reset button not available")
                click("clear-client-data")
                wait(lambda: any(e["id"] == "clear-client-data-dialog-cancel" for e in request("/v1/elements")["elements"]), "confirmation not shown")
                click("clear-client-data-dialog-cancel")
                assert profile.exists(), "cancel removed data"
                click("clear-client-data")
                wait(lambda: any(e["id"] == "clear-client-data-dialog-confirm" for e in request("/v1/elements")["elements"]), "confirmation not shown")
                (args.output / "desktop-reset-confirmation.png").write_bytes(request("/v1/screenshot"))
                try:
                    click("clear-client-data-dialog-confirm")
                except (OSError, urllib.error.URLError):
                    pass  # The old process may close its dev socket while replying.
                process.wait(timeout=35)
                assert process.returncode == 0, "client did not quit normally"
                wait(lambda: not profile.exists() and request("/health")["status"] == "ok", "client did not restart with cleared data")
                wait(lambda: (root / "client.db").is_file(), "fresh store not created")
                with sqlite3.connect((root / "client.db").as_uri() + "?mode=ro", uri=True) as db:
                    assert db.execute("SELECT COUNT(*) FROM nodes").fetchone()[0] == 0
                    assert db.execute("SELECT COUNT(*) FROM messages").fetchone()[0] == 0
                assert not (root / "old-message").exists()
                assert not (root / "node/config.json").exists(), "local node was re-enabled"
                assert outside.read_text() == "keep"
                (args.output / "desktop-reset-fresh.png").write_bytes(request("/v1/screenshot"))
                print(json.dumps({"cancel_preserved_data": True, "old_process_exited": True,
                    "restarted_empty": True, "outside_data_preserved": True}))
            finally:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                process.wait(timeout=10)
                time.sleep(.5)


if __name__ == "__main__":
    main()
