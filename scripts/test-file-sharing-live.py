#!/usr/bin/env python3
"""Opt-in real-model file placement, sharing and Chat attachment acceptance.

Use a freshly built Station and an explicit existing profile/model. Two isolated
offline Mesh nodes use synthetic files; only the writer calls the real provider.
The harness checks filesystem, tool events and peer reads independently of the
model's final answer. Temporary credentials and nodes are removed on exit.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import traceback
import uuid
from urllib.parse import urlsplit
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("channels", ROOT / "scripts/test-chat-channels.py")
channels = importlib.util.module_from_spec(spec)
spec.loader.exec_module(channels)
fixture = channels.fixture


class Node(fixture.Node):
    def start(self, fake=False):
        self.log = (self.root / "station.log").open("ab")
        self.process = subprocess.Popen(
            [str(fixture.TARGET / "zork-station"), "--data", str(self.root)]
            + (["--fake-agent"] if fake else []),
            stdout=self.log, stderr=self.log, start_new_session=True,
        )


def events(node, session):
    result = []
    for path in sorted((node.root / "shared-files/sessions" / session / "segments").glob("*.jsonl")):
        for line in path.read_text().splitlines():
            try:
                result.append(json.loads(line)["event"])
            except json.JSONDecodeError:
                pass
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", type=Path, required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--thinking")
    parser.add_argument("--context-tokens", type=int)
    parser.add_argument("--output-tokens", type=int)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--phase-timeout", type=int, default=300)
    args = parser.parse_args()
    profile = json.loads(args.profile.read_text())
    model = next(m for m in profile["models"] if m["id"] == args.model)
    thinking = args.thinking or model.get("default_thinking", "off")
    if not model.get("limits"):
        assert args.context_tokens and args.output_tokens, "provide missing model limits"
        model["limits"] = {"context_window_tokens": args.context_tokens,
                           "max_output_tokens": args.output_tokens}
    if profile.get("auth", {}).get("type") == "oauth":
        assert profile["auth"].get("expires", 0) > time.time() * 1000 + args.phase_timeout * 3000 + 300000, "access expires too soon"
        # An isolated test must never rotate the original refresh credential.
        profile["auth"].pop("refresh", None)
    secrets = [str(value) for key, value in profile.get("auth", {}).items()
               if key in ("access", "refresh", "key", "email") and value]

    def redact(value):
        text = value if isinstance(value, str) else json.dumps(value, ensure_ascii=False, indent=2)
        for secret in secrets:
            text = text.replace(secret, "[REDACTED]")
        return text

    args.output.mkdir(parents=True, exist_ok=True)
    scratch = Path(tempfile.mkdtemp(prefix="zork-files-live-"))
    scratch.chmod(0o700)
    previous_registry = os.environ.get("ZORK_REGISTRY_DIR")
    os.environ["ZORK_REGISTRY_DIR"] = str(scratch / "registry")
    nodes, phases, checks = [], [], []
    session = None
    started = time.monotonic()
    report = {"model": args.model, "thinking": thinking, "provider": profile["provider"],
              "provider_host": urlsplit(profile.get("base_url", "")).hostname,
              "fake_agent": False, "topology": "two isolated Stations on one host",
              "limits": model["limits"], "phases": phases, "checks": checks}
    failed = False
    try:
        a = Node(scratch / "writer"); nodes.append(a)
        b = Node(scratch / "reader"); nodes.append(b)
        for node, other in ((a, b), (b, a)):
            node.pair(other)
            node.config["admin"] = {"token": channels.TOKEN}
            node.config["mesh"]["name"] = node.root.name
            node.config["mesh"]["peers"][0].update(client=True, collaborate=True)
            (node.root / "config.json").write_text(json.dumps(node.config))
        path = a.root / "profiles/live.json"
        path.write_text(json.dumps(profile)); path.chmod(0o600)
        a.start(); b.start(fake=True)
        for node in nodes:
            fixture.wait(lambda n=node: channels.request(n, "GET", "/readyz")[0] == 200, "Station ready")
            fixture.wait(lambda n=node: channels.request(n, "GET", "/v1/node/shared-files")[0] == 200, "shared source ready")
        channels.ok(a, "POST", "/v1/node/agents", {"id": "writer", "name": "File assistant", "role": "leader",
            "profile_id": "live", "model": args.model, "thinking": thinking,
            "instructions": "这是隔离验收环境。只处理当前任务的工作文件、共享文件和当前聊天；不要读取或修改凭据、数据库或节点配置。"})
        opened = channels.ok(a, "POST", "/v1/node/agents/writer/open", {})
        chat, session = opened["chat_id"], opened["agent"]["session_id"]
        catalog = channels.ok(a, "GET", "/v1/node/agents/writer/skills")["catalog"]
        guide = next(s for s in catalog["skills"] if s["name"] == "file-sharing")
        report["skill"] = {"name": guide["name"], "content_hash": guide["content_hash"]}
        assert not list((a.root / "shared").iterdir())

        def run_phase(label, prompt):
            before = len(events(a, session))
            phase = {"name": label, "prompt": prompt}; phases.append(phase)
            start = time.monotonic()
            channels.ok(a, "POST", f"/v1/im/sessions/{chat}/messages", {
                "content": prompt, "request_id": str(uuid.uuid4())})
            print("START " + label, flush=True)
            seen = set()
            while time.monotonic() - start < args.phase_timeout:
                current = events(a, session)[before:]
                for event in current:
                    if event["kind"] == "tool_result":
                        result = event["result"]
                        if result["invocation_id"] not in seen:
                            seen.add(result["invocation_id"])
                            print(json.dumps({"phase": label, "tool": result["tool"], "outcome": result["outcome"]}), flush=True)
                terminal = next((e for e in reversed(current) if e["kind"] == "turn_finished"), None)
                if terminal:
                    phase.update(outcome=terminal["outcome"], duration_seconds=round(time.monotonic() - start, 2))
                    phase["tools"] = [e["result"]["tool"] for e in current if e["kind"] == "tool_result"]
                    phase["failures"] = [e["result"] for e in current if e["kind"] == "tool_result" and e["result"]["outcome"] != "succeeded"]
                    assert terminal["outcome"] == "finished" and not terminal.get("outstanding"), terminal
                    assert any(e["kind"] == "step_completed" and e.get("usage") for e in current), "no real provider usage"
                    assert any(e["kind"] == "tool_result" and e["result"]["tool"] == "chat.send"
                               and e["result"]["outcome"] == "succeeded" for e in current), "no user-visible Chat reply"
                    return current
                time.sleep(1)
            raise AssertionError("real model timed out: " + label)

        draft = run_phase("ordinary_file_stays_in_workspace",
            "三笔订单金额分别为 A：17 元、B：23 元、C：41 元。计算合计，生成一份简短的 summary.txt，包含各笔金额和合计，并告诉我保存的位置。")
        # Opening a home Chat allocates identity; first input starts its runtime.
        workspace = Path(channels.sql(a, "SELECT workspace_path FROM sessions WHERE id=?", (session,))[0][0])
        (workspace / "orders.csv").write_text("item,amount\nA,17\nB,23\nC,41\n")
        (workspace / "scratch-notes.txt").write_text("INTERNAL-DRAFT-DO-NOT-PUBLISH\n")
        (workspace / "attachment-only.txt").write_text("CHAT-ATTACHMENT-ONLY\n")
        summary = workspace / "summary.txt"
        data = summary.read_bytes()
        assert b"81" in data and all(str(n).encode() in data for n in (17, 23, 41)), data
        assert not list((a.root / "shared").iterdir()), "ordinary output was automatically shared"
        checks.append("ordinary_deliverable_stays_in_workspace_and_shared_root_is_empty")

        shared = run_phase("explicit_file_sharing",
            "我想在另一台设备的文件共享里看到刚才的 summary.txt，请帮我放进去，保持文件名 summary.txt，并确认可以打开。")
        shared_paths = sorted(str(p.relative_to(a.root / "shared")) for p in (a.root / "shared").rglob("*") if p.is_file())
        assert shared_paths == ["summary.txt"], shared_paths
        assert (a.root / "shared/summary.txt").read_bytes() == data
        invocations = [i for e in draft + shared if e["kind"] == "step_completed" for i in e.get("invocations", [])]
        assert any(i["tool"] == "file.read" and "file-sharing" in str(i["arguments"].get("path", "")) for i in invocations), "model did not read the runtime Skill"
        assert not any(i["tool"] in ("file.locations", "file.share") for i in invocations), "obsolete file tool was exposed"
        reads = [i["arguments"]["path"] for i in invocations if i["tool"] == "file.read" and str(i["arguments"].get("path", "")).startswith("synch://shared/")]
        assert any("root=" in p and "origin=" in p for p in reads), "model did not confirm fixed shared bytes"
        checks.append("model_reads_skill_shares_only_selected_file_with_ordinary_file_operations_and_reads_fixed_reference")

        def remote_entry():
            page = channels.ok(b, "POST", "/v1/node/shared-files/directory", {"space": "shared", "path": "", "origin": a.origin})
            return next((e for e in page["entries"] if e["path"] == "summary.txt"), None)
        entry = fixture.wait(remote_entry, "peer sees shared report")
        selected = entry["selected"]
        req = Request(b.url + "/v1/node/shared-files/content", method="POST",
            headers={"Content-Type": "application/json", "Authorization": "Bearer " + channels.TOKEN},
            data=json.dumps(selected).encode())
        with urlopen(req, timeout=30) as response:
            assert response.read() == data
        report["peer_file"] = selected
        checks.append("second_station_reads_identical_bytes_from_actual_synch_publication")

        attached = run_phase("chat_attachment_is_separate",
            "把当前工作区的 attachment-only.txt 作为文件附件发到当前聊天。")
        sent = [e["result"]["data"] for e in attached if e["kind"] == "tool_result" and e["result"]["tool"] == "chat.send"]
        assert any(any(v.get("name") == "attachment-only.txt" for v in result.get("attachments", [])) for result in sent), sent
        assert sorted(str(p.relative_to(a.root / "shared")) for p in (a.root / "shared").rglob("*") if p.is_file()) == ["summary.txt"]
        assert (workspace / "scratch-notes.txt").read_text() == "INTERNAL-DRAFT-DO-NOT-PUBLISH\n"
        checks.append("chat_attachment_uses_chat_send_without_extra_shared_copy_or_draft_leak")
        (args.output / "summary.txt").write_bytes(data)
        active_catalog = next(e["tools"] for e in events(a, session) if e["kind"] == "session_created")
        names = {tool["name"] for tool in active_catalog}
        forbidden = {"file.locations", "file.share", "chat.search", "chat.recover", "chat.post_page", "agent.interrupt", "agent.message", "agent.recover", "device.agents", "device.exec", "mcp.share", "mcp.installed", "mcp.configure", "mcp.probe", "mcp.enable", "mcp.disable", "service.start", "service.stop", "service.restart", "service.share", "service.unshare", "page.publish", "page.unpublish", "page.deliver", "browser", "android.devices", "provider.login"}
        assert not forbidden.intersection(names) and not any(name.startswith("skill.") for name in names), sorted(names)
        assert "client.browser" in names
        checks.append("runtime_catalog_omits_removed_tools_and_exposes_client_browser")
        report["outcome"] = "passed"
    except Exception:
        failed = True
        report["outcome"] = "failed"
        report["error"] = redact(traceback.format_exc())
        print(report["error"], flush=True)
    finally:
        for node in nodes:
            node.stop()
        stream = events(nodes[0], session) if nodes and session else []
        (args.output / "events.json").write_text(redact(stream))
        for node in nodes:
            log = node.root / "station.log"
            if log.exists():
                (args.output / (node.root.name + ".log")).write_text(redact(log.read_text(errors="replace")))
        catalog = next((e["tools"] for e in stream if e["kind"] == "session_created"), [])
        report["tool_catalog"] = [tool["name"] for tool in catalog]
        report["tool_count"] = len(catalog)
        usage = [e["usage"] for e in stream if e["kind"] == "step_completed" and e.get("usage")]
        report["usage"] = {key: sum(v.get(key) or 0 for v in usage)
                           for key in ("input_tokens", "output_tokens", "cached_input_tokens", "output_reasoning_tokens")}
        report["model_steps"] = len(usage)
        report["duration_seconds"] = round(time.monotonic() - started, 2)
        report["station_sha256"] = hashlib.file_digest((fixture.TARGET / "zork-station").open("rb"), "sha256").hexdigest()
        (args.output / "report.json").write_text(redact(report))
        shutil.rmtree(scratch)
        if previous_registry is None:
            os.environ.pop("ZORK_REGISTRY_DIR", None)
        else:
            os.environ["ZORK_REGISTRY_DIR"] = previous_registry
        print(json.dumps({"outcome": report["outcome"], "report": str(args.output / "report.json"), "model_steps": len(usage)}), flush=True)
    if failed:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
