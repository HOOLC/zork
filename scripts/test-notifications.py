#!/usr/bin/env python3
"""Verify desktop notification preferences with an isolated client and no live node.

Build zork-gui first. The standalone binary intentionally cannot request macOS
notification permission; this test verifies that limitation is visible and safe.
"""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts/lib"))
from build_env import build_environment

spec = importlib.util.spec_from_file_location("notification_ui", ROOT / "scripts/lib/native_gui_fixture.py")
ui = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ui)


def main():
    env = build_environment()
    binary = Path(env.get("CARGO_TARGET_DIR", ROOT / "target")) / "debug/zork-gui"
    artifacts = ROOT / "artifacts/notifications"
    artifacts.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="zork-notifications-") as directory:
        root = Path(directory)
        client = root / "client"
        client.mkdir()
        with sqlite3.connect(client / "client.db") as db:
            db.executescript("CREATE TABLE nodes(id TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE cache(node TEXT,key TEXT,value TEXT,PRIMARY KEY(node,key));")
            db.execute("INSERT INTO cache VALUES(?,?,?)", ("device", "local-node-enabled", "false"))
        native = ui.Native(None, root)

        def preference():
            with sqlite3.connect(client / "client.db") as db:
                row = db.execute("SELECT value FROM cache WHERE node='device' AND key='notification-preferences-v1'").fetchone()
            return json.loads(row[0]) if row else {"enabled": True, "preview": False, "sound": True}

        def click(id):
            ui.wait(lambda: native.element(id, True), id)
            native.click(id)

        def launch(size):
            native.process = subprocess.Popen([str(binary), "--dev", "--dev-port", native.url.rsplit(":", 1)[1], "--dev-token", "mesh-native-fixture"],
                env=dict(env, ZORK_CLIENT_DATA=str(client), ZORK_GUI_LOCALE="zh-CN", ZORK_GUI_PREFERENCES_PATH=str(root / "preferences.json"), ZORK_GUI_TEST_WINDOW_SIZE=size),
                stdout=native.log, stderr=native.log)
            ui.wait(lambda: native.ui("/health"), "notification fixture")
            ui.wait(lambda: native.element("client_notifications") or native.element("desktop-manage"), "settings navigation")
            if not native.element("client_notifications"):
                click("desktop-manage")
            click("client_notifications")
            ui.wait(lambda: native.element("notifications-toggle"), "notification settings")

        try:
            launch("1280x800")
            assert preference() == {"enabled": True, "preview": False, "sound": True}
            for id in ("notifications-toggle", "notifications-preview", "notifications-sound"):
                assert native.element(id)["role"] == "option", f"{id} must use the shared Switch"
            click("notifications-preview")
            ui.wait(lambda: preference()["preview"], "preview preference")
            click("notifications-sound")
            ui.wait(lambda: not preference()["sound"], "sound preference")
            click("notifications-toggle")
            ui.wait(lambda: not preference()["enabled"], "disabled preference")
            ui.wait(lambda: native.element("notifications-test") and not native.element("notifications-test")["enabled"], "test disabled with notifications")
            click("notifications-toggle")
            click("notifications-test")
            ui.wait(lambda: native.element("notifications-test", True), "test completion")
            if sys.platform == "darwin":
                ui.wait(lambda: "直接运行开发二进制" in (native.element("notifications-permission-status") or {}).get("label", ""), "standalone binary permission feedback")
            native.screenshot(artifacts / "settings-1280x800.png")
            (artifacts / "settings-1280x800.json").write_bytes(native.ui("/v1/elements"))
            native.stop()
            # An unreachable fixture node exercises the selected-conversation mute
            # control without connecting to any real Station or user data.
            with sqlite3.connect(client / "client.db") as db:
                node = {"id": "notification-fixture", "name": "通知测试设备", "url": "http://127.0.0.1:9", "token": None, "local": False}
                db.execute("INSERT INTO nodes VALUES(?,?)", (node["id"], json.dumps(node)))
                for owner, key, value in [
                    ("device", "last-node", node["id"]),
                    (node["id"], "last-session", "notification-session"),
                    (node["id"], "agents", [{"id": "leader", "role": "leader", "name": "测试领队", "session_id": "notification-session"}]),
                    (node["id"], "sessions", [{"kind": "agent", "session_id": "notification-session", "profile_id": "fixture", "model": "fixture", "thinking": "off", "workspace": "", "status": "wait"}]),
                ]:
                    db.execute("INSERT OR REPLACE INTO cache VALUES(?,?,?)", (owner, key, json.dumps(value)))
            launch("900x600")
            assert preference()["enabled"] and preference()["preview"] and not preference()["sound"], "preferences lost across restart"
            assert native.element("notification-mute-current")["role"] == "option"
            click("notification-mute-current")
            ui.wait(lambda: ["notification-fixture", "notification-session"] in preference()["muted"], "mute selected offline conversation")
            click("notification-mute-current")
            ui.wait(lambda: not preference()["muted"], "unmute selected offline conversation")
            click("notifications-preview")
            ui.wait(lambda: not preference()["preview"], "private preview restored")
            native.screenshot(artifacts / "settings-900x600.png")
            (artifacts / "settings-900x600.json").write_bytes(native.ui("/v1/elements"))
            (artifacts / "report.json").write_text(json.dumps({"passed": True, "checks": ["default privacy", "toggle persistence", "disabled test action", "standalone permission feedback", "restart", "offline conversation mute/unmute", "compact layout"], "native_os_delivery": "not exercised by this standalone binary test"}, indent=2) + "\n")
            print("Notification settings passed: persistence, controls, permission feedback, restart, two sizes.")
        finally:
            native.stop()
            native.log.close()
            shutil.copy2(root / "gui.log", artifacts / "gui.log")


if __name__ == "__main__":
    main()
