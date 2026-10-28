from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

from benchlib import analyze


TIME_MOD = 1 << 36
HOST_START_MS = 1_792_507_200_000


def synth_lines(
    n: int,
    latency_ms: float,
    offset_us: float,
    start_us: int,
    fps: float = 60,
    drop: set[int] | None = None,
    fail_every: int = 0,
    wrap_at: int | None = None,
    host_start_ms: int = HOST_START_MS,
) -> list[dict]:
    del wrap_at
    drop = drop or set()
    lines = [{"type": "calib", "t_us": start_us, "rect": [1, 2, 3, 4], "host_w": 1920, "host_h": 1080}]
    for index in range(n):
        host_ms = int(host_start_ms + index * 1000 / fps)
        t_us = int(host_ms * 1000 - offset_us + latency_ms * 1000)
        if index in drop:
            continue
        code = host_ms % TIME_MOD
        if fail_every > 0 and (index + 1) % fail_every == 0:
            lines.append({"type": "frame", "t_us": t_us, "code": None, "err": "invalid"})
        lines.append({"type": "frame", "t_us": t_us, "code": code})
    lines.append({"type": "end", "t_us": lines[-1]["t_us"] + 1, "reason": "end-code"})
    return lines


class AnalyzeTests(unittest.TestCase):
    def test_constant_latency(self) -> None:
        lines = synth_lines(600, 40, 5000, HOST_START_MS * 1000 - 5000 + 40000)
        result = analyze.analyze_frames(
            lines,
            {"before": {"t_us": lines[1]["t_us"], "offset_us": 5000}, "after": None},
        )
        self.assertEqual(result["latency_ms"]["p50"], 40.0)
        self.assertEqual(result["latency_ms"]["p95"], 40.0)
        self.assertAlmostEqual(result["delivery"]["display_fps"], 60, delta=0.5)
        self.assertAlmostEqual(result["delivery"]["longest_freeze_ms"], 16.7, delta=1)
        self.assertEqual(result["delivery"]["invalid_latency"], 0)

    def test_warmup_and_freeze(self) -> None:
        lines = synth_lines(600, 40, 0, HOST_START_MS * 1000 + 40000)
        first_t = lines[1]["t_us"]
        for line in lines:
            if line.get("type") == "frame" and line["t_us"] < first_t + 2_000_000:
                line["t_us"] += 460_000
        result = analyze.analyze_frames(lines, None)
        self.assertEqual(result["latency_ms"]["p95"], 40.0)
        frozen = analyze.analyze_frames(
            synth_lines(600, 40, 0, HOST_START_MS * 1000 + 40000, drop=set(range(300, 337))),
            None,
        )
        self.assertEqual(frozen["delivery"]["freezes_over_500ms"], 1)
        self.assertAlmostEqual(frozen["delivery"]["longest_freeze_ms"], 633, delta=20)

    def test_decode_failures_and_invalid(self) -> None:
        result = analyze.analyze_frames(
            synth_lines(600, 40, 0, HOST_START_MS * 1000 + 40000, fail_every=10), None
        )
        self.assertAlmostEqual(result["delivery"]["decode_fail_ratio"], 0.0909, delta=0.001)
        self.assertEqual(result["delivery"]["invalid_latency"], 0)

        lines = synth_lines(600, 40, 0, HOST_START_MS * 1000 + 40000)
        for index in range(3):
            host_ms = HOST_START_MS + int(400 * 1000 / 60) + index
            lines.append({
                "type": "frame",
                "t_us": host_ms * 1000 - 50_000,
                "code": host_ms % TIME_MOD,
            })
        result = analyze.analyze_frames(lines, None)
        self.assertEqual(result["delivery"]["invalid_latency"], 3)

    def test_offset_wrap_and_helpers(self) -> None:
        before = {"t_us": 1_000, "offset_us": 5_000}
        after = {"t_us": 2_000, "offset_us": 6_000}
        self.assertEqual(analyze.offset_at(1_000, {"before": before, "after": after}), 5_000)
        self.assertEqual(analyze.offset_at(1_500, {"before": before, "after": after}), 5_500)
        self.assertEqual(analyze.offset_at(3_000, {"before": before, "after": after}), 6_000)
        self.assertEqual(analyze.offset_at(1_500, {"before": before, "after": None}), 5_000)
        self.assertEqual(analyze.unwrap_ms(5, TIME_MOD * 3 + 10), TIME_MOD * 3 + 5)
        self.assertEqual(analyze.unwrap_ms(TIME_MOD - 5, TIME_MOD * 3 + 2), TIME_MOD * 3 - 5)

    def test_no_calibration_and_jsonl(self) -> None:
        with self.assertRaisesRegex(analyze.AnalyzeError, "no calibration"):
            analyze.analyze_frames([], None)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "frames.jsonl"
            path.write_text('{"type":"calib"}\n{}\n{bad}\n', encoding="utf-8")
            with self.assertRaisesRegex(analyze.AnalyzeError, r":3:"):
                analyze.read_jsonl(path)

    def test_analyze_run(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            lines = synth_lines(600, 40, 0, HOST_START_MS * 1000 + 40000)
            (root / "frames.jsonl").write_text(
                "\n".join(json.dumps(line) for line in lines) + "\n", encoding="utf-8"
            )
            (root / "clock.json").write_text(
                json.dumps({"before": {"t_us": 0, "offset_us": 0}, "after": None}),
                encoding="utf-8",
            )
            result = analyze.analyze_run(root)
            self.assertEqual(result["latency_ms"]["p50"], 40.0)
            (root / "clock.json").unlink()
            self.assertEqual(analyze.analyze_run(root)["latency_ms"]["p50"], 40.0)


if __name__ == "__main__":
    unittest.main()
