#!/usr/bin/env python3
"""Exercise critical smoke rejection paths without running a product instance."""
import importlib.util
import argparse
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("critical_smoke", Path(__file__).with_name("smoke-critical.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)
spec = importlib.util.spec_from_file_location("native_budget", Path(__file__).parent / "test-client-frame-budget.py")
native_budget = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native_budget)


class GateTests(unittest.TestCase):
    def sample(self, ui=750, node=1000):
        return {"gui_alive": True, "ui_interactive": {"ms": ui}, "local_node": {"ms": node}}

    def test_both_milestones_must_meet_the_inclusive_deadline(self):
        self.assertEqual(smoke.GATES["local-startup"]["budget_ms"], 1000)
        self.assertTrue(smoke.gate_passes(self.sample(), 1000))
        self.assertFalse(smoke.gate_passes(self.sample(node=1000.001), 1000))
        self.assertFalse(smoke.gate_passes(self.sample(ui=1001, node=20), 1000))

    def test_missing_readiness_or_process_exit_cannot_pass(self):
        for key in ("ui_interactive", "local_node", "gui_alive"):
            sample = self.sample()
            sample.pop(key)
            self.assertFalse(smoke.gate_passes(sample, 1000))
        sample = self.sample()
        sample["local_node"]["error"] = "process exited"
        self.assertFalse(smoke.gate_passes(sample, 1000))

    def test_invalid_measurements_cannot_pass(self):
        for value in (-1, float("nan"), float("inf"), "1", True, False):
            self.assertFalse(smoke.gate_passes(self.sample(node=value), 1000))

    def test_readiness_belongs_to_the_same_embedded_node_and_fixture(self):
        root = Path("/fixture/node")
        supervisor = {"protocol": 1, "agent_mode": "embedded", "data_root": str(root)}
        station = {"ok": True, "pid": 42}
        agent = {"ok": True, "pid": 42, "embedded": True}
        info = {"protocol": 1, "data_root": str(root)}
        smoke.validate_node(root, supervisor, station, agent, info, 42)
        for changed in ({**agent, "embedded": False}, {**agent, "pid": 99}, {**agent, "ok": False}):
            with self.assertRaises(RuntimeError):
                smoke.validate_node(root, supervisor, station, changed, info, 42)
        with self.assertRaises(RuntimeError):
            smoke.validate_node(root, supervisor, station, agent, {**info, "data_root": "/other"}, 42)
        with self.assertRaises(RuntimeError):
            smoke.validate_node(root, {**supervisor, "agent_mode": None}, station, agent, info, 42)

    def test_unverified_gate_does_not_prevent_other_approved_gates_from_running(self):
        calls = []
        def unavailable(*args):
            calls.append("startup")
            raise RuntimeError("No signed app")
        def available(*args):
            calls.append("directory")
            return {"passed": True, "status": "passed"}
        with tempfile.TemporaryDirectory() as output, patch.dict(smoke.RUNNERS, {
                "local-startup": unavailable, "client-frame": available}):
            results = smoke.run_gates(argparse.Namespace(), Path(output))
        self.assertEqual(calls, ["startup", "directory"])
        self.assertEqual(results["local-startup"]["status"], "unverified")
        self.assertTrue(results["client-frame"]["passed"])

    def test_missing_artifacts_produce_a_failed_suite_with_both_gates_unverified(self):
        with tempfile.TemporaryDirectory() as output:
            result = subprocess.run([sys.executable, smoke.__file__, "--output", output], capture_output=True)
            report = json.loads((Path(output) / "result.json").read_text())
        self.assertEqual(result.returncode, 1)
        self.assertFalse(report["passed"])
        self.assertEqual(set(report["results"]), set(smoke.GATES))
        self.assertTrue(all(result["status"] == "unverified" for result in report["results"].values()))


class ClientBudgetTests(unittest.TestCase):
    def setUp(self):
        self.definition = copy.deepcopy(smoke.GATES["client-frame"])

    def report(self, whole_ms=7, interval_ms=7):
        fixture = self.definition["fixture"]
        names = [name for _ in range(fixture["panel_pairs"]) for name in ["panel-open", "panel-close"]]
        names += ["scroll-start", "scroll-middle", "scroll-end"]
        report = {"fixture": copy.deepcopy(fixture), "status": "measured", "profile": "release",
                "renderer": "native-metal", "uncapped": True, "gpuCompletionProbe": True,
                "physicalPresentationMeasured": False, "records": fixture["messages"],
                "viewport": fixture["viewport"], "scaleFactor": 1,
                "cases": [{"name": name, "frames": [
                    {"at": frame * interval_ms, "completedAt": frame * interval_ms + whole_ms,
                    "cpuMs": 1, "continuing": True} for frame in range(4)]} for name in names]}
        report["liquid"] = {"status": "measured", "viewport": fixture["liquid_viewport"],
            "component": "zork-ui::modal::PlainDialog", "control": "liquid-library-dialog",
            "recipe": "plain-panel-opacity-backdrop", "cases": []}
        for _ in range(fixture["panel_pairs"]):
            for name in ("liquid-open", "liquid-close"):
                frames = copy.deepcopy(report["cases"][0]["frames"])
                opened = name == "liquid-open"
                report["liquid"]["cases"].append({"name": name, "frames": frames,
                    "contentTransitionFrames": 4, "backdropTransitionFrames": 4,
                    "mounted": opened,
                    "after": {"engine": "plain", "open": opened, "contentAlpha": 1 if opened else 0,
                              "backdropAlpha": 1 if opened else 0}})
        scroll = fixture["playground_scroll"]
        report["playground"] = {"status": "measured", "viewport": fixture["liquid_viewport"],
            "component": "zork-ui::modal::PlainDialog", "control": "liquid-dialog-panel", "cases": []}
        for name in ("page-scroll", "directory-scroll"):
            report["playground"]["cases"].append({"name": name,
                "frames": copy.deepcopy(report["cases"][0]["frames"]),
                "marker": name + "-visible-row", "before": {"y": 200}, "after": {"y": 80},
                "scrollSpeed": scroll["px_per_second"], "directionChanges": scroll["legs"] - 1})
        for _ in range(fixture["panel_pairs"]):
            for name in ("dialog-open", "dialog-close"):
                opened = name == "dialog-open"
                report["playground"]["cases"].append({"name": name,
                    "frames": copy.deepcopy(report["liquid"]["cases"][0]["frames"]),
                    "contentTransitionFrames": 4, "backdropTransitionFrames": 4,
                    "after": {"engine": "plain", "open": opened, "mounted": opened,
                              "contentAlpha": 1 if opened else 0,
                              "backdropAlpha": 1 if opened else 0}})
        report["coldEntries"] = copy.deepcopy(report["cases"][0]["frames"])
        report["coverage"] = {"message_kinds": {"plain": fixture["messages"]},
                              "measured_kinds": ["plain"], "both_roles_measured": True}
        return report

    def test_strict_whole_frame_deadline_and_approved_target(self):
        self.assertEqual(self.definition["budget_ms"], 8)
        self.assertEqual(self.definition["target_fps"], 120)
        self.assertTrue(all(row["passed"] for row in native_budget.evaluate(self.report(7.999), self.definition)))
        self.assertFalse(any(row["passed"] for row in native_budget.evaluate(self.report(8), self.definition)))

    def test_fast_cpu_and_high_average_fps_do_not_hide_a_slow_completion_gap(self):
        report = self.report(1, 2)
        for case in [{"frames": report["coldEntries"]}, *report["cases"], *report["liquid"]["cases"],
                     *report["playground"]["cases"]]:
            for row in case["frames"][2:]:
                row["at"] += 7
                row["completedAt"] += 7
        rows = native_budget.evaluate(report, self.definition)
        self.assertTrue(all(row["completedFps"] > 120 and row["frameComplete"]["maxMs"] < 8 for row in rows))
        self.assertFalse(any(row["passed"] for row in rows))

    def test_a_single_long_frame_cannot_be_hidden_by_p95_or_dropped_as_warmup(self):
        report = self.report()
        phase = report["cases"][0]
        phase["frames"] = [{"at": i * 7, "completedAt": i * 7 + (8.1 if i == 0 else 7),
                            "cpuMs": 1, "continuing": True} for i in range(100)]
        row = native_budget.evaluate(report, self.definition)[1]
        self.assertLess(row["frameComplete"]["p95Ms"], 8)
        self.assertEqual(row["overBudgetFrames"], 1)
        self.assertFalse(row["passed"])

    def test_missing_gpu_completion_and_invalid_times_are_unverified(self):
        for value in (None, float("nan"), float("inf"), -1, "7", True):
            report = self.report()
            report["cases"][0]["frames"][0]["completedAt"] = value
            with self.assertRaises(native_budget.Unverified):
                native_budget.evaluate(report, self.definition)

    def test_approved_scroll_and_dialog_workloads_are_in_the_full_gate(self):
        self.assertTrue(self.definition["fixture"]["playground"])
        rows = native_budget.evaluate(self.report(), self.definition)
        names = [row["name"] for row in rows]
        for name in ("page-scroll", "directory-scroll"):
            self.assertEqual(names.count(name), 1)
        for name in ("liquid-open", "liquid-close", "dialog-open", "dialog-close"):
            self.assertEqual(names.count(name), self.definition["fixture"]["panel_pairs"])

    def test_first_long_frame_fails_each_new_workload_even_with_a_fast_p95(self):
        for section, name in [("liquid", "liquid-open"), *[("playground", name) for name in
                ("page-scroll", "directory-scroll", "dialog-open", "dialog-close")]]:
            with self.subTest(workload=name):
                report = self.report()
                case = next(case for case in report[section]["cases"] if case["name"] == name)
                template = case["frames"][0]
                case["frames"] = [{**template, "at": i * 7,
                    "completedAt": i * 7 + (9 if i == 0 else 7)} for i in range(100)]
                result = next(row for row in native_budget.evaluate(report, self.definition) if row["name"] == name)
                self.assertLess(result["frameComplete"]["p95Ms"], 8)
                self.assertEqual(result["firstFrameMs"], 9)
                self.assertFalse(result["passed"])

    def test_missing_movement_or_incomplete_dialog_evidence_is_unverified(self):
        for change in (
            lambda r: r.pop("playground"),
            lambda r: r["playground"].update(status="incomplete"),
            lambda r: r["playground"].update(control="other-dialog"),
            lambda r: r["playground"]["cases"].pop(),
            lambda r: r["playground"]["cases"][0].update(after={"y": 200}),
            lambda r: r["playground"]["cases"][0].update(scrollSpeed=1),
            lambda r: r["playground"]["cases"][1].update(directionChanges=0),
            lambda r: r["playground"]["cases"][2]["after"].update(contentAlpha=0),
            lambda r: r["playground"]["cases"][2]["after"].update(mounted=False),
            lambda r: r["playground"]["cases"][2].update(contentTransitionFrames=0),
            lambda r: r["playground"]["cases"][2].update(backdropTransitionFrames=0),
        ):
            report = self.report()
            change(report)
            with self.assertRaises(native_budget.Unverified):
                native_budget.evaluate(report, self.definition)

    def test_wrong_renderer_and_shortened_workload_are_unverified(self):
        for change in (lambda r: r.update(renderer="webgpu"),
                       lambda r: r["cases"].pop(),
                       lambda r: r["fixture"].update(panel_pairs=1),
                       lambda r: r.update(records=100),
                       lambda r: r.update(coldEntries=[]),
                       lambda r: r["coverage"].update(both_roles_measured=False),
                       lambda r: r.update(scaleFactor=float("nan")),
                       lambda r: r.update(uncapped=False)):
            report = self.report()
            change(report)
            with self.assertRaises(native_budget.Unverified):
                native_budget.evaluate(report, self.definition)

    def test_other_component_or_missing_plain_fade_cannot_validate_dialog(self):
        for change in (
            lambda r: r.pop("liquid"),
            lambda r: r["liquid"].update(control="conversation-browser"),
            lambda r: r["liquid"].update(recipe="ordinary-layout"),
            lambda r: r["liquid"]["cases"][0].update(contentTransitionFrames=0),
            lambda r: r["liquid"]["cases"][0].update(backdropTransitionFrames=0),
            lambda r: r["liquid"]["cases"][0]["after"].update(backdropAlpha=0),
            lambda r: r["liquid"]["cases"][0].update(mounted=False),
            lambda r: r["liquid"]["cases"][0]["after"].update(engine="liquid"),
        ):
            report = self.report()
            change(report)
            with self.assertRaises(native_budget.Unverified):
                native_budget.evaluate(report, self.definition)

    def test_intentional_idle_is_not_a_dropped_animation_frame(self):
        report = self.report()
        rows = report["cases"][0]["frames"]
        rows[1]["continuing"] = False
        for row in rows[2:]:
            row["at"] += 1000
            row["completedAt"] += 1000
        self.assertTrue(native_budget.evaluate(report, self.definition)[1]["passed"])


if __name__ == "__main__":
    unittest.main()
