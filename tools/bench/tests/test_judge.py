from __future__ import annotations

import copy
import json
import unittest
from pathlib import Path

from benchlib.judge import aggregate, exit_code, judge


ROOT = Path(__file__).parent / "fixtures" / "report"
THRESHOLDS = json.loads((Path(__file__).parents[1] / "thresholds.json").read_text())


def load_summary(name: str) -> dict:
    return json.loads((ROOT / name).read_text())


def result(summary: dict, product: str = "remotescreen", scene: str = "static", network: str = "wifi-good") -> dict:
    for item in summary["results"]:
        if (
            item["product"]["id"] == product
            and item["scene"] == scene
            and item["network"]["profile"] == network
        ):
            return item
    raise AssertionError(f"missing {product}/{scene}/{network}")


class JudgeTests(unittest.TestCase):
    def test_no_baseline(self) -> None:
        current = load_summary("current.json")
        verdict = judge(current, None, THRESHOLDS)
        infos = [item for item in verdict["items"] if item["message"] == "no baseline"]
        self.assertEqual(len(infos), 1)
        self.assertEqual(
            [item["level"] for item in verdict["items"] if item["scene"] == "scroll" and item["metric"] == "longest_freeze_ms"],
            ["fail"],
        )
        self.assertEqual(exit_code(verdict), 1)

    def test_p95_regression(self) -> None:
        current = load_summary("current.json")
        baseline = load_summary("baseline.json")
        item = next(
            item
            for item in judge(current, baseline, THRESHOLDS)["items"]
            if item["metric"] == "latency_p95"
        )
        self.assertEqual(item["level"], "regression")
        self.assertEqual(item["limit"], 43.2)
        self.assertIn("48.0", item["message"])
        self.assertIn("43.2", item["message"])

    def test_p95_small_abs_not_regression(self) -> None:
        current = load_summary("current.json")
        baseline = load_summary("baseline.json")
        result(current)["latency_ms"]["p95"] = 14.0
        result(baseline)["latency_ms"]["p95"] = 10.0
        verdict = judge(current, baseline, THRESHOLDS)
        self.assertFalse(any(item["metric"] == "latency_p95" and item["level"] == "regression" for item in verdict["items"]))

    def test_fps_drop(self) -> None:
        baseline = load_summary("baseline.json")
        current = load_summary("current.json")
        base = result(baseline)["delivery"]["display_fps"]
        current_result = result(current)
        current_result["delivery"]["display_fps"] = 53
        self.assertTrue(any(item["metric"] == "display_fps" for item in judge(current, baseline, THRESHOLDS)["items"]))
        current_result["delivery"]["display_fps"] = 53.5
        self.assertTrue(any(item["metric"] == "display_fps" for item in judge(current, baseline, THRESHOLDS)["items"]))
        current_result["delivery"]["display_fps"] = 54.5
        self.assertFalse(any(item["metric"] == "display_fps" and item["level"] == "regression" for item in judge(current, baseline, THRESHOLDS)["items"]))
        self.assertEqual(base, 60)

    def test_cpu_warn(self) -> None:
        current = load_summary("current.json")
        baseline = load_summary("baseline.json")
        result(current)["host_usage"]["cpu_pct_mean"] = 11.0
        result(baseline)["host_usage"]["cpu_pct_mean"] = 8.0
        verdict = judge(current, baseline, THRESHOLDS)
        self.assertFalse(any(item["metric"] == "cpu_pct_mean" for item in verdict["items"]))
        result(current)["host_usage"]["cpu_pct_mean"] = 11.5
        verdict = judge(current, baseline, THRESHOLDS)
        self.assertTrue(any(item["metric"] == "cpu_pct_mean" and item["level"] == "warn" for item in verdict["items"]))

    def test_status_not_comparable(self) -> None:
        current = load_summary("current.json")
        baseline = load_summary("baseline.json")
        current_scroll = result(current, scene="scroll", network="weak")
        current_scroll["status"] = "invalid"
        verdict = judge(current, baseline, THRESHOLDS)
        item = next(item for item in verdict["items"] if item["scene"] == "scroll" and item["metric"] == "status")
        self.assertEqual(item["level"], "info")
        self.assertIn("current=invalid baseline=ok", item["message"])
        self.assertEqual(exit_code({"regressions": 0, "fails": 0, "warns": 3}), 0)

    def test_vs_best_warn(self) -> None:
        current = load_summary("current.json")
        baseline = load_summary("baseline.json")
        result(current)["latency_ms"]["p50"] = 40
        result(current)["latency_ms"]["p95"] = 40
        verdict = judge(current, baseline, THRESHOLDS)
        item = next(item for item in verdict["items"] if item["metric"] == "vs_best_p50")
        self.assertEqual(item["level"], "warn")
        self.assertIn("Moonlight + Sunshine", item["message"])
        self.assertIn("30.0", item["message"])
        result(current)["latency_ms"]["p50"] = 32
        self.assertFalse(any(item["metric"] == "vs_best_p50" for item in judge(current, baseline, THRESHOLDS)["items"]))

    def test_repeats_median(self) -> None:
        current = load_summary("current.json")
        repeated = [copy.deepcopy(result(current)) for _ in range(3)]
        for item, p95 in zip(repeated, (30, 50, 40)):
            item["latency_ms"]["p95"] = p95
        grouped = aggregate(repeated)
        key = next(key for key in grouped if key[0] == "remotescreen")
        self.assertEqual(grouped[key]["metrics"]["latency_p95"], 40)

    def test_items_sorted_and_counted(self) -> None:
        current = load_summary("current.json")
        baseline = load_summary("baseline.json")
        verdict = judge(current, baseline, THRESHOLDS)
        levels = [item["level"] for item in verdict["items"]]
        self.assertEqual(verdict["regressions"], levels.count("regression"))
        self.assertEqual(verdict["fails"], levels.count("fail"))
        self.assertEqual(verdict["warns"], levels.count("warn"))
        self.assertEqual(levels[:2], ["regression", "regression"])


if __name__ == "__main__":
    unittest.main()
