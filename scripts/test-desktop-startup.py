#!/usr/bin/env python3
"""Native startup recovery, including persisted Mesh identity and usable return navigation."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing, contextmanager
import fcntl
import importlib.util
import json
import http.server
import os
from pathlib import Path
import shutil
import socket
import sqlite3
import subprocess
import tempfile
import threading
import time
import traceback

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("critical", ROOT / "scripts/smoke-critical.py")
critical = importlib.util.module_from_spec(spec)
spec.loader.exec_module(critical)


class ModelEndpoint:
    """Deterministic external provider; the Station and embedded Agent are real."""
    def __enter__(self):
        self.inputs = []
        self.received_at = {}
        self.helped = set()
        self.replied = set()
        self.completed = set()
        self.expected = ""
        self.session = ""
        self.errors = []
        inputs = self.inputs
        model = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
                inputs.append(body.decode())
                if model.expected and model.expected in body.decode():
                    model.received_at.setdefault(model.expected, time.perf_counter())
                if "node_starting" in body.decode():
                    model.errors.append("Agent reply tool failed: node_starting")
                message = {"role": "assistant", "content": "Done."}
                finish = "stop"
                if model.expected and model.expected in body.decode() and model.expected not in model.replied:
                    if model.expected not in model.helped:
                        model.helped.add(model.expected)
                        call = {"tool": "tool.help", "action": "Read the reply tool contract",
                                "arguments": {"tool": "chat.post_message"}}
                        call_id = "help-" + str(len(model.helped))
                    else:
                        model.replied.add(model.expected)
                        call = {"tool": "chat.post_message", "action": "Reply in the startup conversation",
                                "arguments": {"text": "Reply: " + model.expected}}
                        call_id = "reply-" + str(len(model.replied))
                    message = {"role": "assistant", "content": None, "tool_calls": [{
                        "id": call_id, "type": "function",
                        "function": {"name": "call", "arguments": json.dumps(call)}}]}
                    finish = "tool_calls"
                elif model.expected and model.expected in body.decode() and model.expected in model.replied:
                    message = {"role": "assistant", "content": None, "tool_calls": [{
                        "id": "end-" + str(len(model.replied)), "type": "function",
                        "function": {"name": "call", "arguments": json.dumps({"tool": "end", "action": "Finish the replied turn", "arguments": {}})}}]}
                    finish = "tool_calls"
                    model.completed.add(model.expected)
                reply = json.dumps({"id": "startup-response", "object": "chat.completion",
                    "model": "startup-model", "choices": [{"index": 0,
                    "message": message, "finish_reason": finish}],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(reply)))
                self.end_headers()
                self.wfile.write(reply)
                if finish == "stop" and model.expected and model.expected in body.decode():
                    model.completed.add(model.expected)

            def log_message(self, *_):
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.worker = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.worker.start()
        return self

    def configure(self, node):
        profiles = node / "profiles"
        profiles.mkdir(exist_ok=True)
        (profiles / "startup-fixture.json").write_text(json.dumps({
            "provider": "openai-compatible", "billing": "usage",
            "base_url": f"http://127.0.0.1:{self.server.server_port}/v1",
            "auth": {"type": "api_key", "key": "isolated-startup-test"},
            "models": [{"id": "startup-model", "api": "openai-completions", "streaming": False,
                        "thinking": ["off"], "default_thinking": "off", "default": True,
                        "capabilities": {"input": ["text"]},
                        "limits": {"context_window_tokens": 100000, "max_output_tokens": 1000}}]}))

    def received(self, text):
        return any(text in body for body in self.inputs)

    def __exit__(self, *_):
        self.server.shutdown()
        self.server.server_close()
        self.worker.join()


class Desktop:
    def __init__(self, app, root, output, case, trace):
        self.app, self.root, self.client = app, root, root / "client"
        self.output, self.case, self.trace = output, case, trace
        self.sample = {"case": case, "kind": "interaction", "milestones_ms": {}}
        self.token = os.urandom(32).hex()
        config = self.client / "node/config.json"
        try:
            occupied = {int(v.rsplit(":", 1)[1]) for v in json.loads(config.read_text())["bind"].values()}
        except (OSError, ValueError, KeyError):
            occupied = set()
        while True:
            with socket.socket() as reserve:
                reserve.bind(("127.0.0.1", 0))
                self.port = reserve.getsockname()[1]
            if self.port not in occupied:
                break

    def __enter__(self):
        env = {k: v for k, v in os.environ.items() if not k.startswith("ZORK_")}
        env.update(ZORK_CLIENT_DATA=str(self.client), ZORK_REGISTRY_DIR=str(self.root / "registry"),
                   ZORK_GUI_PREFERENCES_PATH=str(self.root / "preferences.json"), ZORK_GUI_LOCALE="zh-CN")
        if self.trace:
            self.traces = self.output / (self.case + "-trace")
            self.traces.mkdir()
            env["ZORK_STARTUP_TRACE"] = str(self.traces)
        self.log = (self.output / (self.case + ".log")).open("wb")
        self.diagnostics = []
        self.clock = time.clock_gettime_ns(time.CLOCK_MONOTONIC)
        self.started = time.perf_counter()
        self.process = subprocess.Popen([str(self.app / "Contents/MacOS/zork-gui"), "--dev",
            "--dev-port", str(self.port), "--dev-token", self.token], env=env, stdout=self.log, stderr=self.log)
        return self

    def mark(self, name):
        self.sample["milestones_ms"].setdefault(name, (time.perf_counter() - self.started) * 1000)

    def ui(self, path, body=None):
        return critical.request(self.port, path, self.token, body)

    def node_api(self, path, body=None):
        import http.client
        config = self.config()
        port = int(config["bind"]["runtime"].rsplit(":", 1)[1])
        token = config["admin"].get("token") or json.loads((self.client / "node/run/node-token.json").read_text())
        with closing(http.client.HTTPConnection("127.0.0.1", port, timeout=.5)) as conn:
            conn.request("GET" if body is None else "POST", path,
                         None if body is None else json.dumps(body),
                         {"Content-Type": "application/json", "Authorization": "Bearer " + token})
            response = conn.getresponse()
            value = json.loads(response.read())
        assert response.status in (200, 201, 202), (path, response.status, value)
        return value

    def elements(self):
        snapshot = self.ui("/v1/elements")
        if snapshot.get("elements"):
            self.mark("first_content")
        return snapshot

    def wait(self, probe, label, timeout=20):
        end = time.monotonic() + timeout
        last = "not observed"
        while time.monotonic() < end:
            assert self.process.poll() is None, "GUI exited while waiting for " + label
            try:
                value = probe()
                if value:
                    return value
            except (OSError, ValueError, KeyError, RuntimeError, sqlite3.OperationalError, critical.http.client.HTTPException) as error:
                last = str(error)
            time.sleep(.002)
        raise AssertionError(label + ": " + last)

    def visible(self, identifier):
        return critical.visible(self.elements(), identifier)

    def click(self, identifier):
        reply = self.ui("/v1/actions", {"type": "click", "target": {"element_id": identifier}})
        assert reply.get("accepted") is True, identifier + " input rejected"

    def roundtrip(self):
        self.wait(lambda: self.visible("desktop-manage"), "chat workspace")
        self.mark("chat_visible")
        self.click("desktop-manage")
        self.wait(lambda: self.visible("desktop-return"), "settings")
        self.click("desktop-return")
        self.wait(lambda: self.visible("desktop-manage"), "return to chat")
        self.mark("ui_interactive")

    def config(self):
        return json.loads((self.client / "node/config.json").read_text())

    def local_ready(self):
        node = self.client / "node"
        config = self.config()
        runtime = int(config["bind"]["runtime"].rsplit(":", 1)[1])
        agent = int(config["bind"]["agent"].rsplit(":", 1)[1])
        pid = int((node / "run/zork-station.pid").read_text())
        token = config["admin"].get("token") or json.loads((node / "run/node-token.json").read_text())
        critical.validate_node(node, json.loads(critical.control(node, "status")),
            critical.request(runtime, "/readyz"), critical.request(agent, "/readyz"),
            critical.request(runtime, "/v1/node/status", token), pid)
        critical.request(runtime, "/v1/tasks", token)
        self.mark("local_node")
        return True

    def create_chat(self, model):
        entry = self.wait(lambda: next((e["id"] for e in self.elements()["elements"]
                          if e["id"].startswith("new-chat-") and e["role"] == "button"
                          and e["label"] == "新建 Chat" and e["visible"] and e["enabled"]), None), "new Chat entry")
        self.click(entry)
        self.wait(lambda: self.visible("new-chat-input"), "new Chat editor")
        assert self.node_api("/v1/node/chats")["items"] == []
        for control in ("new-chat-device", "new-chat-model", "new-chat-thinking", "new-chat-profile"):
            self.wait(lambda: any(e["id"] == control and e["visible"] and e["enabled"]
                                 for e in self.elements()["elements"]), control + " ready")
        self.click("new-chat-device")
        self.wait(lambda: self.visible("new-chat-device-0"), "device choices")
        self.click("new-chat-device-0")
        self.click("new-chat-model")
        self.wait(lambda: self.visible("new-chat-model-0"), "model choices")
        self.click("new-chat-model-0")
        assert self.node_api("/v1/node/chats")["items"] == [], "selecting controls created an empty Chat"
        self.screenshot("new-chat-empty")
        text = "startup first Chat message"
        model.expected = text
        self.click("new-chat-input")
        reply = self.ui("/v1/actions", {"type": "type_text", "text": text})
        assert reply.get("accepted") is True
        self.wait(lambda: any(e["id"] == "new-chat-send" and e["enabled"] for e in self.elements()["elements"]), "first send enabled")
        self.screenshot("new-chat-draft")
        self.click("new-chat-send")
        chat = self.wait(lambda: next(iter(self.node_api("/v1/node/chats")["items"]), None), "first Chat committed")
        session = chat["chat_id"]
        self.wait(lambda: self.visible("composer-input"), "created Chat opened")
        def confirmed():
            with closing(sqlite3.connect((self.client / "client.db").as_uri() + "?mode=ro", uri=True, timeout=.05)) as db:
                messages = [(status, json.loads(value)["content"]) for status, value in db.execute("SELECT status,value FROM messages WHERE session=?", (session,))]
                return messages.count(("sent", text)) == 1 and ("sent", "Reply: " + text) in messages
        self.wait(confirmed, "first user message and Session reply confirmed in the same client cache")
        self.wait(lambda: any(e["visible"] and e["label"] == "Reply: " + text
                  for e in self.elements()["elements"]), "first Agent reply painted in the created Chat")
        assert self.node_api("/v1/node/agents")["items"] == []
        assert len(self.node_api("/v1/node/chats")["items"]) == 1
        self.wait(lambda: text in model.completed and any(s["session_id"] == session and s["status"] == "wait" for s in self.node_api("/v1/im/sessions")["items"]), "first Session settles")
        time.sleep(.4)  # Capture the reply after its ordinary entry transition.
        self.screenshot("new-chat-created")
        return session

    def chat(self, session, model):
        self.selected_chat = session
        def chat_entry():
            self.navigation_at_click = self.elements()
            return next((e["id"] for e in self.navigation_at_click.get("elements", [])
                         if e["id"].startswith("chat-") and e["id"].endswith("-" + session)
                         and e["visible"] and e["enabled"]), None)
        self.click(self.wait(chat_entry, "restored Chat"))
        self.wait(lambda: self.visible("composer-input"), "editable conversation")
        self.mark("composer_editable")
        text = "startup conversation input " + self.case
        model.expected, model.session = text, session
        reply = self.ui("/v1/actions", {"type": "type_text", "text": text,
                                        "target": {"element_id": "composer-input"}})
        assert reply.get("accepted") is True
        def send_action():
            return any(e["id"] == "send-button" and e["visible"] and e["enabled"]
                       and e["label"] == "发送消息" for e in self.elements().get("elements", []))
        self.wait(send_action, "enabled conversation send action, not the stop action")
        self.mark("send_action_enabled")
        self.click("send-button")
        self.mark("send_clicked")

        diagnosed = False
        def confirmed(content):
            nonlocal diagnosed
            with closing(sqlite3.connect((self.client / "client.db").as_uri() + "?mode=ro", uri=True, timeout=.05)) as db:
                found = any(status == "sent" and position is not None and json.loads(value)["content"] == content
                           for status, position, value in db.execute(
                               "SELECT status,position,value FROM messages WHERE session=?", (session,)))
                if not found and not diagnosed and (time.perf_counter() - self.started) * 1000 > self.sample["milestones_ms"]["send_clicked"] + 1000:
                    diagnosed = True
                    detail = {"model_received_ms": ((model.received_at[text] - self.started) * 1000) if text in model.received_at else None,
                              "outbox": [dict(zip(("status", "attempted", "error", "value"), row))
                                         for row in db.execute("SELECT status,attempted,error,value FROM messages WHERE status!='sent'")]}
                    for label, pid in [("gui", self.process.pid), ("station", int((self.client / "node/run/zork-station.pid").read_text()))]:
                        self.diagnostics.append(subprocess.Popen(["sample", str(pid), "1", "10", "-mayDie", "-file",
                            str(self.output / (self.case + "-" + label + "-sample.txt"))], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
                    try:
                        detail["source_page"] = self.node_api("/v1/im/sessions/" + session + "/messages")
                    except Exception as error:
                        detail["source_page_error"] = str(error)
                    (self.output / (self.case + "-slow-delivery.json")).write_text(json.dumps(detail, ensure_ascii=False, indent=2))
                return found
        try:
            self.wait(lambda: confirmed(text), "source echo committed by the client")
        except Exception:
            (self.output / (self.case + "-ui.json")).write_text(json.dumps(self.elements(), ensure_ascii=False, indent=2))
            raise
        self.mark("message_confirmed")
        self.wait(lambda: model.received(text), "real embedded Agent reaches its model provider")
        self.sample["milestones_ms"]["agent_received_input"] = (model.received_at[text] - self.started) * 1000
        def reply_received():
            assert not model.errors, model.errors
            return confirmed("Reply: " + text)
        try:
            self.wait(reply_received, "Agent chat.post_message reply reaches the client")
        except Exception:
            (self.output / (self.case + "-model.json")).write_text(json.dumps(model.inputs, indent=2))
            raise
        self.mark("agent_reply_received")
        self.sample["after_first_content_ms"] = {
            key: self.sample["milestones_ms"][key] - self.sample["milestones_ms"]["first_content"]
            for key in ("composer_editable", "send_action_enabled", "message_confirmed", "agent_received_input", "agent_reply_received")}
        # A reply can arrive before its publishing tool and final model turn
        # settle; the next restart must begin without prior in-flight work.
        self.wait(lambda: any(s["session_id"] == session and s["status"] == "wait"
                              for s in self.node_api("/v1/im/sessions")["items"])
                              and text in model.completed, "reply turn settles after the final model response")

    def measure(self, session, model):
        self.sample["kind"] = "paired-startup-performance"
        def mesh():
            self.wait(lambda: int((self.client / "node/run/zork-mesh.pid").read_text()) ==
                      int((self.client / "node/run/zork-station.pid").read_text()) and self.origin(),
                      "Station Mesh bridge and sources ready")
            self.mark("station_mesh")
            self.wait(self.transport_ready, "client Mesh transport ready")
            self.mark("client_mesh")
        with ThreadPoolExecutor(max_workers=3) as pool:
            local = pool.submit(self.wait, self.local_ready, "local Station and embedded Agent")
            ui = pool.submit(self.chat, session, model)
            transport = pool.submit(mesh)
            local.result()
            ui.result()
            transport.result()
        self.sample["passed"] = all(self.sample["milestones_ms"][name] < 1000
                                    for name in ("local_node", "message_confirmed", "agent_reply_received",
                                                 "station_mesh", "client_mesh"))

    def background_chat(self, session, model):
        # Mesh starts delayed HTTPS probes after the first frame. Exercise the
        # business API while they are running, independently of startup timing.
        time.sleep(max(0, self.started + 1 - time.perf_counter()))
        original, started, case = self.sample, self.started, self.case
        self.sample = {"case": case + "-background-network", "kind": "background-network-chat", "milestones_ms": {}}
        self.started, self.case = time.perf_counter(), self.sample["case"]
        try:
            self.chat(session, model)
            self.sample["passed"] = self.sample["milestones_ms"]["agent_reply_received"] < 1000
            original["background_network_chat"] = self.sample
            original["passed"] = original["passed"] and self.sample["passed"]
        finally:
            self.sample, self.started, self.case = original, started, case

    def origin(self):
        config = self.config()
        runtime = int(config["bind"]["runtime"].rsplit(":", 1)[1])
        return critical.request(runtime, "/v1/node/mesh", config["admin"]["token"])["origin"]

    def transport_ready(self):
        data = json.loads((self.client / "transport/client-mesh-ready.json").read_text())
        return data["pid"] == self.process.pid

    def screenshot(self, name):
        import http.client
        with closing(http.client.HTTPConnection("127.0.0.1", self.port, timeout=3)) as conn:
            conn.request("GET", "/v1/screenshot", headers={"Authorization": "Bearer " + self.token})
            response = conn.getresponse()
            pixels = response.read()
        assert response.status == 200 and pixels.startswith(b"\x89PNG\r\n\x1a\n")
        (self.output / (name + ".png")).write_bytes(pixels)

    def __exit__(self, error_type, error, _):
        if error_type:
            if self.process.poll() is None:
                subprocess.run(["sample", str(self.process.pid), "1", "10", "-mayDie", "-file",
                    str(self.output / (self.case + "-failure-sample.txt"))], capture_output=True, timeout=5)
            try:
                client_log = self.client / "logs/client.log"
                if client_log.exists():
                    shutil.copy2(client_log, self.output / (self.case + "-client.log"))
                for log in (self.client / "node").glob("*.log"):
                    shutil.copy2(log, self.output / (self.case + "-" + log.name))
                (self.output / (self.case + "-failed-sample.json")).write_text(json.dumps(self.sample, ensure_ascii=False, indent=2))
                (self.output / (self.case + "-failed-ui.json")).write_text(json.dumps(self.elements(), ensure_ascii=False, indent=2))
                if hasattr(self, "navigation_at_click"):
                    (self.output / (self.case + "-clicked-ui.json")).write_text(json.dumps(self.navigation_at_click, ensure_ascii=False, indent=2))
                self.screenshot(self.case + "-failed")
                if str(error).startswith("editable conversation"):
                    entry = next(e["id"] for e in self.elements()["elements"]
                                 if e["id"].startswith("chat-") and e["id"].endswith("-" + self.selected_chat))
                    self.click(entry)
                    recovered = False
                    try:
                        self.wait(lambda: self.visible("composer-input"), "diagnostic second click", timeout=2)
                        recovered = True
                    except AssertionError:
                        pass
                    (self.output / (self.case + "-retry-diagnostic.json")).write_text(json.dumps({"second_click_opened": recovered}))
            except Exception:
                pass
        shutdown_error = None
        try:
            if self.process.poll() is None:
                subprocess.run(["osascript", "-l", "JavaScript", "-e",
                    "ObjC.import('AppKit'); $.NSRunningApplication.runningApplicationWithProcessIdentifier("
                    + str(self.process.pid) + ").terminate;"], check=True, capture_output=True, timeout=5)
                self.process.wait(timeout=20)
            self.sample["shutdown"] = "native quit"
            if self.sample["kind"] == "paired-startup-performance":
                receipts = {name: (self.client / path / "mesh/synch/clean-start.json").is_file()
                            for name, path in [("station", "node"), ("client", "transport")]}
                self.sample["shutdown_receipts"] = receipts
                if not all(receipts.values()):
                    raise RuntimeError("Mesh did not record a complete shutdown: " + str(receipts))
        except Exception as failure:
            shutdown_error = failure
            self.sample["shutdown_error"] = str(failure)
            self.sample["exit_code"] = self.process.poll()
            (self.output / (self.case + "-shutdown-error.json")).write_text(json.dumps(self.sample, indent=2))
            for log in [self.client / "logs/client.log", *list((self.client / "node").glob("*.log"))]:
                if log.exists():
                    shutil.copy2(log, self.output / (self.case + "-shutdown-" + log.name))
        finally:
            critical.cleanup(self.process, self.app, self.root)
        self.log.close()
        for process in self.diagnostics:
            process.wait(timeout=10)
        if self.trace:
            marks = []
            for path in self.traces.glob("*.jsonl"):
                lines = path.read_text().splitlines(keepends=True)
                for index, line in enumerate(lines):
                    try:
                        mark = json.loads(line)
                    except ValueError:
                        if index == len(lines) - 1 and not line.endswith("\n"):
                            self.sample.setdefault("incomplete_trace_files", []).append(path.name)
                            continue
                        raise
                    marks.append({"mark": mark["mark"], "ms": (mark["ns"] - self.clock) / 1e6})
            self.sample["process_marks"] = sorted(marks, key=lambda mark: mark["ms"])
        if shutdown_error and error_type is None:
            raise RuntimeError("normal application shutdown failed") from shutdown_error


@contextmanager
def client_root(app):
    root = Path(tempfile.mkdtemp(prefix="zsr-", dir="/tmp")).resolve()
    try:
        yield root
    finally:
        if critical.owned_processes(app, root):
            raise RuntimeError("Fixture processes survived cleanup; retained " + str(root))
        shutil.rmtree(root)


def run(app, output, trace, reports, restarts=3):
    with client_root(app) as root:
        with Desktop(app, root, output, "welcome", trace) as desktop:
            desktop.wait(lambda: desktop.visible("desktop-startup-page"), "welcome page")
            assert not desktop.visible("desktop-return"), "unrequested settings replaced startup"
            desktop.screenshot("welcome")
            desktop.click("desktop-startup-settings")
            desktop.wait(lambda: desktop.visible("desktop-return"), "settings from welcome")
            desktop.click("desktop-return")
            desktop.wait(lambda: desktop.visible("desktop-startup-page"), "return without an active device")
            assert not (root / "client/node/config.json").exists(), "welcome silently enabled a node"
            desktop.sample["passed"] = True
        reports.append(desktop.sample)

    with client_root(app) as root, ModelEndpoint() as model:
        client = critical.fixture(root)
        node = client / "node"
        node.mkdir()
        (node / "config.json").write_text("invalid startup fixture")
        with Desktop(app, root, output, "failed-startup", trace) as desktop:
            desktop.wait(lambda: desktop.visible("desktop-startup-retry"), "visible startup error and retry")
            assert desktop.visible("desktop-startup-page")
            desktop.screenshot("startup-error")
            desktop.click("desktop-startup-settings")
            desktop.wait(lambda: desktop.visible("desktop-return"), "settings after failure")
            desktop.click("desktop-return")
            desktop.wait(lambda: desktop.visible("desktop-startup-retry"), "return to failed startup")
            desktop.click("desktop-startup-settings")
            desktop.wait(lambda: desktop.visible("local-node-toggle"), "failed local node controls")
            desktop.click("local-node-toggle")
            desktop.click("desktop-return")
            desktop.wait(lambda: desktop.visible("desktop-start-local"), "disabled startup clears its failure")
            desktop.click("desktop-start-local")
            desktop.wait(lambda: desktop.visible("desktop-startup-retry"), "new start reports the remaining configuration error")
            # Repair only this deliberately invalid fixture, then use the real retry action.
            (node / "config.json").unlink()
            desktop.click("desktop-startup-retry")
            desktop.roundtrip()
            desktop.wait(desktop.local_ready, "retry starts real local services")
            desktop.sample["passed"] = True
        reports.append(desktop.sample)

        # Hold one local service port to exercise a genuinely pending startup.
        # This is a UI fault scenario, never a performance sample or fake Agent.
        with sqlite3.connect(client / "client.db") as db:
            db.execute("DELETE FROM nodes")
        with socket.socket() as occupied:
            occupied.bind(("127.0.0.1", 0))
            occupied.listen()
            config = json.loads((node / "config.json").read_text())
            config["bind"]["agent"] = "127.0.0.1:" + str(occupied.getsockname()[1])
            (node / "config.json").write_text(json.dumps(config))
            with Desktop(app, root, output, "pending-local-startup", trace) as desktop:
                desktop.wait(lambda: desktop.visible("desktop-startup-status"), "local preparation status")
                assert desktop.visible("desktop-startup-page")
                time.sleep(.3)  # Inspect the visible delayed indicator after observing readiness state.
                desktop.screenshot("startup-preparing")
                desktop.click("desktop-startup-settings")
                desktop.wait(lambda: desktop.visible("desktop-return"), "settings during preparation")
                desktop.click("desktop-return")
                desktop.wait(lambda: desktop.visible("desktop-startup-page"), "return during preparation")
                desktop.click("desktop-startup-settings")
                desktop.wait(lambda: desktop.visible("desktop-return"), "explicit settings choice")
                occupied.close()
                desktop.wait(desktop.local_ready, "local services recover when their port is available")
                desktop.wait(lambda: any(e["id"] == "desktop-return" and e["label"] == "返回对话"
                    for e in desktop.elements()["elements"]), "settings remain after recovery")
                desktop.click("desktop-return")
                desktop.wait(lambda: desktop.visible("desktop-manage"), "return to recovered workspace")
                desktop.sample["passed"] = True
        reports.append(desktop.sample)

        config = json.loads((node / "config.json").read_text())
        config["mesh"]["enabled"] = True
        (node / "config.json").write_text(json.dumps(config))
        model.configure(node)
        with Desktop(app, root, output, "station-identity", trace) as desktop:
            desktop.roundtrip()
            origin = desktop.wait(desktop.origin, "authenticated local Mesh identity")
            session = desktop.create_chat(model)
            desktop.sample["passed"] = True
            reports.append(desktop.sample)
        # The same persisted fact is recorded after real enrollment. No user DB,
        # fixed identity, offline mode or pre-running service is used by this fixture.
        with sqlite3.connect(client / "client.db") as db:
            local = next(json.loads(row[0]) for row in db.execute("SELECT value FROM nodes")
                         if json.loads(row[0])["local"])
            db.execute("INSERT OR REPLACE INTO cache VALUES (?,?,?)", (local["id"], "mesh-origin", json.dumps(origin)))
        with Desktop(app, root, output, "client-identity", trace) as desktop:
            desktop.roundtrip()
            desktop.wait(desktop.transport_ready, "persisted client transport")

        for index in range(restarts):
            with Desktop(app, root, output, "paired-restart-" + str(index + 1), trace) as desktop:
                desktop.measure(session, model)
                desktop.wait(desktop.transport_ready, "background Mesh eventually recovers")
                desktop.background_chat(session, model)
            reports.append(desktop.sample)
            print(json.dumps({k: v for k, v in desktop.sample.items() if k != "process_marks"}), flush=True)

        # Hold the same boundary as a pending Mesh initialization without a
        # running Mesh service. This is fault coverage, never a timing sample.
        config = json.loads((node / "config.json").read_text())
        config["mesh"]["enabled"] = False
        (node / "config.json").write_text(json.dumps(config))
        with Desktop(app, root, output, "local-reply-without-mesh", trace) as desktop:
            desktop.wait(desktop.local_ready, "local runtime without Mesh")
            desktop.roundtrip()
            assert desktop.node_api("/v1/node/mesh").get("origin") is None
            config["mesh"]["enabled"] = True
            (node / "config.json").write_text(json.dumps(config))
            desktop.chat(session, model)
            desktop.sample["passed"] = True
            desktop.sample["fixture"] = "Mesh enabled in configuration while its runtime remains unavailable; functional fault coverage only"
        reports.append(desktop.sample)

        # The known device is unavailable. Its cached workspace and navigation
        # must still open while Mesh readoption is pending in the background.
        with sqlite3.connect(client / "client.db") as db:
            db.execute("DELETE FROM nodes")
            remote = {**local, "id": "offline-peer", "local": False, "url": "", "token": None,
                      "mesh": {"origin": origin, "addr": None}}
            db.execute("INSERT INTO nodes VALUES (?,?)", (remote["id"], json.dumps(remote)))
            db.execute("INSERT OR REPLACE INTO cache VALUES ('device','local-node-enabled','false')")
            db.execute("INSERT OR REPLACE INTO cache VALUES ('device','last-node',?)", (json.dumps(remote["id"]),))
        with Desktop(app, root, output, "offline-cached-workspace", trace) as desktop:
            desktop.sample["kind"] = "cached-navigation-performance"
            desktop.roundtrip()
            desktop.wait(lambda: desktop.visible("new-chat-loading") or desktop.visible("new-chat-error"),
                         "unknown device content is not presented as an empty catalog")
            desktop.screenshot("cached-workspace-preparing")
            desktop.sample["passed"] = desktop.sample["milestones_ms"]["ui_interactive"] < 1000
        reports.append(desktop.sample)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/desktop-startup")
    parser.add_argument("--trace-startup", action="store_true")
    parser.add_argument("--restarts", type=int, default=3)
    args = parser.parse_args()
    if not 1 <= args.restarts <= 50:
        parser.error("--restarts must be between 1 and 50")
    app, output = args.app.resolve(strict=True), args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    report = {"passed": False, "samples": [], "paired_startup_budget_ms": 1000, "cached_navigation_budget_ms": 1000,
              "preferred_ms": 500, "tracing": args.trace_startup,
              "measurement": "external wall time; real UI conversation input/send, source echo, and real embedded Agent chat.post_message reply reaching the client; deterministic external model HTTP provider; OS caches uncontrolled",
              "first_launch": "welcome records the first application launch as a functional check, including OS assessment; its latency is not covered by the paired-startup budget; the approved all-startup gate remains in smoke-critical.py"}
    try:
        with (app.parent / "owner.lock").open("a+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            run(app, output, args.trace_startup, report["samples"], args.restarts)
            report["binary_sha256"] = critical.digest(app / "Contents/MacOS/zork-gui")
        report["passed"] = all(sample["passed"] for sample in report["samples"])
    except Exception as error:
        report["error"] = str(error)
        report["traceback"] = traceback.format_exc()
    (output / "result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(("PASS" if report["passed"] else "FAIL") + " desktop startup: " + str(output / "result.json"), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
