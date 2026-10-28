from __future__ import annotations

import io
import json
import os
import sys
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from datetime import datetime, timedelta, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import regress

NOW = datetime(2026, 10, 21, 12, 0, tzinfo=timezone.utc)


def write_report(
    directory: Path,
    name: str,
    p95: float,
    *,
    shaping: str = "faultnet-delay-2ms-jitter-1ms-100mbit",
    timestamp: datetime = NOW,
    use_timestamp_us: bool = False,
) -> Path:
    path = directory / name
    path.write_text(
        json.dumps(
            {
                "schema": 1,
                **(
                    {"timestamp_us": int(timestamp.timestamp() * 1_000_000)}
                    if use_timestamp_us
                    else {"timestamp": timestamp.isoformat().replace("+00:00", "Z")}
                ),
                "shaping": shaping,
                "capture_to_receive_ms": {"p50": 1.0, "p95": p95},
            }
        ),
        encoding="utf-8",
    )
    return path


class RegressionTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(dir=os.environ.get("TMPDIR"))
        self.root = Path(self.temp.name)
        self.history = self.root / "history"
        self.history.mkdir()

    def tearDown(self) -> None:
        self.temp.cleanup()

    def run_regress(self, current: Path, *extra: str) -> tuple[int, str, str]:
        stdout = io.StringIO()
        stderr = io.StringIO()
        with redirect_stdout(stdout), redirect_stderr(stderr):
            status = regress.main(
                [
                    "--history",
                    str(self.history),
                    "--current",
                    str(current),
                    *extra,
                ]
            )
        return status, stdout.getvalue(), stderr.getvalue()

    def test_no_history_is_insufficient_and_is_archived(self) -> None:
        current = write_report(self.root, "current.json", 5.0)

        status, output, _ = self.run_regress(current)

        self.assertEqual(status, 0)
        self.assertIn("insufficient history", output)
        self.assertEqual(len(list(self.history.glob("*.json"))), 1)

    def test_future_history_is_ignored(self) -> None:
        for index in range(3):
            write_report(
                self.history,
                f"future-{index}.json",
                100.0,
                timestamp=NOW + timedelta(days=index + 1),
            )
        current = write_report(self.root, "current.json", 5.0)

        status, output, _ = self.run_regress(current)

        self.assertEqual(status, 0)
        self.assertIn("insufficient history: 1", output)

    def test_two_samples_including_current_are_insufficient(self) -> None:
        write_report(self.history, "one.json", 5.0, timestamp=NOW - timedelta(days=1))
        current = write_report(self.root, "current.json", 20.0)

        status, output, _ = self.run_regress(current)

        self.assertEqual(status, 0)
        self.assertIn("insufficient history: 2", output)

    def test_normal_result_uses_median(self) -> None:
        for index, p95 in enumerate((4.0, 5.0)):
            write_report(
                self.history,
                f"history-{index}.json",
                p95,
                timestamp=NOW - timedelta(days=index + 1),
            )
        current = write_report(self.root, "current.json", 5.0)

        status, output, _ = self.run_regress(current)

        self.assertEqual(status, 0)
        self.assertIn("latency ok", output)
        self.assertIn("median 4.500 ms", output)
        self.assertIn("n=2", output)

    def test_over_threshold_is_a_regression(self) -> None:
        for index, p95 in enumerate((4.0, 5.0)):
            write_report(
                self.history,
                f"history-{index}.json",
                p95,
                timestamp=NOW - timedelta(days=index + 1),
            )
        current = write_report(self.root, "current.json", 5.41)

        status, output, _ = self.run_regress(current)

        self.assertEqual(status, 1)
        self.assertIn("LATENCY REGRESSION", output)

    def test_threshold_boundary_is_not_a_regression(self) -> None:
        for index, p95 in enumerate((4.0, 5.0)):
            write_report(
                self.history,
                f"history-{index}.json",
                p95,
                timestamp=NOW - timedelta(days=index + 1),
            )
        current = write_report(self.root, "current.json", 5.3)

        status, output, _ = self.run_regress(current)

        self.assertEqual(status, 0)
        self.assertIn("latency ok", output)

    def test_just_over_threshold_is_a_regression(self) -> None:
        for index, p95 in enumerate((4.0, 5.0)):
            write_report(
                self.history,
                f"history-{index}.json",
                p95,
                timestamp=NOW - timedelta(days=index + 1),
            )
        current = write_report(self.root, "current.json", 5.4001)

        status, output, _ = self.run_regress(current)

        self.assertEqual(status, 1)
        self.assertIn("LATENCY REGRESSION", output)

    def test_other_shaping_is_ignored(self) -> None:
        for index in range(3):
            write_report(
                self.history,
                f"other-{index}.json",
                100.0,
                shaping="netem-wifi-good",
                timestamp=NOW - timedelta(days=index + 1),
            )
        current = write_report(self.root, "current.json", 5.0)

        status, output, _ = self.run_regress(current)

        self.assertEqual(status, 0)
        self.assertIn("insufficient history", output)

    def test_expired_samples_are_ignored(self) -> None:
        write_report(
            self.history,
            "old.json",
            100.0,
            timestamp=NOW - timedelta(days=8),
        )
        write_report(self.history, "fresh.json", 5.0, timestamp=NOW - timedelta(days=1))
        current = write_report(self.root, "current.json", 5.5)

        status, output, _ = self.run_regress(current, "--days", "7")

        self.assertEqual(status, 0)
        self.assertIn("insufficient history: 2", output)

    def test_filename_timestamp_is_a_fallback(self) -> None:
        path = self.history / "latency-20261020T120000Z.json"
        path.write_text(
            json.dumps(
                {
                    "shaping": "faultnet-delay-2ms-jitter-1ms-100mbit",
                    "capture_to_receive_ms": {"p95": 5.0},
                }
            ),
            encoding="utf-8",
        )
        current = write_report(self.root, "current.json", 5.5)

        self.assertEqual(regress.read_sample(path).timestamp.date().isoformat(), "2026-10-20")
        status, output, _ = self.run_regress(current)
        self.assertEqual(status, 0)
        self.assertIn("insufficient history: 2", output)

    def test_timestamp_us_is_preferred(self) -> None:
        path = self.history / "latency-20200101T000000Z.json"
        path.write_text(
            json.dumps(
                {
                    "shaping": "faultnet-delay-2ms-jitter-1ms-100mbit",
                    "capture_to_receive_ms": {"p95": 5.0},
                    "timestamp_us": int(NOW.timestamp() * 1_000_000),
                }
            ),
            encoding="utf-8",
        )
        current = write_report(self.root, "current.json", 5.5)

        sample = regress.read_sample(path)
        self.assertIsNotNone(sample)
        self.assertEqual(sample.timestamp, NOW)
        status, output, _ = self.run_regress(current)
        self.assertEqual(status, 0)
        self.assertIn("insufficient history: 2", output)

    def test_current_is_not_counted_in_history(self) -> None:
        for index, name in enumerate(
            (
                "latency-20261020T120000Z.json",
                "latency-20261019T120000Z.json",
                "latency-20261018T120000Z.json",
            )
        ):
            write_report(
                self.history,
                name,
                5.0,
                timestamp=NOW - timedelta(days=index + 1),
            )
        current = self.history / "latency-20261021T120000Z.json"
        write_report(current.parent, current.name, 5.0)

        status, output, _ = self.run_regress(current)

        self.assertEqual(status, 0)
        self.assertIn("p95 5.000 ms", output)
        self.assertIn("n=3", output)

    def test_invalid_current_returns_usage_error(self) -> None:
        current = self.root / "bad.json"
        current.write_text("{", encoding="utf-8")

        status, _, error = self.run_regress(current)

        self.assertEqual(status, 2)
        self.assertIn("error:", error)


if __name__ == "__main__":
    unittest.main()
