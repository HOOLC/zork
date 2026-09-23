"""Isolated real Station peer and source marker for the critical startup smoke."""
from contextlib import closing
import http.client
import http.server
import ipaddress
import json
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import threading
import time


def unused_port(kind=socket.SOCK_STREAM):
    with socket.socket(type=kind) as reservation:
        reservation.bind(("127.0.0.1", 0))
        return reservation.getsockname()[1]


def wait_for(probe, process, label, seconds=30):
    deadline = time.monotonic() + seconds
    last_error = "not observed"
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"{label}: Station exited")
        try:
            value = probe()
            if value:
                return value
        except (OSError, ValueError, KeyError, RuntimeError, http.client.HTTPException) as error:
            last_error = str(error)
        time.sleep(.01)
    raise RuntimeError(f"{label}: {last_error}")


def cached_message(client, origin, session, content):
    database = (client / "client.db").as_uri() + "?mode=ro"
    with closing(sqlite3.connect(database, uri=True, timeout=.05)) as connection:
        rows = connection.execute(
            "SELECT value FROM messages WHERE node=? AND session=? AND status='sent' AND position IS NOT NULL",
            (origin, session),
        )
        return any(json.loads(value).get("content") == content for (value,) in rows)


def save_remote_node(client, origin, address):
    node = {"id": origin, "name": "Smoke Remote", "url": "", "token": None,
            "local": False, "mesh": {"origin": origin, "addr": address}, "group": None}
    with closing(sqlite3.connect(client / "client.db")) as connection:
        connection.execute("INSERT INTO nodes(id,value) VALUES (?,?)", (origin, json.dumps(node)))
        connection.commit()


