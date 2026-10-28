import unittest
from pathlib import Path

from benchlib.hoststats import HostStatsError, read_host_stats, summarize


FIXTURES = Path(__file__).parent / "fixtures" / "hoststats"
BASIC_WINDOW = (3_000_000, 12_000_000)


def fixture(name: str) -> list[dict]:
    return read_host_stats(FIXTURES / name)


class HostStatsTests(unittest.TestCase):
    def test_basic_window_all(self):
        usage, bandwidth = summarize(fixture("basic.jsonl"), BASIC_WINDOW)
        self.assertIsNotNone(usage)
        self.assertIsNotNone(bandwidth)
        assert usage is not None
        assert bandwidth is not None
        self.assertEqual(usage["cpu_pct_mean"], 9.5)
        self.assertEqual(usage["cpu_pct_p95"], 14)
        self.assertEqual(usage["gpu_pct_mean"], 21.0)
        self.assertEqual(usage["gpu_pct_p95"], 30)
        self.assertEqual(usage["gpu_encode_pct_mean"], 18.0)
        self.assertEqual(usage["processes"], ["sunshine"])
        self.assertEqual(usage["samples"], 10)
        self.assertEqual(bandwidth["host_tx_mbps"], 10.0)
        self.assertEqual(bandwidth["host_rx_mbps"], 0.4)
        self.assertIsNone(bandwidth["baseline_tx_mbps"])

    def test_first_sample_dropped(self):
        usage, _ = summarize(fixture("basic.jsonl"), (2_000_000, 12_000_000))
        self.assertIsNotNone(usage)
        assert usage is not None
        self.assertEqual(usage["samples"], 10)
        self.assertEqual(usage["cpu_pct_mean"], 9.5)

    def test_baseline_subtracted(self):
        usage, bandwidth = summarize(
            fixture("baseline.jsonl"),
            (6_000_000, 12_000_000),
            (3_000_000, 5_000_000),
        )
        self.assertIsNotNone(usage)
        self.assertIsNotNone(bandwidth)
        assert bandwidth is not None
        self.assertEqual(bandwidth["host_tx_mbps"], 9.0)
        self.assertEqual(bandwidth["host_rx_mbps"], 0.32)
        self.assertEqual(bandwidth["baseline_tx_mbps"], 1.0)

    def test_counter_reset(self):
        lines = fixture("basic.jsonl")
        samples = [line for line in lines if line.get("type") == "sample"]
        samples[-1]["nic_tx_bytes"] = 1
        usage, bandwidth = summarize(lines, BASIC_WINDOW)
        self.assertIsNotNone(usage)
        self.assertIsNotNone(bandwidth)
        assert bandwidth is not None
        self.assertIsNone(bandwidth["host_tx_mbps"])
        self.assertEqual(bandwidth["host_rx_mbps"], 0.4)

    def test_too_few(self):
        self.assertEqual(
            summarize(fixture("basic.jsonl"), (3_000_000, 3_000_000)),
            (None, None),
        )

    def test_bom_ok(self):
        usage, bandwidth = summarize(fixture("bom.jsonl"), BASIC_WINDOW)
        self.assertIsNotNone(usage)
        self.assertIsNotNone(bandwidth)
        assert usage is not None
        assert bandwidth is not None
        self.assertEqual(usage["cpu_pct_mean"], 9.5)
        self.assertEqual(bandwidth["host_tx_mbps"], 10.0)

    def test_no_gpu(self):
        usage, bandwidth = summarize(fixture("nogpu.jsonl"), BASIC_WINDOW)
        self.assertIsNotNone(usage)
        self.assertIsNotNone(bandwidth)
        assert usage is not None
        self.assertIsNone(usage["gpu_pct_mean"])
        self.assertIsNone(usage["gpu_pct_p95"])
        self.assertIsNone(usage["gpu_encode_pct_mean"])
        self.assertEqual(usage["cpu_pct_mean"], 9.5)

    def test_bad_line(self):
        with self.assertRaisesRegex(HostStatsError, r":4:"):
            read_host_stats(FIXTURES / "bad_line.jsonl")

    def test_missing_start(self):
        with self.assertRaises(HostStatsError):
            read_host_stats(FIXTURES / "no_start.jsonl")

    def test_ps1_static_checks(self):
        script = (Path(__file__).parents[1] / "host" / "sample.ps1").read_bytes()
        text = script.decode("ascii")
        for required in (
            "Win32_PerfFormattedData_PerfProc_Process",
            "Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine",
            "Get-NetAdapterStatistics",
            "UTF8Encoding($false)",
        ):
            self.assertIn(required, text)
        for forbidden in ("Get-Counter", "Add-Content", "Out-File", "Write-Host"):
            self.assertNotIn(forbidden, text)
        self.assertLess(max(script, default=0), 0x80)


if __name__ == "__main__":
    unittest.main()
