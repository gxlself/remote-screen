from __future__ import annotations

import json
import unittest
from pathlib import Path

from benchlib.judge import judge
from benchlib.report import render_report


ROOT = Path(__file__).parent / "fixtures" / "report"
THRESHOLDS = json.loads((Path(__file__).parents[1] / "thresholds.json").read_text())


class ReportTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.current = json.loads((ROOT / "current.json").read_text())
        cls.baseline = json.loads((ROOT / "baseline.json").read_text())
        cls.verdict = judge(cls.current, cls.baseline, THRESHOLDS)
        cls.report = render_report(cls.current, cls.baseline, cls.verdict)

    def test_golden(self) -> None:
        expected = (ROOT / "expected_report.md").read_text()
        self.assertEqual(self.report, expected)

    def test_table_header_fixed(self) -> None:
        header = "| 产品 | 版本 | 延迟 p50 | p95 | p99 | max | 帧率 | 最长卡顿 | >100ms 卡顿 | 带宽 | Host CPU | Host GPU | SSIM | 样本 | 状态 |"
        self.assertEqual(self.report.count(header), 3)

    def test_none_rendered_as_dash(self) -> None:
        failed = next(line for line in self.report.splitlines() if "| 本产品 |" in line and "| 失败 |" in line)
        self.assertEqual(failed.count("—"), 12)

    def test_order(self) -> None:
        section = self.report.split("## 静态桌面 · wifi-good", 1)[1].split("## ", 1)[0]
        labels = [line.split("|")[1].strip() for line in section.splitlines() if line.startswith("|") and not line.startswith("| 产品 |") and "---" not in line]
        self.assertEqual(labels[:4], ["本产品", "RustDesk", "Moonlight + Sunshine", "本机基线"])

    def test_local_line(self) -> None:
        self.assertIn("本机基线 p50", self.report.split("## 静态桌面 · wifi-good", 1)[1].split("## ", 1)[0])
        self.assertNotIn("本机基线 p50", self.report.split("## 滚动网页 · weak", 1)[1].split("## ", 1)[0])

    def test_jitter_note(self) -> None:
        self.assertIn("weak 网络在 macOS dummynet 上整形，未加抖动。", self.report)
        all_jitter = json.loads(json.dumps(self.current))
        for result in all_jitter["results"]:
            result["network"]["jitter_applied"] = True
        self.assertNotIn("未加抖动", render_report(all_jitter))

    def test_thresholds_json_matches_contract(self) -> None:
        expected = {
            "schema": 1,
            "self_product": "remotescreen",
            "regression": {
                "latency_p50": {"ratio": 0.15, "abs_ms": 3},
                "latency_p95": {"ratio": 0.20, "abs_ms": 5},
                "display_fps": {"drop_ratio": 0.10},
                "longest_freeze_ms": {"ratio": 0.5, "abs_ms": 100},
                "ssim": {"drop": 0.02},
            },
            "warn": {
                "host_tx_mbps": {"ratio": 0.30},
                "cpu_pct_mean": {"ratio": 0.30, "abs": 3},
                "gpu_pct_mean": {"ratio": 0.30, "abs": 3},
            },
            "absolute": {"weak": {"longest_freeze_ms": 500}},
            "vs_best": {"latency_p50_ratio": 0.10, "latency_p95_ratio": 0.10},
        }
        self.assertEqual(THRESHOLDS, expected)


if __name__ == "__main__":
    unittest.main()