class RemoteStation:
    def __init__(self, image, root, output, local_config, binaries=None):
        self.image = image
        self.root = root / "remote"
        self.root.mkdir()
        self.output = output
        self.network = "zork-critical-" + root.name
        self.container = self.network + "-station"
        self.address = None
        self.config = json.loads(json.dumps(local_config))
        self.config["bind"] = {name: f"0.0.0.0:{unused_port()}"
                               for name in self.config["bind"]}
        self.config["admin"] = {"token": os.urandom(32).hex()}
        self.config["mesh"].update(enabled=True, name="Smoke Remote",
                                   bind=f"0.0.0.0:{unused_port(socket.SOCK_DGRAM)}",
                                   peers=[], group=None)
        if self.config["mesh"]["offline"]:
            raise RuntimeError("Critical Mesh fixture must use normal network configuration")
        self.write_config()
        self.process = None
        self.origin = None
        self.client_origin = None
        self.session = None
        self.binding = None
        self.model_server = None
        self.model_thread = None
        self.docker("network", "create", "--driver", "bridge", self.network)
        try:
            mounted = (["--mount", f"type=bind,source={binaries.resolve()},target=/opt/zork-smoke,readonly",
                        "--entrypoint", "/opt/zork-smoke/zork-station"] if binaries else [])
            self.docker("create", "--name", self.container, "--network", self.network,
                        "--mount", f"type=bind,source={self.root},target=/data",
                        "--env", "ZORK_REGISTRY_DIR=/data/registry", *mounted, image,
                        "--data", "/data")
            details = self.inspect()
            network = json.loads(self.docker("network", "inspect", self.network))[0]
            if details["HostConfig"]["NetworkMode"] != self.network or network["Driver"] != "bridge":
                raise RuntimeError("Remote Station must use its own Docker bridge network")
        except BaseException:
            self.docker("rm", "-f", self.container, check=False)
            self.docker("network", "rm", self.network, check=False)
            raise

    @staticmethod
    def docker(*args, check=True):
        result = subprocess.run(["docker", *args], capture_output=True, text=True)
        if check and result.returncode:
            raise RuntimeError(f"docker {' '.join(args[:2])}: {result.stderr.strip()}")
        return result.stdout.strip()

    def inspect(self):
        return json.loads(self.docker("inspect", self.container))[0]

    def poll(self):
        return None if self.inspect()["State"]["Running"] else 1

    def write_config(self):
        path = self.root / "config.json"
        pending = self.root / "config.json.pending"
        with pending.open("w") as target:
            json.dump(self.config, target)
            target.flush()
            os.fsync(target.fileno())
        pending.replace(path)

    def request(self, path, body=None, admin=False):
        port = int(self.config["bind"]["runtime"].rsplit(":", 1)[1])
        headers = {"Content-Type": "application/json"}
        if admin:
            headers["Authorization"] = "Bearer " + self.config["admin"]["token"]
        with closing(http.client.HTTPConnection(self.address, port, timeout=2)) as connection:
            connection.request("GET" if body is None else "POST", path,
                               None if body is None else json.dumps(body), headers)
            response = connection.getresponse()
            raw = response.read()
            value = json.loads(raw) if raw else None
            if response.status not in (200, 201, 202):
                raise RuntimeError(f"remote {path}: HTTP {response.status}: {value}")
            return value

    def start(self, label):
        if self.process is not None:
            raise RuntimeError("Remote Station is already running")
        self.label = label
        self.docker("start", self.container)
        self.process = self
        try:
            address = wait_for(
                lambda: self.inspect()["NetworkSettings"]["Networks"][self.network]["IPAddress"],
                self, "remote Station bridge address", seconds=5)
            if ipaddress.ip_address(address).is_loopback or (self.address and self.address != address):
                raise RuntimeError("Remote Station lost its separate network address")
            self.address = address
            agent_port = int(self.config["bind"]["agent"].rsplit(":", 1)[1])
            def ready():
                pid = int((self.root / "run/zork-mesh.pid").read_text())
                station = self.request("/readyz")
                if pid <= 0 or station.get("pid") != pid or station.get("ok") is not True:
                    return None
                with closing(http.client.HTTPConnection(self.address, agent_port, timeout=.5)) as conn:
                    conn.request("GET", "/readyz")
                    if conn.getresponse().status != 200:
                        return None
                origin = self.request("/v1/mesh").get("origin")
                return origin if isinstance(origin, str) and origin.startswith("key:") else None
            origin = wait_for(ready, self, "remote Station Mesh and Agent ready")
            if self.origin is not None and origin != self.origin:
                raise RuntimeError("Remote Station identity changed across restart")
            self.origin = origin
            return origin
        except BaseException:
            self.stop()
            raise

    def stop(self):
        process, self.process = self.process, None
        if process is not None:
            self.docker("stop", "--time", "5", self.container, check=False)
            with (self.output / f"remote-{self.label}.log").open("w") as log:
                subprocess.run(["docker", "logs", self.container], stdout=log,
                               stderr=subprocess.STDOUT, check=False)
        if self.model_server is not None:
            self.model_server.shutdown()
            self.model_server.server_close()
            self.model_thread.join()
            self.model_server = None
            self.model_thread = None

    def close(self):
        try:
            self.stop()
        finally:
            self.docker("rm", "-f", self.container, check=False)
            self.docker("network", "rm", self.network, check=False)

    def start_model_provider(self):
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers.get("Content-Length", "0")))
                reply = json.dumps({
                    "id": "smoke-response", "object": "chat.completion", "model": "fixture-model",
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": "Done."},
                                 "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
                }).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(reply)))
                self.end_headers()
                self.wfile.write(reply)

            def log_message(self, *_):
                pass

        self.model_server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.model_thread = threading.Thread(target=self.model_server.serve_forever, daemon=True)
        self.model_thread.start()
        return self.model_server.server_port

    def trust_client(self, client_origin):
        self.stop()
        self.client_origin = client_origin
        self.config["mesh"]["peers"] = [{"origin": client_origin, "name": "Smoke Client",
                                         "addr": None, "client": True}]
        self.write_config()
        model_port = self.start_model_provider()
        profiles = self.root / "profiles"
        profiles.mkdir(exist_ok=True)
        (profiles / "fixture.json").write_text(json.dumps({
            "provider": "openai-compatible", "billing": "usage",
            "base_url": f"http://host.docker.internal:{model_port}/v1",
            "auth": {"type": "api_key", "key": "sk-test"},
            "models": [{"id": "fixture-model", "api": "openai-completions",
                        "streaming": False, "thinking": ["off"], "default_thinking": "off",
                        "capabilities": {"input": ["text"]},
                        "limits": {"context_window_tokens": 100000, "max_output_tokens": 1000},
                        "default": True}],
        }))
        (self.root / "workspace").mkdir(exist_ok=True)
        self.start("trusted")

    def post_source_marker(self):
        if self.binding is None:
            self.request("/v1/node/agents", {"profile_id": "fixture", "model": "fixture-model",
                         "thinking": "off", "id": "remote-leader", "name": "Smoke Leader",
                         "role": "leader"}, admin=True)
            opened = self.request("/v1/node/agents/remote-leader/open", {}, admin=True)
            self.session = opened["session_id"]
            context = self.request(f"/v1/tools/context?threadId={self.session}")
            self.binding = {"sessionKey": context["sessionKey"],
                            "conversationId": context["conversationId"],
                            "rootMessageId": context["rootMessageId"]}
            wait_for(lambda: next((item for item in self.request("/v1/im/sessions")["items"]
                                   if item["session_id"] == self.session and item["status"] == "wait"),
                                  None), self.process, "remote Agent settled before timing")
        marker = f"critical Mesh source {time.time_ns()}"
        self.request("/chat/post-message", {**self.binding, "kind": "progress", "text": marker})
        wait_for(lambda: any(message.get("content") == marker for message in
                             self.request(f"/v1/im/sessions/{self.session}/messages")["items"]),
                 self.process, "remote source marker committed")
        return marker
