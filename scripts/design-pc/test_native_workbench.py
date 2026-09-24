#!/usr/bin/env python3
"""Exercise the native design browser through its physical-input dev API."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
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
            return item if item and item["visible"] and item["visible_bounds"]["height"] >= 24 else None
        if item := target():
            return item
        native.ui("/v1/actions", {"type": "scroll", "target": {"element_id": "design-directory"},
                                  "delta_y": 10000})
        for _ in range(80):
            if item := target():
                return item
            native.ui("/v1/actions", {"type": "scroll", "target": {"element_id": "design-directory"},
                                      "delta_y": -300})
        raise AssertionError(f"cannot reveal {id}")

    def click(id):
        wait(lambda: native.element(id, enabled=True), id, timeout=10)
        native.click(id)

    def open_state(entry, story):
        reveal(f"design-entry-{entry}")
        click(f"design-entry-{entry}")
        reveal(f"design-state-{story}")
        click(f"design-state-{story}")
        wait(lambda: native.element("story-canvas"), f"{story} canvas", timeout=10)

    def canvas_width():
        return native.element("story-canvas")["bounds"]["width"]

    def save(name):
        started, previous, repeats = time.monotonic(), None, 0
        def settled():
            nonlocal previous, repeats
            elements = snapshot()["elements"]
            bounds = [(e["id"], e["bounds"]) for e in elements if e["id"] in ("story-canvas", "design-overview")]
            repeats = repeats + 1 if bounds == previous else 0
            previous = bounds
            if repeats >= 2 and time.monotonic() - started > 1:
                (args.output / f"{name}.png").write_bytes(native.ui("/v1/screenshot"))
                return True
            return False
        wait(settled, f"settled {name} screenshot", timeout=10)

    home = tempfile.TemporaryDirectory()
    try:
        # A private HOME keeps the remembered position and stills out of the user's cache.
        env = dict(os.environ, HOME=home.name)
        native.process = subprocess.Popen(
            [str(args.binary.resolve()), "--dev", "--dev-port", native.url.rsplit(":", 1)[1],
             "--dev-token", "mesh-native-fixture", "--" + args.launch_selector,
             "history" if args.launch_selector == "family" else "history-collapsed"],
            cwd=ROOT, env=env, stdout=native.log, stderr=native.log,
        )
        wait(lambda: native.element("design-entry-history"), "native browser ready", timeout=30)
        if args.launch_selector == "family":
            wait(lambda: native.element("design-overview"), "family overview", timeout=10)
            assert native.element("design-tile-history-empty")
            save("01-history-overview")
            checks.append("family launch opens the overview of all states")
        open_state("history", "history-collapsed")
        fit_width = canvas_width()
        assert fit_width > 560, fit_width
        save("02-history-state")

        click("design-width-360")
        wait(lambda: abs(canvas_width() - 360) < 1, "360 canvas", timeout=10)
        save("03-history-360")
        click("design-width-window")
        wait(lambda: abs(canvas_width() - fit_width) < 1, "window restored", timeout=10)
        checks.append("width presets resize the live specimen")

        for state in ("history-empty", "history-error"):
            open_state("history", state)
            save(f"04-{state}")
        checks.append("normal, empty and error states")

        open_state("field", "field-empty")
        click("story-field")
        native.type("保留输入 Native 123")
        open_state("button", "button-primary")
        open_state("field", "field-empty")
        wait(lambda: native.element("story-field"), "restored field", timeout=10)
        save("05-field-retained")
        checks.append("switching components keeps the specimen")

        native.ui("/v1/actions", {"type": "key", "keystroke": "cmd-k"})
        native.type("新建有草稿")
        native.ui("/v1/actions", {"type": "key", "keystroke": "enter"})
        wait(lambda: native.element("new-chat-composer-surface"), "search jumps to a state", timeout=10)
        save("06-search")
        checks.append("search reaches a state in one step")

        open_state("onboarding", "onboarding-login")
        click("design-width-360")
        click("desktop-welcome-login")
        wait(lambda: native.element("onboarding-cancel-login"), "browser wait specimen", timeout=10)
        click("onboarding-cancel-login")
        wait(lambda: native.element("desktop-welcome-login", enabled=True), "cancel returns to login", timeout=10)
        open_state("onboarding", "onboarding-failure")
        notice = wait(lambda: native.element("onboarding-error"), "startup failure notice", timeout=10)
        retry = native.element("desktop-startup-retry")
        assert retry["bounds"]["y"] - (notice["bounds"]["y"] + notice["bounds"]["height"]) >= 12
        save("07-onboarding-failure-spacing")
        checks.append("first-use states render interactively")

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
        home.cleanup()


if __name__ == "__main__":
    main()
