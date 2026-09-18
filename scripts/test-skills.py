#!/usr/bin/env python3
"""Skill sources through a rebuilt Gateway, isolated files and the fake provider."""
import importlib.util
import json
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
from urllib.error import HTTPError
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("fixture", ROOT / "scripts/test-mesh.py")
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)
fixture.TARGET = Path(os.environ.get("ZORK_TEST_BIN_DIR", str(Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "debug")))


def skill(directory, name, description, body):
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "SKILL.md").write_text(f"---\nname: {name}\ndescription: {description}\n---\n{body}\n")


def request(base, method, path, body=None, token="fixture"):
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = "Bearer " + token
    with urlopen(Request(base + path, method=method, headers=headers,
                         data=None if body is None else json.dumps(body).encode()), timeout=10) as response:
        raw = response.read()
        return json.loads(raw) if raw else None


def rejected(base, method, path, body, expected, token="fixture"):
    try:
        request(base, method, path, body, token)
    except HTTPError as error:
        assert error.code == expected, (error.code, error.read())
    else:
        raise AssertionError(f"{method} {path} accepted invalid request")


def main():
    with tempfile.TemporaryDirectory(prefix="zork-skills-") as temporary:
        root = Path(temporary)
        node = fixture.Node(root / "node")
        node.config["admin"] = {"token": "fixture"}
        node.config["skills"] = {"shared_path": "skills", "paths": ["device-skills"]}
        config_path = node.root / "config.json"
        config_path.write_text(json.dumps(node.config))
        skill(node.root / "skills/build", "build", "Shared build", "SHARED_BODY")
        skill(node.root / "device-skills/build", "build", "Device build", "DEVICE_BODY")
        skill(node.root / "agent-skills/build", "build", "Agent build", "AGENT_BODY")
        skill(node.root / "files/other-device/secret", "secret", "Other device", "NOT_VISIBLE")
        log_path = node.root / "skills.log"
        session_id = None
        for restart in range(2):
            with log_path.open("ab") as log:
                process = subprocess.Popen([str(fixture.TARGET / "zork-station"), "--data", str(node.root), "--fake-agent"], stdout=log, stderr=log)
                try:
                    fixture.wait(lambda: request(node.url, "GET", "/readyz"), "Gateway ready")
                    endpoint = "/v1/node/agents/leader/skills"
                    if restart == 0:
                        selection = {"profile_id": "fixture", "model": "fixture-model", "thinking": "off"}
                        created = request(node.url, "POST", "/v1/node/agents", dict(selection, id="leader", name="Leader", role="leader", skill_paths=["agent-skills"]))
                        assert created["skill_paths"] == ["agent-skills"]
                        rejected(node.url, "GET", endpoint, None, 401, token=None)
                        rejected(node.url, "PUT", endpoint, {"paths": [""]}, 400)
                        rejected(node.url, "PUT", "/v1/node/agents/missing/skills", {"paths": []}, 404)
                        catalog = request(node.url, "GET", endpoint)["catalog"]
                        assert len([s for s in catalog["skills"] if s["name"] == "build"]) == 3, catalog
                        assert any(s["name"] == "skill-management" for s in catalog["skills"])
                        assert any(s["name"] == "file-sharing" for s in catalog["skills"])
                        assert any(s["name"] == "slack" for s in catalog["skills"])
                        assert {s["description"] for s in catalog["skills"] if s["name"] == "build"} == {"Agent build", "Device build", "Shared build"}
                        assert len(catalog["diagnostics"]) == 0
                        opened = request(node.url, "POST", "/v1/node/agents/leader/open", {})
                        chat_id = opened["chat_id"]
                        session_id = opened["agent"]["session_id"]

                        def history():
                            try:
                                return request(node.agent_url, "GET", f"/sessions/{session_id}/history?limit=200")
                            except HTTPError as error:
                                if error.code == 404:
                                    return {"items": []}
                                raise

                        def send(content):
                            import uuid
                            request(node.url, "POST", f"/v1/im/sessions/{chat_id}/messages", {"request_id": str(uuid.uuid4()), "content": content})

                        def read_skill(expected, path):
                            send(json.dumps({"fake_tool": {"name": "file.read", "input": {"path": str(path)}}}))
                            fixture.wait(lambda: expected in json.dumps(history()), "file.read result " + expected)

                        read_skill("AGENT_BODY", node.root / "agent-skills/build/SKILL.md")
                        request(node.url, "PUT", endpoint, {"paths": []})
                        read_skill("DEVICE_BODY", node.root / "device-skills/build/SKILL.md")
                        node.config["skills"]["paths"] = []
                        config_path.write_text(json.dumps(node.config))
                        read_skill("SHARED_BODY", node.root / "skills/build/SKILL.md")
                        snapshot = history()
                        notices = [notice for item in snapshot["items"] for notice in item["event"].get("notices", []) if notice.startswith("Current skill catalog")]
                        assert len(notices) == 3, notices
                        assert "Other device" not in "\n".join(notices)

                        def tool(name, arguments, outcome="succeeded"):
                            before = history()
                            seen = {item["event_id"] for item in before["items"]}
                            send(json.dumps({"fake_tool": {"name": name, "input": arguments}}))
                            def result():
                                snapshot = history()
                                return next((item["event"]["result"] for item in snapshot["items"] if item["event_id"] not in seen and item["event"].get("kind") == "tool_result" and item["event"]["result"]["tool"] == name), None)
                            result = fixture.wait(result, "tool result " + name)
                            assert result["outcome"] == outcome, result
                            return result["data"]

                        guide_path = next(s["path"] for s in request(node.url, "GET", endpoint)["catalog"]["skills"] if s["name"] == "skill-management")
                        assert Path(guide_path).is_file()
                        assert (node.root / "skills/skill-management/SKILL.md").read_bytes() == Path(guide_path).read_bytes()
                        assert (node.root / "skills/service-sharing/SKILL.md").is_file()
                        assert all(not (node.root / "files" / name).exists() for name in ("bundled-skills", "custom-skills", "managed-skills"))
                        guide = tool("file.read", {"path": guide_path})
                        assert guide["content"] == Path(guide_path).read_text()
                        slack_path = next(s["path"] for s in request(node.url, "GET", endpoint)["catalog"]["skills"] if s["name"] == "slack")
                        assert tool("file.read", {"path": slack_path})["content"] == Path(slack_path).read_text()
                        tool("shell.run", {"command": "test \"$SKILLS_ROOT\" = " + shlex.quote(str(node.root / "skills"))})
                        added = tool("skill.sources", {"action": "add", "path": "agent-skills"})
                        assert added["agent_id"] == "leader" and added["agent_paths"] == ["agent-skills"]
                        content = "---\nname: managed\ndescription: Managed skill\n---\nManaged body\n"
                        directory = node.root / "skills/managed"
                        tool("shell.run", {"command": "mkdir -p " + shlex.quote(str(directory))})
                        tool("file.write", {"path": str(directory / "SKILL.md"), "content": content})
                        assert (directory / "SKILL.md").read_text() == content
                        catalog = request(node.url, "GET", endpoint)["catalog"]
                        assert any(s["name"] == "managed" for s in catalog["skills"])
                        tool("file.write", {"path": str(directory / "SKILL.md"), "content": content + "Updated\n"})
                        assert tool("file.read", {"path": str(directory / "SKILL.md")})["content"].endswith("Updated\n")
                        tool("shell.run", {"command": "mv " + shlex.quote(str(directory / "SKILL.md")) + " " + shlex.quote(str(directory / "saved.md"))})
                        assert not any(s["name"] == "managed" for s in request(node.url, "GET", endpoint)["catalog"]["skills"])
                        assert (directory / "saved.md").read_text().endswith("Updated\n")
                    else:
                        catalog = request(node.url, "GET", endpoint)
                        assert catalog["paths"] == []
                        build_skills = [s for s in catalog["catalog"]["skills"] if s["name"] == "build"]
                        assert len(build_skills) == 1 and build_skills[0]["description"] == "Shared build", build_skills
                        assert request(node.agent_url, "GET", f"/sessions/{session_id}")["session_id"] == session_id
                        assert any(s["name"] == "skill-management" for s in catalog["catalog"]["skills"])
                        assert (node.root / "skills/managed/saved.md").read_text().endswith("Updated\n")
                    process.terminate()
                    assert process.wait(timeout=30) == 0
                except Exception:
                    print(log_path.read_text()[-12000:])
                    raise
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait()
    print("PASS: Skill API authorization/validation, ordered sources, live Agent/node updates, ordinary file read/write and shell, preserved resources, durable catalogs and Gateway restart; temporary data cleaned")


if __name__ == "__main__":
    main()
