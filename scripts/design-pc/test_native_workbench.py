#!/usr/bin/env python3
"""Exercise the native component browser through its physical-input dev API."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/lib"))
from native_gui_fixture import Native, wait


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--launch-selector", choices=("family", "story", "start-story"), default="family")
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/design-pc/native-workbench")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    native = Native(None, args.output)
    checks = []

    def snapshot():
        return json.loads(native.ui("/v1/elements"))

    def reveal(id):
        def target():
            elements = snapshot()["elements"]
            item = next((e for e in elements if e["id"] == id), None)
            return item if item and item["visible"] and item["visible_bounds"]["height"] >= 28 else None
        if item := target():
            return item
        native.ui("/v1/actions", {"type": "scroll", "target": {"element_id": "story-navigation"},
                                  "delta_y": 10000})
        for _ in range(80):
            if item := target():
                return item
            native.ui("/v1/actions", {"type": "scroll", "target": {"element_id": "story-navigation"},
                                      "delta_y": -350})
        raise AssertionError(f"cannot reveal {id}")

    def click(id):
        wait(lambda: native.element(id, enabled=True), id, timeout=10)
        native.click(id)

    def choose(id, option):
        click(id)
        click(option)
        wait(lambda: not native.element(id + "-menu"), "menu dismissed", timeout=10)

    def canvas_width():
        return native.element("story-canvas")["bounds"]["width"]

    def save(name):
        started, previous, repeats = time.monotonic(), None, 0
        def settled():
            nonlocal previous, repeats
            elements = snapshot()["elements"]
            if any(e["visible"] and e["id"] in ("story-size-menu", "story-scenario-menu") for e in elements):
                return False
            bounds = [(e["id"], e["bounds"]) for e in elements
                      if e["id"] in ("story-canvas", "story-size", "story-scenario")]
            repeats = repeats + 1 if bounds == previous else 0
            previous = bounds
            # The history fixture intentionally includes a live activity spinner;
            # wait for layout and transient menus, not identical animation pixels.
            if repeats >= 2 and time.monotonic() - started > 1:
                (args.output / f"{name}.png").write_bytes(native.ui("/v1/screenshot"))
                return True
            return False
        wait(settled, f"settled {name} screenshot", timeout=10)

    try:
        native.process = subprocess.Popen(
            [str(args.binary.resolve()), "--dev", "--dev-port", native.url.rsplit(":", 1)[1],
             "--dev-token", "mesh-native-fixture", "--" + args.launch_selector,
             "history" if args.launch_selector == "family" else "history-collapsed"],
            cwd=ROOT, env=os.environ.copy(), stdout=native.log, stderr=native.log,
        )
        wait(lambda: native.element("story-canvas"), "native workbench ready", timeout=30)
        fit_width = canvas_width()
        assert fit_width > 560, fit_width
        assert not any(e["id"].startswith("story-history-") for e in snapshot()["elements"])
        save("01-history-fit")
        checks.append("family directory and work-area layout")

        click("story-expand")
        native.ui("/v1/actions", {"type": "scroll", "target": {"element_id": "history-ledger"}, "delta_y": -550})
        expanded = wait(lambda: next((e for e in snapshot()["elements"]
                                     if e["id"].startswith("history-output-disclosure-")
                                     and "收起" in e["label"]), None), "expanded body", timeout=10)
        disclosure = expanded["id"]
        reveal("story-family-button")
        click("story-family-button")
        wait(lambda: native.element("story-button"), "button component", timeout=10)
        reveal("story-family-history")
        click("story-family-history")
        wait(lambda: any(e["id"] == disclosure and "收起" in e["label"]
                         for e in snapshot()["elements"]), "retained expansion", timeout=10)
        checks.append("component switch retains expanded content")

        choose("story-size", "story-size-2")
        wait(lambda: abs(canvas_width() - 320) < 1, "narrow canvas", timeout=10)
        save("02-history-narrow")
        choose("story-size", "story-size-0")
        wait(lambda: abs(canvas_width() - fit_width) < 1, "fit restored", timeout=10)
        assert any(e["id"] == disclosure and "收起" in e["label"] for e in snapshot()["elements"])
        checks.append("canvas changes preserve content state")

        click("story-reset")
        wait(lambda: any(e["id"].startswith("history-output-disclosure-") and "展开" in e["label"]
                         for e in snapshot()["elements"]), "reset collapses content", timeout=10)
        checks.append("explicit reset restores only current specimen")
        choose("story-scenario", "story-scenario-history-empty")
        save("03-history-empty")
        choose("story-scenario", "story-scenario-history-error")
        save("04-history-error")
        choose("story-scenario", "story-scenario-history-collapsed")
        checks.append("normal, empty and error scenes")

        reveal("story-family-activity")
        click("story-family-activity")
        click("story-scenario")
        labels = {e["label"] for e in snapshot()["elements"]
                  if e["id"].startswith("story-scenario-activity-") and e["visible"]}
        assert labels == {"收起预览", "展开预览", "动态运行"}, labels
        click("story-scenario-activity-session-live")
        wait(lambda: native.element("session-activity-command-1"), "live activity row", timeout=10)
        save("05-activity-live")
        click("session-activity-simulate-end")
        wait(lambda: (button := native.element("session-activity-simulate-end"))
             and button["label"] == "重新开始", "activity completes", timeout=10)
        click("story-reset")
        wait(lambda: (button := native.element("session-activity-simulate-end"))
             and button["label"] == "模拟完成", "activity reset", timeout=10)
        assert native.element("story-scenario")["label"] == "动态运行"
        checks.append("activity scenes are named and reset replays the current scene")

        reveal("story-family-field")
        click("story-family-field")
        click("story-field")
        native.type("保留输入 Native 123")
        reveal("story-family-button")
        click("story-family-button")
        reveal("story-family-field")
        click("story-family-field")
        wait(lambda: native.element("story-field"), "restored field", timeout=10)
        save("05-field-retained")
        checks.append("input round-trip captured for visual inspection")

        reveal("design-guide-docs-00-overview-md")
        click("design-guide-docs-00-overview-md")
        wait(lambda: native.element("design-guide-scroll"), "native design guide", timeout=10)
        save("06-design-guide")
        checks.append("editable design guide opens natively")
        reveal("design-assets-brand")
        click("design-assets-brand")
        wait(lambda: native.element("design-assets-scroll"), "native design assets", timeout=10)
        assert native.element("design-image-design-assets-brand-mark-svg")
        save("07-design-assets")
        checks.append("curated SVG assets render in the native browser")

        reveal("story-family-onboarding")
        click("story-family-onboarding")
        wait(lambda: native.element("desktop-welcome-login"), "first-use login", timeout=10)
        click("desktop-welcome-login")
        wait(lambda: native.element("onboarding-cancel-login"), "browser wait specimen", timeout=10)
        click("onboarding-cancel-login")
        wait(lambda: native.element("desktop-welcome-login", enabled=True), "cancel returns to login", timeout=10)
        choose("story-scenario", "story-scenario-onboarding-failure-compact")
        notice = wait(lambda: native.element("onboarding-error"), "startup failure notice", timeout=10)
        retry = native.element("desktop-startup-retry")
        assert retry["bounds"]["y"] - (notice["bounds"]["y"] + notice["bounds"]["height"]) >= 12
        assert notice["bounds"]["width"] < 300
        save("08-onboarding-failure-spacing")
        choose("story-scenario", "story-scenario-onboarding-ready-compact")
        wait(lambda: native.element("new-chat-welcome"), "first Chat greeting", timeout=10)
        assert not native.element("onboarding-finish")
        assert not native.element("new-chat-context")
        assert not native.element("new-chat-device")
        options = native.element("new-chat-options")
        assert options["label"] == "Demo model · 高"
        assert options["bounds"] == options["visible_bounds"]
        assert native.element("new-chat-composer-surface")["bounds"]["x"] >= 0
        save("09-onboarding-new-chat")
        checks.append("failure actions have breathing room and the first Chat omits device setup while showing its selected model and thinking level")

        choose("story-scenario", "story-scenario-onboarding-models-compact")
        click("onboarding-add-model")
        wait(lambda: native.element("profile-close-form"), "real model connection form", timeout=10)
        wait(lambda: native.element("profile-provider-select") and
             native.element("profile-provider-select")["label"] == "OpenAI",
             "fixture provider ready", timeout=10)
        assert not native.element("model-add-device-mini1")
        checks.append("first-use flow opens the real local model editor without device choice")

        print("PASS zork-design-pc: " + "; ".join(checks), flush=True)
        (args.output / "result.json").write_text(json.dumps({
            "checks": checks,
            "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
            "head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
            "working_tree": subprocess.check_output(["git", "status", "--short"], cwd=ROOT, text=True),
        }, ensure_ascii=False, indent=2))
    finally:
        if native.process and native.process.poll() is None:
            try:
                (args.output / "last-elements.json").write_text(json.dumps(snapshot(), ensure_ascii=False, indent=2))
                save("last-window")
            except Exception:
                pass
        native.stop()
        native.log.close()


if __name__ == "__main__":
    main()
