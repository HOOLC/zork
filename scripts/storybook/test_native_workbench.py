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
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/storybook/native-workbench")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    native = Native(None, args.output)
    checks = []

    def snapshot():
        return json.loads(native.ui("/v1/elements"))

    def reveal(id):
        for _ in range(15):
            elements = snapshot()["elements"]
            target = next((e for e in elements if e["id"] == id), None)
            if target and target["visible"] and target["visible_bounds"]["height"] >= 28:
                return target
            area = next(e for e in elements if e["id"] == "story-navigation")
            down = not target or target["bounds"]["y"] > area["bounds"]["y"]
            native.ui("/v1/actions", {"type": "scroll", "target": {"element_id": "story-navigation"},
                                      "delta_y": -300 if down else 300})
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
            pixels = native.ui("/v1/screenshot")
            repeats = repeats + 1 if pixels == previous else 0
            previous = pixels
            if repeats >= 2 and time.monotonic() - started > .4:
                (args.output / f"{name}.png").write_bytes(pixels)
                return True
            return False
        wait(settled, f"settled {name} screenshot", timeout=10)

    try:
        native.process = subprocess.Popen(
            [str(args.binary.resolve()), "--dev", "--dev-port", native.url.rsplit(":", 1)[1],
             "--dev-token", "mesh-native-fixture", "--start-story", "history-collapsed"],
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

        print("PASS native Storybook: " + "; ".join(checks), flush=True)
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
