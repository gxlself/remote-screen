from __future__ import annotations

import unittest

from benchlib import stats


class StatsTests(unittest.TestCase):
    def test_nearest_rank_and_summary(self) -> None:
        values = list(range(1, 21))
        summary = stats.summarize(values)
        self.assertEqual(summary["p50"], 10)
        self.assertEqual(summary["p95"], 19)
        self.assertEqual(summary["max"], 20)
        self.assertEqual(summary["mean"], 10.5)
        self.assertEqual(stats.percentile([4, 1, 3, 2], 95), 4)
        self.assertEqual(stats.summarize([4, 1, 3, 2])["p95"], 4)

    def test_empty(self) -> None:
        self.assertIsNone(stats.summarize([]))
        with self.assertRaises(ValueError):
            stats.percentile([], 50)

    def test_rounding(self) -> None:
        self.assertEqual(stats.summarize([1.23456, 2.0])["mean"], 1.617)


if __name__ == "__main__":
    unittest.main()
