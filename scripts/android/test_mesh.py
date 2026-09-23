#!/usr/bin/env python3
"""Android JNI -> embedded Synch -> isolated real Station/fake-model Agent.

Requires an already running arm64 emulator (default emulator-5554), debug APK
and instrumentation APK. No user node, model account or workspace is accessed.
"""
from test_apks import require_test_apks
import argparse
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[2]
PACKAGE = "ing.zork.android.test"
RUNNER = PACKAGE + ".test/androidx.test.runner.AndroidJUnitRunner"
CLASS = "ing.zork.android.MeshIntegrationTest"


def main():
    os.environ['ZORK_CHANNEL'] = 'test'
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", default="emulator-5554")
    parser.add_argument("--host-ip", help="Development host LAN IP for a physical Android device")
    parser.add_argument("--keep-node", action="store_true", help="keep fixture node for manual UI validation")
    parser.add_argument("--history-only", action="store_true", help="run the JNI session history/detail and foreground recovery check after fixture setup")
    parser.add_argument("--apk", type=Path, default=ROOT / "apps/android/app/build/outputs/apk/debug/app-debug.apk")
    parser.add_argument("--test-apk", type=Path, default=ROOT / "apps/android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk")
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/android")
    args = parser.parse_args()
    require_test_apks(args.apk, args.test_apk)
    sdk = Path(os.environ.get("ANDROID_HOME", Path.home() / "Library/Android/sdk"))
    adb = [str(sdk / "platform-tools/adb"), "-s", args.serial]
    artifacts = args.output
    artifacts.mkdir(parents=True, exist_ok=True)

    def device(*command, **kwargs):
        return subprocess.check_output([*adb, *command], **kwargs)

    for apk in [args.apk, args.test_apk]:
        print(device("install", "-r", str(apk.resolve()), text=True), flush=True)

    def instrument(method, **values):
        device("shell", "am", "force-stop", PACKAGE)
        command = [*adb, "shell", "am", "instrument", "-w", "-r", "-e", "class", f"{CLASS}#{method}"]
        for key, value in values.items():
            command += ["-e", key, value]
        result = subprocess.run([*command, RUNNER], text=True, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, timeout=180)
        (artifacts / f"{method}.log").write_text(result.stdout)
        print(result.stdout, flush=True)
        assert result.returncode == 0 and "OK (1 test)" in result.stdout, f"Android test failed: {method}"

    instrument("bootstrap")
    identity = json.loads(device("exec-out", "run-as", PACKAGE, "cat", "files/android-mesh-lab.json"))["identity"]
    spec = importlib.util.spec_from_file_location("mesh_fixture", ROOT / "scripts/test-mesh.py")
    fixture = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(fixture)
    root = Path(tempfile.mkdtemp(prefix="zork-android-", dir="/tmp"))
    node = fixture.Node(root / "node")
    token = "isolated-android-fixture"
    node.config["admin"] = {"token": token}
    if args.host_ip: node.config["mesh"]["bind"] = "0.0.0.0:" + str(node.udp)
    node.config["mesh"]["peers"] = [{"origin": identity, "name": "Android test client", "execute": [], "client": True}]
    (node.root / "config.json").write_text(json.dumps(node.config))

    def admin(method, path, body=None):
        request = Request(node.url + path,
                          data=None if body is None else json.dumps(body).encode(), method=method,
                          headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"})
        with urlopen(request, timeout=15) as response:
            return json.load(response)

    keep = False
    try:
        node.start()
        fixture.wait(lambda: node.request("GET", "/readyz")[0] == 200, "Station ready")
        fixture.wait(lambda: urlopen(node.agent_url + "/readyz", timeout=2).status == 200, "Agent ready")
        admin("POST", "/v1/node/agents", {"id":"android-leader", "name":"Android Leader", "role":"leader",
              "avatar":"panda", "profile_id":"fixture", "model":"fixture-model", "thinking":"off"})
        # Session creation is an admin operation; the Android client only reads
        # this fixture through its normal, unchanged Mesh client grant.
        history_session = admin("POST", "/v1/im/sessions", {"profile_id":"fixture", "model":"fixture-model",
            "thinking":"off", "workspace":str(node.workspace)})["session_id"]
        admin("POST", f"/v1/im/sessions/{history_session}/messages", {"content":json.dumps({
            "fake_tools":[{"name":"shell.run", "input":{"command":"pwd"}}]}), "request_id":"android-history-fixture"})
        instrument("prepareExecutionHistoryFixture", origin=node.origin, address=f"{args.host_ip or '10.0.2.2'}:{node.udp}", session=history_session)
        instrument("sessionExecutionHistoryUsesNativeObservation")
        if args.history_only:
            (artifacts / "session-history-native.json").write_bytes(device("exec-out", "run-as", PACKAGE, "cat", "files/session-history-native.json"))
            print("PASS: Android JNI execution history and foreground recovery", flush=True)
            keep = args.keep_node
            return
        instrument("exchangeAndQueueBeforeProcessDeath", origin=node.origin, address=f"{args.host_ip or '10.0.2.2'}:{node.udp}")
        instrument("settingsManageDevice")
        instrument("restoreInNewProcess")
        instrument("sharedStateOwnsDeliveryAndProjection")
        instrument("commentsAndTextFilesUseSharedDraftAndDelivery")
        instrument("localEditsDuringStalledConnection")
        node.config["mesh"]["peers"][0]["client"] = False
        (node.root / "config.json").write_text(json.dumps(node.config))
        # The config watcher normally converges within a second. Query the
        # authenticated node config instead of guessing a timing delay.
        fixture.wait(lambda: admin("GET", "/v1/node/mesh")["config"]["peers"][0]["client"] is False, "client revoked")
        instrument("revocationRejectsClient")
        node.config["mesh"]["peers"][0]["client"] = True
        (node.root / "config.json").write_text(json.dumps(node.config))
        summary = {"fixture":str(root), "node_origin":node.origin, "node_udp":node.udp,
                   "supervisor_pid":node.process.pid, "serial":args.serial,
                   "checks":["JNI and Android TLS initialization", "real QUIC Station requests", "live subscription",
                             "stable-ID deduplication", "shared Rust delivery and projection", "offline cache", "process restart and pending send",
                             "local drafts while a real network request is stalled", "revocation"]}
        (artifacts / "mesh-result.json").write_text(json.dumps(summary, indent=2) + "\n")
        print("PASS: Android embedded Mesh integration", flush=True)
        keep = args.keep_node
    finally:
        if not keep:
            node.stop()
        print(f"isolated fixture: {root}", flush=True)


if __name__ == "__main__":
    main()
