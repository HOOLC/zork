#!/usr/bin/env python3
"""Interactive multi-turn real-LLM device/MCP/skill acceptance on isolated Gateways.

Build current binaries first. Pass an existing profile and exact model explicitly.
The model receives ordinary user requests; the harness only prepares fixtures and
checks independent filesystem, MCP and durable-event evidence. Credentials are
never written to reports, and temporary profile copies are removed on every exit.
"""
import argparse
import importlib.util
import json
import os
import re
from pathlib import Path
import secrets
import shutil
import sqlite3
import subprocess
import tempfile
import time
import traceback

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("mcp_fixture", ROOT / "scripts/test-mcp.py")
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)
f = m.f


def ok(response):
    assert response[0] in (200, 201, 202), response
    return response[1]


def events(node, session):
    result = []
    for path in sorted((node.root / "shared-files/sessions" / session / "segments").glob("*.jsonl")):
        for line in path.read_text().splitlines():
            try:
                result.append(json.loads(line)["event"])
            except json.JSONDecodeError:
                pass  # Writer may be in the middle of its next durable append.
    return result


class LiveNode(f.Node):
    def start(self):
        self.log = (self.root / "supervisor.log").open("ab")
        self.process = subprocess.Popen(
            [str(f.TARGET / "zork"), "start", "--data", str(self.root)],
            stdout=self.log, stderr=self.log, start_new_session=True,
        )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", type=Path, required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--thinking")
    parser.add_argument("--context-tokens", type=int)
    parser.add_argument("--output-tokens", type=int)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--phase-timeout", type=int, default=900)
    parser.add_argument("--commands", type=Path, required=True)
    args = parser.parse_args()
    profile = json.loads(args.profile.read_text())
    model = next(v for v in profile["models"] if v["id"] == args.model)
    thinking = args.thinking or model.get("default_thinking", "off")
    if not model.get("limits"):
        assert args.context_tokens and args.output_tokens, "profile needs explicit model limits"
        model["limits"] = {"context_window_tokens": args.context_tokens, "max_output_tokens": args.output_tokens}
    # Existing access credentials suffice for this bounded run; do not rotate the
    # original account's refresh token from an isolated test profile.
    if profile.get("auth", {}).get("type") == "oauth":
        expiry = profile["auth"].get("expires", 0)
        assert expiry > time.time() * 1000 + args.phase_timeout * 3000 + 300000, "subscription access expires too soon"
        profile["auth"].pop("refresh", None)
    redactions = [str(v) for k, v in profile.get("auth", {}).items()
                  if k in ("access", "refresh", "key", "email") and v]

    def redact(value):
        text = value if isinstance(value, str) else json.dumps(value, ensure_ascii=False, indent=2)
        for secret in redactions:
            text = text.replace(secret, "[REDACTED]")
        return text

    args.output.mkdir(parents=True, exist_ok=True)
    scratch = Path(tempfile.mkdtemp(prefix="zork-node-tools-live-"))
    os.chmod(scratch, 0o700)
    nodes, sessions, checks, phases = [], [], [], []
    report = {"model": args.model, "thinking": thinking, "provider": profile.get("provider"),
              "billing": profile.get("billing"), "fake_agent": False, "model_limits": model["limits"],
              "topology": "two isolated Gateways on one host", "phases": phases, "checks": checks}
    failure = None
    counter = 0
    started = time.monotonic()
    print(json.dumps({"fixture": str(scratch), "model": args.model, "thinking": thinking}), flush=True)
    try:
        a = LiveNode(scratch / "coordinator"); nodes.append(a)
        b = LiveNode(scratch / "test-device"); nodes.append(b)
        a.pair(b); b.pair(a)
        for node in nodes:
            node.config["mesh"]["peers"][0]["client"] = True
            node.config["mesh"]["name"] = node.root.name
            node.config["admin"] = {"token": "mcp-fixture"}
            for workspace in node.config["mesh"]["workspaces"]:
                workspace.update(profile_id="live", model=args.model, thinking=thinking)
            (node.root / "profiles/fixture.json").unlink()
            path = node.root / "profiles/live.json"
            path.write_text(json.dumps(profile)); path.chmod(0o600)
            (node.root / "config.json").write_text(json.dumps(node.config))
            node.request = lambda method, path, body=None, n=node: m.request(n, method, path, body)
            node.start()
        for node in nodes:
            f.wait(lambda n=node: n.request("GET", "/readyz")[0] == 200, "ready")
            f.wait(lambda n=node: n.get("/v1/mesh").get("origin") == n.origin, "Mesh identity")
        created = ok(a.request("POST", "/v1/im/sessions", {
            "profile_id": "live", "model": args.model, "thinking": thinking, "workspace": str(a.workspace)}))
        coordinator = created["session_id"]; sessions.append((a, coordinator))
        seed = b.workspace / "seed"; seed.mkdir()
        (seed / "echo-server.py").write_text(m.FIXTURE)
        proof = "skill-proof-" + secrets.token_hex(8)
        guide = seed / "guide"; (guide / "scripts").mkdir(parents=True)
        (guide / "SKILL.md").write_text(
            "---\nname: live-echo-guide\ndescription: Process live acceptance echo requests using the installed live-echo MCP and its packaged helper.\n---\n"
            "For an echo acceptance request, run scripts/helper.py from this skill directory with the user's input as one argument. "
            "The helper adds the installation proof tag; do not invent the tag. Find the live-echo MCP, inspect echo, "
            "call it with the helper output as text, await completion, and report the exact returned text.\n")
        (guide / "scripts/helper.py").write_text(f"import sys\nprint({proof!r} + ':' + sys.argv[1])\n")
        old = b.workspace / "existing-guide"; old.mkdir()
        (old / "SKILL.md").write_text("---\nname: existing-guide\ndescription: Existing skill that must survive management operations\n---\nPreserve this source.\n")
        ok(b.request("POST", "/v1/node/agents", {"id": "research", "name": "Research", "role": "leader",
            "profile_id": "live", "model": args.model, "thinking": thinking, "skill_paths": [str(old)]}))
        researcher = ok(b.request("POST", "/v1/node/agents/research/open", {}))["session_id"]
        sessions.append((b, researcher))

        (seed / "README.md").write_text(
            "# 回声工具测试材料\n\n"
            "echo-server.py 是 stdio MCP 服务，用 Python 3 运行。启动参数为程序路径和调用日志路径。"
            "它的 echo 工具返回输入 text，支持 delay 秒延迟。建议安装名 live-echo。\n"
            "guide 是配套操作手册，带 scripts/helper.py。安装手册后应绑定给需要的助手。\n"
            "slow-report.py 是耗时测试作业。运行后打印 REPORT-STARTED，约三分钟后写 report.txt。"
            "每次启动都会在 attempts.jsonl 记一行；中断后无需重新运行。\n")
        (seed / "slow-report.py").write_text(
            "import json,time,os\nfrom pathlib import Path\n"
            "p=Path(__file__).parent\n"
            "with (p/'attempts.jsonl').open('a') as f:f.write(json.dumps({'pid':os.getpid()})+'\\n')\n"
            "print('REPORT-STARTED',flush=True)\ntime.sleep(180)\n"
            "(p/'report.txt').write_text('report complete')\nprint('REPORT-DONE',flush=True)\n")
        report["fixture"] = {"coordinator_workspace":str(a.workspace), "test_workspace":str(b.workspace)}
        print(json.dumps({"ready":True, **report["fixture"]}), flush=True)
        consumed = 0
        seen = set()
        while time.monotonic() - started < args.phase_timeout * 3:
            commands = args.commands.read_text().splitlines() if args.commands.exists() else []
            stop = False
            for line in commands[consumed:]:
                command = json.loads(line)
                consumed += 1
                if command.get("stop"):
                    stop = True
                    break
                node, sid = (b, researcher) if command.get("to") == "research" else (a, coordinator)
                counter += 1
                prompt = command["content"]
                ok(node.request("POST", f"/v1/im/sessions/{sid}/messages", {"content":prompt,"request_id":f"human-{counter}"}))
                phases.append({"number":counter,"to":command.get("to","coordinator"),"content":prompt,"sent_at_ms":int(time.time()*1000)})
                print(json.dumps({"sent":counter,"to":command.get("to","coordinator")}),flush=True)
            snapshot = {"messages":phases,"sessions":{}}
            for node, sid in sessions:
                stream = events(node,sid)
                public = []
                for event in stream:
                    if event["kind"] == "step_completed":
                        for inv in event.get("invocations",[]):
                            if inv["tool"] == "chat.post_message":
                                public.append(inv["arguments"])
                    if event["kind"] == "tool_result":
                        result=event["result"]
                        if result["invocation_id"] not in seen:
                            seen.add(result["invocation_id"])
                            print(json.dumps({"node":node.root.name,"tool":result["tool"],"outcome":result["outcome"]}),flush=True)
                active = next((e for e in reversed(stream) if e["kind"] in ("turn_started","turn_finished")),{})
                snapshot["sessions"][node.root.name] = {"public_messages":public,"last_turn_event":active,"recent_events":stream[-8:]}
                (args.output/f"{node.root.name}-{sid}-events.json").write_text(redact(stream))
            (args.output/"live.json").write_text(redact(snapshot))
            if stop:
                break
            time.sleep(1)
        else:
            raise AssertionError("interactive run timed out")
        def query(name, arguments):
            nonlocal counter
            counter += 1
            return ok(a.request("POST","/v1/node-tools",{"session_id":coordinator,"invocation_id":f"oracle-{counter}","tool":name,"arguments":arguments}))
        oracle = {"skill_files":{n.root.name:[str(p.relative_to(n.root)) for p in (n.root/"skills").rglob("SKILL.md") if ".zork" not in p.parts] for n in nodes}}
        for n in nodes:
            with sqlite3.connect(n.root/"state/gateway.sqlite") as db:
                oracle[n.root.name+"_agents"]=[json.loads(v[0]) for v in db.execute("SELECT value FROM node_agents")]
        oracle["attempts"] = (seed/"attempts.jsonl").read_text() if (seed/"attempts.jsonl").exists() else None
        oracle["report_exists"]=(seed/"report.txt").exists()
        oracle["old_source_exists"]=(old/"SKILL.md").exists()
        report["oracle"]=oracle
        report["outcome"]="completed_for_review"
    except BaseException as error:
        failure = error
        report["outcome"] = "failed"
        report["error"] = redact(traceback.format_exc())
        print(redact(traceback.format_exc()), flush=True)
    finally:
        for node in reversed(nodes):
            node.stop()
        all_events = []
        for node, sid in sessions:
            stream = events(node, sid)
            all_events.extend(stream)
            (args.output / f"{node.root.name}-{sid}-events.json").write_text(redact(stream))
        usage = [e["usage"] for e in all_events if e["kind"] == "step_completed" and e.get("usage")]
        report["usage"] = {key: sum(v.get(key) or 0 for v in usage) for key in
                           ("input_tokens", "output_tokens", "cached_input_tokens", "output_reasoning_tokens")}
        report["model_steps"] = len(usage)
        report["duration_seconds"] = round(time.monotonic() - started, 2)
        report["binaries"] = {p.name: subprocess.check_output(["shasum", "-a", "256", str(p)], text=True).split()[0]
                              for p in (f.TARGET / "zork", f.TARGET / "zork-station")}
        (args.output / "report.json").write_text(redact(report))
        shutil.rmtree(scratch)
        print(json.dumps({"outcome": report["outcome"], "report": str(args.output / "report.json"), "model_steps": len(usage)}), flush=True)
    if failure:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
