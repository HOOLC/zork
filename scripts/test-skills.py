#!/usr/bin/env python3
"""Skill sources through a rebuilt Gateway, isolated files and the fake provider."""
import importlib.util
import json
import os
from pathlib import Path
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
        skill(node.root / "other-device/secret", "secret", "Other device", "NOT_VISIBLE")
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
                        guide = tool("file.read", {"path": guide_path})
                        assert guide["content"] == Path(guide_path).read_text()
                        slack_path = next(s["path"] for s in request(node.url, "GET", endpoint)["catalog"]["skills"] if s["name"] == "slack")
                        assert tool("file.read", {"path": slack_path})["content"] == Path(slack_path).read_text()
                        added = tool("skill.sources", {"action": "add", "path": "agent-skills"})
                        assert added["agent_id"] == "leader" and added["agent_paths"] == ["agent-skills"]
                        content = "---\nname: managed\ndescription: Managed skill\n---\nManaged body\n"
                        draft = {"source": str(node.root / "agent-skills"), "directory": "managed", "content": content}
                        saved = tool("skill.write", draft)
                        assert (node.root / "agent-skills/managed/SKILL.md").read_text() == content
                        tool("skill.write", draft, outcome="failed")
                        updated = tool("skill.write", dict(draft, content=content + "Updated\n", expected_hash=saved["content_hash"]))
                        archived = tool("skill.archive", {"source": draft["source"], "directory": "managed", "expected_hash": updated["content_hash"]})
                        assert Path(archived["archived_path"]).read_text().endswith("Updated\n")
                        assert not (node.root / "agent-skills/managed/SKILL.md").exists()
                        removed = tool("skill.sources", {"action": "remove", "path": "agent-skills"})
                        assert removed["agent_paths"] == []
                        bundle = tool("skill.bundle", {"action": "list"})
                        assert "skill-management" in bundle["active"]["skills"]
                        selected_version = bundle["active"]["directory"]
                        tool("skill.bundle", {"action": "rollback", "version": selected_version})
                        guide_entry = next(s for s in request(node.url, "GET", endpoint)["catalog"]["skills"] if s["name"] == "skill-management")
                        failure = tool("skill.write", {"source": guide_entry["source"], "directory": ".", "content": guide["content"], "expected_hash": guide_entry["content_hash"]}, outcome="failed")
                        assert "release-managed" in failure["error"]
                        tool("skill.bundle", {"action": "disable", "skill": "skill-management"})
                        assert Path(guide_path).is_file()
                        assert all(s["name"] != "skill-management" for s in request(node.url, "GET", endpoint)["catalog"]["skills"])
                    else:
                        catalog = request(node.url, "GET", endpoint)
                        assert catalog["paths"] == []
                        assert catalog["catalog"]["skills"][0]["description"] == "Shared build"
                        assert request(node.agent_url, "GET", f"/sessions/{session_id}")["session_id"] == session_id
                        assert all(s["name"] != "skill-management" for s in catalog["catalog"]["skills"])
                        status = tool("skill.bundle", {"action": "list"})
                        assert status["rollback"] and status["active"]["directory"] == selected_version
                        assert status["disabled"] == ["skill-management"]
                        tool("skill.bundle", {"action": "enable", "skill": "skill-management"})
                        assert any(s["name"] == "skill-management" for s in request(node.url, "GET", endpoint)["catalog"]["skills"])
                    process.terminate()
                    assert process.wait(timeout=30) == 0
                except Exception:
                    print(log_path.read_text()[-12000:])
                    raise
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.wait()
    print("PASS: Skill API authorization/validation, ordered sources, live Agent/node updates, ordinary file.read, bundle controls, persistent disable/rollback, durable catalogs and Gateway restart; temporary data cleaned")


if __name__ == "__main__":
    main()
