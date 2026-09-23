#!/usr/bin/env python3
"""Gate complete native client frames, with real Metal completion feedback."""
import argparse
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import platform
import signal
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
GATE_ID = "client-frame"


def contract():
    spec = importlib.util.spec_from_file_location("critical_smoke", ROOT / "scripts/smoke-critical.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.GATES[GATE_ID]


class Unverified(RuntimeError):
    pass


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def distribution(values):
    if not values or any(type(v) not in (int, float) or not math.isfinite(v) or v < 0 for v in values):
        raise Unverified("Missing or invalid frame measurements")
    values = sorted(values)
    return {"count": len(values), "p50Ms": values[len(values) // 2],
            "p95Ms": values[int(len(values) * .95)], "p99Ms": values[int(len(values) * .99)], "maxMs": values[-1]}


def validate_dialog_case(case, opened):
    frames = case.get("frames", [])
    after = case.get("after", {})
    if (after.get("engine") != "plain" or after.get("open") is not opened
            or after.get("contentAlpha") != (1 if opened else 0)
            or after.get("backdropAlpha") != (1 if opened else 0)
            or case.get("mounted", after.get("mounted")) is not opened
            or any(type(case.get(key)) is not int or case[key] <= 0
                   for key in ("contentTransitionFrames", "backdropTransitionFrames"))
            or not frames):
        raise Unverified("Actual native PlainDialog fade, input tree, or terminal state is missing")


def playground_cases(report, fixture):
    if not fixture.get("playground"):
        return []
    playground = report.get("playground", {})
    cases = playground.get("cases", [])
    expected = ["page-scroll", "directory-scroll"]
    expected += [name for _ in range(fixture["panel_pairs"]) for name in ("dialog-open", "dialog-close")]
    if (playground.get("status") != "measured" or playground.get("viewport") != fixture["liquid_viewport"]
            or playground.get("component") != "zork-ui::modal::PlainDialog"
            or playground.get("control") != "liquid-dialog-panel"
            or [case.get("name") for case in cases] != expected):
        raise Unverified("Missing native page/directory scrolling or complete dialog operation cycle")
    scroll = fixture["playground_scroll"]
    for case in cases[:2]:
        positions = [case.get(key, {}).get("y") for key in ("before", "after")]
        if (case.get("scrollSpeed") != scroll["px_per_second"]
                or case.get("directionChanges") != scroll["legs"] - 1
                or not case.get("marker")
                or not all(type(value) in (int, float) and math.isfinite(value) for value in positions)
                or abs(positions[1] - positions[0]) <= scroll["min_displacement_px"]):
            raise Unverified("Scroll workload did not move real visible content with the approved input")
    for case in cases[2:]:
        opened = case["name"] == "dialog-open"
        after = case.get("after", {})
        validate_dialog_case(case, opened)
    return cases


def evaluate(report, definition):
    fixture = definition["fixture"]
    if (report.get("status") != "measured" or report.get("fixture") != fixture
            or report.get("profile") != "release" or report.get("renderer") != "native-metal"
            or report.get("uncapped") is not True or report.get("gpuCompletionProbe") is not True
            or report.get("physicalPresentationMeasured") is not False
            or report.get("records") != fixture["messages"]
            or report.get("viewport") != fixture["viewport"]
            or type(report.get("scaleFactor")) not in (int, float)
            or not math.isfinite(report["scaleFactor"]) or report["scaleFactor"] <= 0):
        raise Unverified("Incomplete or mismatched native benchmark conditions")
    cases = report.get("cases", [])
    expected = [name for _ in range(fixture["panel_pairs"]) for name in ("panel-open", "panel-close")]
    expected += ["scroll-start", "scroll-middle", "scroll-end"]
    if [case.get("name") for case in cases] != expected:
        raise Unverified("Missing client workload or operation cycle")
    coverage = report.get("coverage", {})
    kinds = coverage.get("message_kinds", {})
    cold = report.get("coldEntries", [])
    if (not kinds or set(coverage.get("measured_kinds", [])) != set(kinds)
            or coverage.get("both_roles_measured") is not True or len(cold) < 2 * len(kinds)):
        raise Unverified("Cold entry or mixed message/role coverage is incomplete")
    liquid = report.get("liquid", {})
    liquid_cases = liquid.get("cases", [])
    if (liquid.get("status") != "measured" or liquid.get("viewport") != fixture["liquid_viewport"]
            or liquid.get("component") != "zork-ui::modal::PlainDialog"
            or liquid.get("control") != "liquid-library-dialog"
            or liquid.get("recipe") != "plain-panel-opacity-backdrop"
            or [case.get("name") for case in liquid_cases] != [name for _ in range(fixture["panel_pairs"]) for name in ("liquid-open", "liquid-close")]):
        raise Unverified("The requested native PlainDialog component/recipe was not measured")
    for case in liquid_cases:
        validate_dialog_case(case, case["name"] == "liquid-open")
    cases = [{"name": "message-entry", "frames": cold}, *cases, *liquid_cases,
             *playground_cases(report, fixture)]
    results = []
    for index, case in enumerate(cases):
        rows = case.get("frames", [])
        if len(rows) < 2:
            raise Unverified("No multi-frame client activity was measured")
        starts = [row.get("at") for row in rows]
        completed = [row.get("completedAt") for row in rows]
        distribution(starts)
        distribution(completed)
        if any(b <= a for a, b in zip(starts, starts[1:])) or any(b < a for a, b in zip(completed, completed[1:])):
            raise Unverified("Frame timestamps are not in submission order")
        cpu = distribution([row.get("cpuMs") for row in rows])
        whole_values = [end - start for start, end in zip(starts, completed)]
        whole = distribution(whole_values)
        if any(row["cpuMs"] > total + .001 for row, total in zip(rows, whole_values)):
            raise Unverified("Whole-frame completion preceded CPU submission")
        if any(type(row.get("continuing")) is not bool for row in rows):
            raise Unverified("Animation continuity evidence is missing")
        # A frame with no requested successor marks intentional idle. Cursor
        # blink or a later independent update must not turn idle into a stall.
        gaps = [b - a for row, a, b in zip(rows, completed, completed[1:]) if row["continuing"]]
        intervals = distribution(gaps)
        if sum(gaps) <= 0:
            raise Unverified("No elapsed continuous drawing interval")
        fps = len(gaps) * 1000 / sum(gaps)
        budget, cadence = definition["budget_ms"], 1000 / definition["target_fps"]
        result = {"name": case["name"], "index": index, "firstFrameMs": whole_values[0],
                  "frameCpu": cpu, "frameComplete": whole, "completionIntervals": intervals,
                  "completedFps": fps, "overBudgetFrames": sum(value >= budget for value in whole_values),
                  "slowIntervals": sum(value >= cadence for value in gaps)}
        result["passed"] = whole["maxMs"] < budget and fps > definition["target_fps"] and intervals["maxMs"] < cadence
        results.append(result)
    return results


def competing_processes():
    lines = subprocess.check_output(["ps", "-axo", "pid=,comm="], text=True).splitlines()
    return [line.strip() for line in lines if line.split() and
            (line.split()[-1].rsplit("/", 1)[-1] in ("cargo", "rustc") or line.endswith("Chrome for Testing"))]


def stop_native_process(process):
    if process.poll() is not None:
        return
    # This process group contains only this invocation's native fixture and
    # caffeinate; preserve all other client instances.
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        pass
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    definition = contract()
    report = {"gate": GATE_ID, "contract": definition, "platform": platform.platform(),
              "machine": platform.machine(), "status": "unverified", "passed": False,
              "measurement": "Production client view, actual native Metal commands. Complete frame includes input, animation, UI rendering, submission and observed GPU completion. The same native Metal renderer targets a reused private texture to remove drawable-supply pacing; native window/input/layout are real, screen presentation FPS is not claimed."}
    try:
        if sys.platform != "darwin":
            raise Unverified("Native macOS desktop session required")
        binary = args.binary.resolve(strict=True)
        stamp = binary.stat()
        report["competingProcessesBefore"] = competing_processes()
        if report["competingProcessesBefore"]:
            raise Unverified("Concurrent builds/benchmarks must finish before measurement")
        config = output / "fixture.json"
        config.write_text(json.dumps(definition["fixture"], indent=2))
        evidence = output / "native-frames.json"
        evidence.unlink(missing_ok=True)
        with (output / "native.log").open("w") as log:
            process = subprocess.Popen(["caffeinate", "-d", "-i", "-u", str(binary), "--native-frames", str(config), str(output)],
                                       cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                process.wait(timeout=180)
            finally:
                stop_native_process(process)
        report["exitCode"] = process.returncode
        report["competingProcessesAfter"] = competing_processes()
        report["binarySha256"] = digest(binary)
        report["rawEvidence"] = str(evidence)
        if report["competingProcessesAfter"]:
            raise Unverified("Competing build/browser observed after measurement")
        after = binary.stat()
        if (stamp.st_ino, stamp.st_mtime_ns, stamp.st_size) != (after.st_ino, after.st_mtime_ns, after.st_size):
            raise Unverified("Native binary changed during measurement")
        raw = json.loads(evidence.read_text())
        if process.returncode:
            raise Unverified(raw.get("error", "Native process failed; see native.log"))
        report["cases"] = evaluate(raw, definition)
        report["viewport"], report["scaleFactor"] = raw["viewport"], raw["scaleFactor"]
        report["passed"] = all(case["passed"] for case in report["cases"])
        report["status"] = "passed" if report["passed"] else "failed"
    except Exception as error:
        report["error"] = str(error)
    (output / "result.json").write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(f"{report['status'].upper()} {GATE_ID}: {output / 'result.json'}")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
