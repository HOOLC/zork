#!/usr/bin/env python3
"""Client identity and browser routing across real isolated Mesh nodes.

The browser worker is a protocol fixture; this checks routing and receipts, not
CEF rendering. Rebuild Station before running.
"""
import importlib.util
import json
import os
from pathlib import Path
import queue
import tempfile
import threading
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("channels", ROOT / "scripts/test-chat-channels.py")
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)
f = c.fixture
REPORT = Path(os.environ.get("ZORK_TEST_ARTIFACT_DIR", ROOT / "artifacts/client-browser"))


def main():
    REPORT.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="zork-client-browser-") as directory:
        root = Path(directory)
        os.environ["ZORK_REGISTRY_DIR"] = str(root / "registry")
        nodes, streams, threads = [], [], []
        try:
            a, b = f.Node(root / "client-owner"), f.Node(root / "executor")
            nodes = [a, b]
            for node, other in ((a, b), (b, a)):
                node.pair(other)
                node.config["admin"] = {"token": c.TOKEN}
                node.config["mesh"]["peers"][0].update(client=False, collaborate=False, execute=[])
                (node.root / "config.json").write_text(json.dumps(node.config))
                c.start(node)
            caller_a, home_a = c.make_caller(a, "owner")
            caller_b, _ = c.make_caller(b, "executor")
            client = "01ARZ3NDEKTSV4RRFFQ69G5FAV"
            c.ok(a, "POST", f"/v1/im/sessions/{home_a}/messages", {
                "content": "Please use my browser", "request_id": "identity", "client_id": client})
            message = next(m for m in c.operation(a, caller_a, "chat.history", {"chat_id": home_a})["items"]
                           if m["text"] == "Please use my browser")
            target = message["client_id"]
            assert target == a.origin + "/" + client, message
            c.operation(b, caller_b, "chat.history", {"target": a.origin, "chat_id": home_a})
            delivered = f.wait(lambda: next((m for m in c.mailbox(a, caller_a)
                if m.get("message", {}).get("message_id") == message["message_id"]), None), "client identity delivered to Agent")
            assert delivered["message"]["client_id"] == target
            received = queue.Queue()

            def connect(generation, secret):
                registration = {"client_id": client, "name": "Fixture client", "secret": secret, "generation": generation}
                stream = urlopen(Request(a.url + "/v1/client/browser/events", data=json.dumps(registration).encode(),
                    headers={"Authorization": "Bearer " + c.TOKEN, "Content-Type": "application/json"}), timeout=30)
                streams.append(stream)

                def worker():
                    data = []
                    try:
                        for raw in stream:
                            line = raw.decode().rstrip("\r\n")
                            if line.startswith("data:"):
                                data.append(line[5:].strip())
                            elif not line and data:
                                value = json.loads("\n".join(data)); data = []
                                for command in value.get("commands", []):
                                    received.put(command)
                                    c.ok(a, "POST", "/v1/client/browser/receipts", dict(registration, replies=[{
                                        "request_id": command["request_id"], "result": {"fixture": command["action"], "client": target}}]))
                    except (OSError, ValueError):
                        return
                thread = threading.Thread(target=worker, daemon=True)
                threads.append(thread); thread.start()
                return registration

            first = connect(1, "a" * 64)

            def command(node, caller, invocation, action=None):
                return c.request(node, "POST", "/v1/browser/command", {
                    "session_id": caller, "client_id": target,
                    "command": {"request_id": invocation, "action": action or {"op": "list"}}})

            local = command(a, caller_a, "same-id")
            remote = command(b, caller_b, "same-id")
            assert "client" in local[1] and "client" in remote[1], (local, remote)
            assert local[0] == remote[0] == 200 and local[1]["client"] == remote[1]["client"] == target
            commands = [received.get(timeout=5), received.get(timeout=5)]
            assert commands[0]["request_id"] != commands[1]["request_id"], commands
            assert command(b, caller_b, "same-id") == remote
            assert received.empty(), "receipt replay dispatched another browser action"
            assert command(b, caller_b, "same-id", {"op": "open", "url": "https://example.com"})[0] == 400
            connect(2, "b" * 64)
            assert c.request(a, "POST", "/v1/client/browser/receipts", dict(first, disconnect=True))[0] == 400
            assert command(b, caller_b, "after-regrant")[0] == 200
            received.get(timeout=5)
            result = {"checks": ["client ID survives source and Agent delivery", "same client across chats and Mesh nodes",
                "caller-scoped invocation receipts prevent collisions and duplicate actions", "new generation rejects old client credentials"],
                "browser": "protocol fixture; no rendering claim"}
            (REPORT / "result.json").write_text(json.dumps(result, indent=2))
            print("PASS: " + "; ".join(result["checks"]), flush=True)
        finally:
            for node in nodes:
                node.stop()
            for stream in streams:
                stream.close()
            for thread in threads:
                thread.join(timeout=3)


if __name__ == "__main__":
    main()
