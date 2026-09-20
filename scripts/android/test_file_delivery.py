#!/usr/bin/env python3
"""Two real Stations -> Android core/JNI -> production Chat/shared-file UI.

Use freshly rebuilt binaries and APKs on a task-owned emulator. The fake model
executes the real chat.post_file tool; source files are outside its workspace.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import zlib

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("channels", ROOT / "scripts/test-chat-channels.py")
channels = importlib.util.module_from_spec(spec)
spec.loader.exec_module(channels)
fixture = channels.fixture
PACKAGE = "ing.zork.android.debug"
CLASS = "ing.zork.android.ChatFileDeliveryTest"


def png():
    def chunk(kind, value):
        return struct.pack(">I", len(value)) + kind + value + struct.pack(">I", zlib.crc32(kind + value))
    rows = bytearray()
    for y in range(64):
        rows.append(0)
        for x in range(96):
            rows.extend(((238, 76, 64), (46, 174, 107), (62, 113, 219), (241, 195, 63))[(y >= 32) * 2 + (x >= 48)])
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 96, 64, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")


class Node(fixture.Node):
    def start(self):
        self.log = (self.root / "station.log").open("ab")
        self.process = subprocess.Popen([str(fixture.TARGET / "zork-station"), "--data", str(self.root), "--fake-agent"],
                                        stdout=self.log, stderr=self.log, start_new_session=True)


def tool(node, chat, session, name, args):
    before = channels.ok(node, "GET", f"/sessions/{session}/history?limit=200", agent=True)
    seen = {item["event_id"] for item in before["items"]}
    channels.ok(node, "POST", f"/v1/im/sessions/{chat}/messages", {
        "request_id": "delivery-" + os.urandom(12).hex(),
        "content": json.dumps({"fake_tool": {"name": name, "input": args}})})
    def result():
        history = channels.ok(node, "GET", f"/sessions/{session}/history?limit=200", agent=True)
        return next((item["event"]["result"] for item in history["items"] if item["event_id"] not in seen
                     and item["event"].get("kind") == "tool_result" and item["event"]["result"]["tool"] == name), None)
    outcome = fixture.wait(result, name)
    assert outcome["outcome"] == "succeeded", outcome
    return outcome["data"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--apk", type=Path, default=ROOT / "apps/android/app/build/outputs/apk/debug/app-debug.apk")
    parser.add_argument("--test-apk", type=Path, default=ROOT / "apps/android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk")
    args = parser.parse_args()
    assert args.serial.startswith("emulator-"), "requires a task-owned emulator"
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    adb = ["adb", "-s", args.serial]
    hashes = {}
    for source in (args.apk, args.test_apk):
        fixed = output / source.name
        if source.resolve() != fixed:
            shutil.copy2(source, fixed)
        hashes[fixed.name] = hashlib.sha256(fixed.read_bytes()).hexdigest()
        subprocess.run([*adb, "install", "-r", str(fixed)], check=True)
    hashes["station"] = hashlib.sha256((fixture.TARGET / "zork-station").read_bytes()).hexdigest()
    (output / "sha256.json").write_text(json.dumps(hashes, indent=2) + "\n")

    def instrument(method, **values):
        subprocess.run([*adb, "shell", "am", "force-stop", PACKAGE], check=True)
        # Keep failure evidence tied to this run, not an earlier failed fixture.
        subprocess.run([*adb, "shell", "run-as", PACKAGE, "rm", "-f",
                        "files/file-delivery-failure.png"], check=True)
        (output / "file-delivery-failure.png").unlink(missing_ok=True)
        command = [*adb, "shell", "am", "instrument", "-w", "-r", "-e", "class", CLASS + "#" + method,
                   "-e", "file_fixture", "true"]
        for key, value in values.items():
            command += ["-e", key, str(value)]
        result = subprocess.run([*command, PACKAGE + ".test/androidx.test.runner.AndroidJUnitRunner"],
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=240)
        (output / (method + ".log")).write_text(result.stdout)
        print(result.stdout, flush=True)
        assert result.returncode == 0 and "OK (1 test)" in result.stdout, method

    instrument("bootstrap")
    identity = json.loads(subprocess.check_output([*adb, "exec-out", "run-as", PACKAGE, "cat", "files/file-delivery-identity.json"]))["identity"]
    nodes = []
    with tempfile.TemporaryDirectory(prefix="zork-delivery-") as directory:
        root = Path(directory)
        previous = os.environ.get("ZORK_REGISTRY_DIR")
        os.environ["ZORK_REGISTRY_DIR"] = str(root / "registry")
        try:
            a = Node(root / "reader"); nodes.append(a)
            b = Node(root / "writer"); nodes.append(b)
            for node, other in ((a, b), (b, a)):
                node.pair(other)
                node.config["admin"] = {"token": channels.TOKEN}
                node.config["mesh"]["peers"][0].update(client=True, collaborate=True)
                node.config["mesh"]["peers"].append({"origin": identity, "name": "Android fixture", "client": True, "execute": []})
                (node.root / "config.json").write_text(json.dumps(node.config))
                channels.start(node)
            reader, chat = channels.make_caller(a, "reader")
            writer, writer_chat = channels.make_caller(b, "writer")
            image = png()
            binary = bytes(i % 251 for i in range(410_000))
            image_path, binary_path = root / "delivery.png", root / "payload.bin"
            image_path.write_bytes(image); binary_path.write_bytes(binary)
            # Only this explicit copy appears in shared files.
            (a.root / "shared/visible.png").write_bytes(image)
            # Force a cold MPT request past one UDP packet. Small requests can
            # hide Android segmentation-offload failures even when Chat works.
            metadata = a.root / "shared/metadata" / ("long-prefix-" + "x" * 120)
            metadata.mkdir(parents=True)
            for index in range(32):
                (metadata / f"{index:03}.txt").write_text(f"explicit shared fixture {index}\n")
            sent = tool(b, writer_chat, writer, "chat.post_file", {"target": a.origin, "chat_id": chat,
                "text": "跨节点文件交付", "attachments": [{"file_path": str(image_path)}, {"file_path": str(binary_path)}]})
            image_path.unlink(); binary_path.unlink()
            assert str(root) not in json.dumps(sent), "local source paths escaped into the public message"
            files = {file["name"]: file for file in sent["attachments"]}
            # Continue the same Session after a real Station restart.
            b.stop(); channels.start(b)
            tool(b, writer_chat, writer, "chat.post_message", {"target": a.origin, "chat_id": chat, "text": "Restarted Session uses the split tool table"})
            # The old input path has gone, so re-delivery must use frozen bytes.
            copied = tool(b, writer_chat, writer, "chat.post_file", {"chat_id": writer_chat,
                "attachments": [{"source_target": a.origin, "source_chat_id": chat, "attachment_id": files["payload.bin"]["id"]}]})
            assert copied["attachments"][0]["content_root"] == files["payload.bin"]["content_root"]
            instrument("realSharedFilesAndChatImagesUseCoreAndNativeExport", origin=a.origin, address=f"10.0.2.2:{a.udp}",
                       chat=chat, message=sent["message_id"], binary=files["payload.bin"]["id"],
                       image_sha=hashlib.sha256(image).hexdigest(), binary_sha=hashlib.sha256(binary).hexdigest())
            for name in ("chat.png", "chat-image.png", "shared-image.png", "result.json"):
                (output / name).write_bytes(subprocess.check_output([*adb, "exec-out", "run-as", PACKAGE, "cat", "files/file-delivery-" + name]))
            (output / "station-result.json").write_text(json.dumps({"topology": "two isolated Stations on one host plus Android emulator",
                "outside_workspace_absolute_path": True, "source_deleted_after_send": True, "same_session_after_restart": True,
                "cross_node_resend_fixed_root": True, "long_path_metadata_files": 32, "sha256": hashes}, indent=2) + "\n")
            print("PASS: two Stations, absolute-path post_file, restart, Android image UI and exact binary export", flush=True)
        finally:
            for node in nodes:
                node.stop()
                log = node.root / "station.log"
                if log.exists(): shutil.copy2(log, output / (node.root.name + ".log"))
            subprocess.run([*adb, "shell", "am", "force-stop", PACKAGE], check=True)
            for source in ("no_backup/file-delivery-client/transport-debug.log",
                           "no_backup/file-delivery-client/connection-errors.jsonl",
                           "files/file-delivery-failure.png"):
                exists = subprocess.run([*adb, "shell", "run-as", PACKAGE, "test", "-f", source],
                                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                if exists.returncode != 0:
                    continue
                evidence = subprocess.run([*adb, "exec-out", "run-as", PACKAGE, "cat", source],
                                          stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                if evidence.returncode == 0 and (not source.endswith(".png") or evidence.stdout.startswith(b"\x89PNG\r\n\x1a\n")):
                    (output / Path(source).name).write_bytes(evidence.stdout)
            if previous is None: os.environ.pop("ZORK_REGISTRY_DIR", None)
            else: os.environ["ZORK_REGISTRY_DIR"] = previous


if __name__ == "__main__":
    main()
