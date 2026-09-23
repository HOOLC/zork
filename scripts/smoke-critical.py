#!/usr/bin/env python3
"""Run all user-approved critical gates against freshly built product artifacts."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from contextlib import closing, ExitStack
import hashlib
import http.client
import ipaddress
import json
import os
from pathlib import Path
import platform
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts/lib"))
from smoke_mesh_fixture import RemoteStation, cached_message, save_remote_node, unused_port
from smoke_remote_frame import run as run_remote_frame

GATES = {
    "local-startup": {
        "requirement": "GUI 进入 main 后 1s 本机与已配对 Mesh 启动完毕",
        "acceptance": "不计进入 GUI main 前的 macOS 系统评估；首次初始化、Station 与客户端 Mesh 首次初始化和无预运行本机服务的已配对重启须在预算内达到界面交互、本机 Station/Agent、Mesh 就绪、真实远端直连及新增远端消息同步落盘",
        "budget_ms": 1000,
        "cases": {
            "first-initialization": ["ui_interactive", "local_node"],
            "restart-1": ["ui_interactive", "local_node"],
            "restart-2": ["ui_interactive", "local_node"],
            "mesh-first-initialization": ["ui_interactive", "local_node", "station_mesh"],
            "client-mesh-first-initialization": ["ui_interactive", "local_node", "station_mesh", "client_mesh"],
            "paired-restart-1": ["ui_interactive", "local_node", "station_mesh", "client_mesh", "remote_connection", "remote_sync"],
            "paired-restart-2": ["ui_interactive", "local_node", "station_mesh", "client_mesh", "remote_connection", "remote_sync"],
        },
    },
    "client-frame": {
        "requirement": "原生客户端整帧小于 8ms，持续绘制超过 120fps 且稳定",
        "acceptance": "原生端组件展示目录的实际 PlainDialog 完整组件开合、内容和背景淡入中间帧及最终输入状态，页面滚动和目录内部滚动，并覆盖生产客户端会话面板及十万条混合消息滚动；滚动必须通过真实输入推动可见内容，弹窗必须到达对应的输入与动画终态。使用同一原生 Metal 渲染器与离屏目标隔离显示缓冲供应限速；输入/动画更新开始至 CPU 和 Metal GPU 均完成的全部帧严格小于预算，连续绘制完成吞吐严格超过目标 FPS，最慢连续完成间隔也须小于目标帧时长；包含首次操作与首帧，主动空闲不算掉帧；测原生渲染能力，不冒充显示器实际呈现 FPS",
        "budget_ms": 8,
        "target_fps": 120,
        "fixture": {
            "viewport": [1280, 800],
            "component_viewport": [798, 838],
            "offscreen": True,
            "gallery": True,
            "gallery_scroll": {
                "px_per_second": 1200,
                "legs": 5,
                "min_displacement_px": 20,
            },
            "messages": 100000,
            "panel_pairs": 4,
            "phase_ms": 1000,
            "scroll_px_per_second": 6000,
        },
    },
}
OBSERVE_SECONDS = 15  # Collect post-main failure diagnostics; never a passing time budget.
PRE_MAIN_OBSERVE_SECONDS = 90  # A diagnostic ceiling, excluded from the gate budget.


def request(port, path, token=None, body=None):
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    with closing(http.client.HTTPConnection("127.0.0.1", port, timeout=.5)) as connection:
        connection.request("GET" if body is None else "POST", path,
                           None if body is None else json.dumps(body), headers)
        response = connection.getresponse()
        value = json.loads(response.read())
        if response.status != 200:
            raise RuntimeError(f"{path}: HTTP {response.status}")
        return value


def control(root, command):
    # The fixture uses a short root, below the production Unix socket path limit.
    with socket.socket(socket.AF_UNIX) as stream:
        stream.settimeout(.5)
        stream.connect(str(root / "run/sup.sock"))
        stream.sendall((command + "\n").encode())
        with stream.makefile("r") as reader:
            return reader.readline().strip()


def visible(snapshot, identifier):
    return any(e["id"] == identifier and e["visible"] and e["enabled"]
               for e in snapshot.get("elements", []))


def validate_node(root, supervisor, station, agent, info, pid):
    if (supervisor.get("agent_mode") != "embedded" or supervisor.get("protocol") != 1
            or supervisor.get("data_root") != str(root)):
        raise RuntimeError("Wrong supervisor generation or data root")
    if (station.get("ok") is not True or agent.get("ok") is not True
            or agent.get("embedded") is not True or not isinstance(pid, int) or pid <= 0
            or station.get("pid") != pid or agent.get("pid") != pid):
        raise RuntimeError("Station and embedded Agent are not ready in the same process")
    if info.get("protocol") != 1 or info.get("data_root") != str(root):
        raise RuntimeError("Local API does not belong to this fixture")


def monotonic_ns():
    return time.clock_gettime_ns(time.CLOCK_MONOTONIC)


def read_main_marker(path, pid, started_ns):
    marker_pid, marked_ns = map(int, path.read_text().split())
    if marker_pid != pid or not started_ns <= marked_ns <= monotonic_ns():
        raise RuntimeError("GUI main marker does not belong to this process launch")
    return marked_ns


def wait_for_main(path, process, started_ns):
    deadline = time.monotonic() + PRE_MAIN_OBSERVE_SECONDS
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"GUI exited before main with {process.returncode}")
        try:
            return read_main_marker(path, process.pid, started_ns)
        except (FileNotFoundError, ValueError):
            time.sleep(.001)
    raise RuntimeError("GUI main marker missing; rebuild the packaged app")


def observe(probe, process, main_ns):
    last_error = "readiness not observed"
    while monotonic_ns() - main_ns < OBSERVE_SECONDS * 1_000_000_000:
        if process.poll() is not None:
            return {"error": f"GUI exited with {process.returncode}"}
        try:
            if probe():
                return {"ms": (monotonic_ns() - main_ns) / 1_000_000}
        except (OSError, ValueError, KeyError, RuntimeError, sqlite3.Error,
                http.client.HTTPException) as error:
            last_error = str(error)
        time.sleep(.001)
    return {"error": last_error}


def gate_passes(sample, budget_ms):
    required = GATES["local-startup"]["cases"].get(sample.get("case"))
    if sample.get("case", "").startswith("paired-"):
        try:
            address = ipaddress.ip_address(sample["remote_address"])
            if address.is_loopback or sample.get("remote_network_driver") != "bridge":
                return False
        except (KeyError, ValueError):
            return False
    return (sample.get("gui_alive") is True
            and required is not None
            and type(sample.get("pre_main_ms")) in (int, float)
            and 0 <= sample["pre_main_ms"] < float("inf")
            and all(type(sample.get(key, {}).get("ms")) in (int, float)
                    and 0 <= sample[key]["ms"] <= budget_ms
                    and "error" not in sample[key]
                    for key in required))


def fixture(root):
    client = root / "client"
    client.mkdir()
    # The sole precondition is the user's persisted intent to enable this node.
    # No node database, identity, ready file or server exists before timing starts.
    with closing(sqlite3.connect(client / "client.db")) as db:
        db.execute("CREATE TABLE cache(node TEXT,key TEXT,value TEXT NOT NULL,PRIMARY KEY(node,key))")
        db.execute("INSERT INTO cache VALUES ('device','local-node-enabled','true')")
        db.commit()
    return client


def owned_processes(app, root):
    prefix = str(app / "Contents") + "/"
    result = {}
    for line in subprocess.check_output(["ps", "-axo", "pid=,command="], text=True).splitlines():
        fields = line.strip().split(None, 1)
        if len(fields) == 2 and fields[1].startswith(prefix) and str(root) in fields[1]:
            result[int(fields[0])] = fields[1]
    return result


def cleanup(process, app, root):
    if process and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
    for sig, seconds in ((signal.SIGTERM, 5), (signal.SIGKILL, 2)):
        for pid, command in owned_processes(app, root).items():
            if owned_processes(app, root).get(pid) == command:
                try:
                    os.kill(pid, sig)
                except ProcessLookupError:
                    pass
        end = time.monotonic() + seconds
        while owned_processes(app, root) and time.monotonic() < end:
            time.sleep(.02)
    if owned_processes(app, root):
        raise RuntimeError(f"Fixture processes survived cleanup; retained data: {root}")


def launch(app, root, client, output, case, trace_startup=False, *, mesh=False,
           client_mesh=False, peer=None, marker=None):
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    token = os.urandom(32).hex()
    env = {key: value for key, value in os.environ.items() if not key.startswith("ZORK_")}
    env.update(ZORK_CLIENT_DATA=str(client), ZORK_GUI_PREFERENCES_PATH=str(root / "preferences.json"),
               ZORK_REGISTRY_DIR=str(root / "registry"), ZORK_GUI_LOCALE="zh-CN")
    main_marker = output / f"{case}-gui-main.ns"
    main_marker.unlink(missing_ok=True)
    env["ZORK_GUI_MAIN_NS_FILE"] = str(main_marker)
    if trace_startup:
        trace = output / f"{case}-trace"
        trace.mkdir(exist_ok=True)
        env["ZORK_STARTUP_TRACE"] = str(trace)
    node = client / "node"
    gui = app / "Contents/MacOS/zork-gui"
    sample = {"case": case, "milestones_ms": {}}
    clicked = False
    source_log = node / "desktop-node.log"
    log_offset = source_log.stat().st_size if source_log.exists() else 0

    def milestone(name):
        if name not in sample["milestones_ms"]:
            sample["milestones_ms"][name] = (monotonic_ns() - started_ns) / 1_000_000

    def ui_ready():
        nonlocal clicked
        snapshot = request(port, "/v1/elements", token)
        milestone("gui_api_answering")
        if snapshot.get("elements"):
            milestone("first_rendered_elements")
        if peer is not None:
            if not clicked:
                entry = next((e for e in snapshot.get("elements", [])
                              if e["id"].startswith("chat-" + peer.origin + "-")
                              and "Smoke Leader" in e["label"]
                              and e["visible"] and e["enabled"]), None)
                if entry is None:
                    return False
                reply = request(port, "/v1/actions", token,
                                {"type": "click", "target": {"element_id": entry["id"]}})
                clicked = reply.get("accepted") is True
                if clicked:
                    milestone("remote_chat_input_acknowledged")
                return False
            return visible(snapshot, "composer-input")
        if not clicked:
            # This control appears in the restored local workspace. A listener
            # or an empty management window is insufficient evidence.
            if not visible(snapshot, "desktop-manage"):
                return False
            milestone("local_workspace_visible")
            reply = request(port, "/v1/actions", token,
                            {"type": "click", "target": {"element_id": "desktop-manage"}})
            clicked = reply.get("accepted") is True
            if clicked:
                milestone("input_acknowledged")
            return False
        # The next real rendered frame must reflect the input's visible effect.
        return visible(snapshot, "desktop-return")

    def node_ready():
        config = json.loads((node / "config.json").read_text())
        milestone("local_config_available")
        if (node / "zork.pid").exists():
            milestone("supervisor_pid_observed")
        runtime_port = int(config["bind"]["runtime"].rsplit(":", 1)[1])
        agent_port = int(config["bind"]["agent"].rsplit(":", 1)[1])
        pid = int((node / "run/zork-station.pid").read_text())
        milestone("station_ready_file_observed")
        supervisor = json.loads(control(node, "status"))
        station = request(runtime_port, "/readyz")
        agent = request(agent_port, "/readyz")
        node_token = config.get("admin", {}).get("token")
        if not node_token:
            node_token = json.loads((node / "run/node-token.json").read_text())
        info = request(runtime_port, "/v1/node/status", node_token)
        validate_node(node, supervisor, station, agent, info, pid)
        request(runtime_port, "/v1/tasks", node_token)
        return True

    def station_mesh_ready():
        config = json.loads((node / "config.json").read_text())
        pid = int((node / "run/zork-station.pid").read_text())
        if int((node / "run/zork-mesh.pid").read_text()) != pid:
            return False
        runtime_port = int(config["bind"]["runtime"].rsplit(":", 1)[1])
        node_token = config.get("admin", {}).get("token") or json.loads(
            (node / "run/node-token.json").read_text())
        identity = request(runtime_port, "/v1/node/mesh", node_token).get("origin")
        status = request(runtime_port, "/v1/mesh")
        if not (isinstance(identity, str) and identity.startswith("key:")
                and status.get("enabled") is True and status.get("origin") == identity):
            return False
        sample["mesh_origin"] = identity
        return True

    def client_mesh_ready():
        ready = json.loads((client / "transport/client-mesh-ready.json").read_text())
        origin = ready.get("origin")
        if not (ready.get("pid") == process.pid and ready.get("embedded") is True
                and isinstance(origin, str) and origin.startswith("key:")
                and (peer is None or origin == peer.client_origin)):
            return False
        sample["client_origin"] = origin
        return True

    def remote_connected():
        snapshot = request(port, "/v1/elements", token)
        return any(e["visible"] and "Smoke Remote" in e["label"]
                   and "直连" in e["label"]
                   for e in snapshot.get("elements", []))

    def remote_synced():
        return clicked and cached_message(client, peer.origin, peer.session, marker)

    process = None
    with (output / f"{case}-launcher.log").open("wb") as log:
        try:
            # Retain spawn-to-main time for diagnosis; readiness uses GUI main.
            with ThreadPoolExecutor(max_workers=6) as pool:
                started_ns = monotonic_ns()
                sample["clock_monotonic_ns"] = started_ns
                process = subprocess.Popen([str(gui), "--dev", "--dev-port", str(port),
                                            "--dev-token", token], env=env, stdout=log, stderr=log)
                milestone("gui_spawn_returned")
                main_ns = wait_for_main(main_marker, process, started_ns)
                sample["pre_main_ms"] = (main_ns - started_ns) / 1_000_000
                ui = pool.submit(observe, ui_ready, process, main_ns)
                local = pool.submit(observe, node_ready, process, main_ns)
                station_mesh = pool.submit(observe, station_mesh_ready, process, main_ns) if mesh else None
                client_mesh_result = pool.submit(observe, client_mesh_ready, process, main_ns) if client_mesh or peer else None
                connection = pool.submit(observe, remote_connected, process, main_ns) if peer else None
                sync = pool.submit(observe, remote_synced, process, main_ns) if peer else None
                sample["ui_interactive"], sample["local_node"] = ui.result(), local.result()
                if station_mesh:
                    sample["station_mesh"] = station_mesh.result()
                if client_mesh_result:
                    sample["client_mesh"] = client_mesh_result.result()
                if peer:
                    sample["remote_connection"] = connection.result()
                    sample["remote_sync"] = sync.result()
                sample["gui_alive"] = process.poll() is None
            if trace_startup and sample["gui_alive"]:
                # Diagnostic pixels are collected after both timed observations.
                # They do not replace the real interaction/readiness assertions.
                try:
                    with closing(http.client.HTTPConnection("127.0.0.1", port, timeout=3)) as connection:
                        connection.request("GET", "/v1/screenshot", headers={"Authorization": "Bearer " + token})
                        response = connection.getresponse()
                        pixels = response.read()
                        if response.status != 200 or not pixels.startswith(b"\x89PNG\r\n\x1a\n"):
                            raise RuntimeError(f"screenshot: HTTP {response.status}")
                        path = output / f"{case}.png"
                        path.write_bytes(pixels)
                        sample["screenshot"] = str(path)
                except (OSError, RuntimeError, http.client.HTTPException) as error:
                    sample["screenshot_error"] = str(error)
        finally:
            # Preserve the independent remote Station while stopping only this
            # GUI and its locally owned Station/supervisor generation.
            cleanup(process, app, node)
            source = node / "desktop-node.log"
            if source.exists():
                with source.open("rb") as source:
                    source.seek(log_offset)
                    (output / f"{case}-node.log").write_bytes(source.read())
            if trace_startup:
                marks = []
                for path in trace.glob("startup-*.jsonl"):
                    for line in path.read_text().splitlines():
                        mark = json.loads(line)
                        elapsed = (mark["ns"] - sample["clock_monotonic_ns"]) / 1_000_000
                        if 0 <= elapsed <= (PRE_MAIN_OBSERVE_SECONDS + OBSERVE_SECONDS + 1) * 1000:
                            marks.append({"pid": mark["pid"], "mark": mark["mark"], "ms": elapsed})
                sample["process_marks"] = sorted(marks, key=lambda value: value["ms"])
    return sample


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def local_startup(app, mesh_image, output, trace_startup=False, mesh_binaries=None):
    # Short Unix socket paths are part of the fixture, independent of checkout location.
    root = Path(tempfile.mkdtemp(prefix="zg-", dir="/tmp")).resolve()
    samples = []
    remote = None
    evidence = output / "samples.jsonl"
    evidence.unlink(missing_ok=True)

    def record(sample):
        sample["passed"] = gate_passes(sample, GATES["local-startup"]["budget_ms"])
        samples.append(sample)
        line = json.dumps({key: value for key, value in sample.items() if key != "process_marks"},
                          ensure_ascii=False)
        with evidence.open("a") as report:
            report.write(line + "\n")
        print(line, flush=True)

    try:
        client = fixture(root)
        for case in ("first-initialization", "restart-1", "restart-2"):
            record(launch(app, root, client, output, case, trace_startup))

        node = client / "node"
        config = json.loads((node / "config.json").read_text())
        if config["mesh"]["offline"]:
            raise RuntimeError("Critical Mesh fixture must use normal network configuration")
        config["mesh"].update(enabled=True, name="Smoke Local",
                              bind=f"127.0.0.1:{unused_port(socket.SOCK_DGRAM)}")
        (node / "config.json").write_text(json.dumps(config))
        first_mesh = launch(app, root, client, output, "mesh-first-initialization",
                            trace_startup, mesh=True)
        record(first_mesh)

        remote = RemoteStation(mesh_image, root, output, config, mesh_binaries)
        remote.start("bootstrap")
        remote.stop()
        mesh_port = int(remote.config["mesh"]["bind"].rsplit(":", 1)[1])
        save_remote_node(client, remote.origin, f"{remote.address}:{mesh_port}")
        remote.start("unpaired")

        setup = launch(app, root, client, output, "client-mesh-first-initialization",
                       trace_startup, mesh=True, client_mesh=True)
        record(setup)
        (output / "mesh-setup.json").write_text(json.dumps({
            "local_origin": first_mesh.get("mesh_origin"),
            "remote_origin": remote.origin,
            "remote_address": remote.address,
            "remote_network": remote.network,
            "client_origin": setup["client_origin"],
            "offline": False,
        }, indent=2) + "\n")
        remote.trust_client(setup["client_origin"])
        for case in ("paired-restart-1", "paired-restart-2"):
            marker = remote.post_source_marker()
            if cached_message(client, remote.origin, remote.session, marker):
                raise RuntimeError("Remote source marker reached the client before timed launch")
            sample = launch(app, root, client, output, case, trace_startup,
                            mesh=True, peer=remote, marker=marker)
            sample["remote_origin"] = remote.origin
            sample["remote_address"] = remote.address
            sample["remote_network_driver"] = "bridge"
            sample["remote_marker"] = marker
            record(sample)
        return samples
    finally:
        if remote is not None:
            remote.close()
        if owned_processes(app, root):
            raise RuntimeError(f"Fixture still in use; retained: {root}")
        shutil.rmtree(root)


def local_startup_gate(args, output):
    report = {"passed": False, "status": "unverified",
              "build_profile": args.build_profile or "unspecified",
              "startup_tracing": args.trace_startup,
              "launch": "fresh processes via packaged GUI executable; OS file cache uncontrolled",
              "measurement": "GUI main to real UI input, local runtime/Mesh, connected independent Station and new remote source message in client cache; pre-main OS time reported separately"}
    locks = ExitStack()
    try:
        if not args.app:
            raise RuntimeError("Missing --app: a freshly built and signed macOS app is required")
        binaries = getattr(args, "mesh_bin_dir", None)
        reference = args.mesh_image
        if binaries:
            from smoke_mesh_artifact import exclusive, lock_path, validate
            binaries = binaries.resolve(strict=True)
            locks.enter_context(exclusive(lock_path(binaries)))
            record = validate(ROOT, binaries)
            reference = reference or record["runtime_image"]
            report["mesh_binaries"] = record
        if not reference:
            raise RuntimeError("Missing --mesh-bin-dir (or legacy --mesh-image)")
        if sys.platform != "darwin":
            raise RuntimeError("Native macOS display session required")
        image = json.loads(subprocess.run(["docker", "image", "inspect", reference],
                                          check=True, capture_output=True, text=True).stdout)[0]
        if image["Os"] != "linux" or image["Architecture"] != "arm64":
            raise RuntimeError("Remote Station image must be Linux arm64 for this Studio")
        revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        labels = image["Config"]["Labels"] or {}
        if not binaries and labels.get("org.opencontainers.image.revision") != revision:
            raise RuntimeError("Remote Station image must be built from this checkout revision")
        report["mesh_image"] = {"reference": reference, "id": image["Id"]}
        if not binaries:
            report["mesh_image"]["revision"] = revision
        app = args.app.resolve(strict=True)
        paths = ("Contents/MacOS/zork-gui", "Contents/Helpers/ZorkSupervisor.app/Contents/MacOS/zork",
                 "Contents/Helpers/ZorkStation.app/Contents/MacOS/zork-station")
        stamps = {path: (app / path).stat() for path in paths}
        report["samples"] = local_startup(app, image["Id"], output, args.trace_startup, binaries)
        if binaries:
            validate(ROOT, binaries)
        # Do not read every executable into the file cache before measuring.
        for path, before in stamps.items():
            after = (app / path).stat()
            if (before.st_ino, before.st_mtime_ns, before.st_size) != (after.st_ino, after.st_mtime_ns, after.st_size):
                raise RuntimeError("App changed during measurement")
        report["binaries"] = {path: digest(app / path) for path in paths}
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
        report["passed"] = bool(report["samples"]) and all(sample["passed"] for sample in report["samples"])
        report["status"] = "passed" if report["passed"] else "failed"
    except Exception as error:
        report["error"] = str(error)
        evidence = output / "samples.jsonl"
        if evidence.exists():
            report["samples"] = [json.loads(line) for line in evidence.read_text().splitlines()]
    finally:
        locks.close()
    return report


def client_frame_gate(args, output):
    if not args.frame_binary:
        raise RuntimeError("Missing --frame-binary: freshly built release native client frame benchmark required")
    if args.frame_host:
        return run_remote_frame(args.frame_host, args.frame_binary.resolve(strict=True), output, ROOT)
    result_path = output / "result.json"
    result_path.unlink(missing_ok=True)
    with (output / "runner.log").open("w") as log:
        process = subprocess.run([
            sys.executable, str(ROOT / "scripts/test-client-frame-budget.py"),
            "--binary", str(args.frame_binary.resolve()), "--output", str(output),
        ], cwd=ROOT, stdout=log, stderr=subprocess.STDOUT)
    if not result_path.is_file():
        raise RuntimeError(f"Client benchmark exited {process.returncode} without evidence; see {output / 'runner.log'}")
    report = json.loads(result_path.read_text())
    if process.returncode != 0 and report.get("passed"):
        raise RuntimeError("Client benchmark exit code disagrees with its report")
    return report


RUNNERS = {"local-startup": local_startup_gate, "client-frame": client_frame_gate}


def run_gates(args, output):
    if set(RUNNERS) != set(GATES):
        raise RuntimeError("Every approved gate must have exactly one smoke implementation")
    results = {}
    for name, run in RUNNERS.items():
        gate_output = output / name
        gate_output.mkdir(exist_ok=True)
        try:
            result = run(args, gate_output)
            if (not isinstance(result, dict) or result.get("status") not in ("passed", "failed", "unverified")
                    or type(result.get("passed")) is not bool
                    or result["passed"] != (result["status"] == "passed")):
                raise RuntimeError("Smoke returned an inconsistent result")
            results[name] = result
        except Exception as error:
            results[name] = {"passed": False, "status": "unverified", "error": str(error)}
        print(f"{results[name]['status'].upper()} {name}", flush=True)
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list", action="store_true", help="Show the user-approved gate definitions")
    parser.add_argument("--app", type=Path, help="App freshly built and packaged by this task")
    parser.add_argument("--frame-binary", type=Path, help="Fresh release zork-gui-render-bench with native-blur-bench")
    parser.add_argument("--frame-host", help="Run the unchanged native frame gate on a separate macOS SSH host")
    parser.add_argument("--mesh-image", help="Current-source Linux arm64 Station image on a bridge network")
    parser.add_argument("--mesh-bin-dir", type=Path,
                        help="Verified Linux Station/Agent from scripts/build/critical-mesh.py; mounted read-only")
    parser.add_argument("--build-profile", choices=("dev", "release"),
                        help="Profile used to build the supplied app; never changes the gate budget")
    parser.add_argument("--trace-startup", action="store_true",
                        help="Collect opt-in process startup milestones; budget is unchanged")
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/critical-gates")
    args = parser.parse_args()
    if args.list:
        print(json.dumps(GATES, ensure_ascii=False, indent=2))
        return 0
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    report = {"gates": GATES, "platform": platform.platform(), "machine": platform.machine(),
              "passed": False}
    try:
        report["checkout_head"] = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
        report["checkout_dirty"] = bool(subprocess.check_output(
            ["git", "status", "--porcelain"], cwd=ROOT, text=True).strip())
        report["results"] = run_gates(args, output)
        report["passed"] = bool(report["results"]) and all(
            result.get("passed") is True and result.get("status") == "passed"
            for result in report["results"].values())
    except Exception as error:
        report["error"] = str(error)
    (output / "result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(f"{'PASS' if report['passed'] else 'FAIL'} critical gates: {output / 'result.json'}", flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
