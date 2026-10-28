import io
import tempfile
import unittest
from contextlib import redirect_stderr, redirect_stdout
from pathlib import Path

import summarize


class SummarizeTests(unittest.TestCase):
    def read_text(self, content):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "samples.csv"
            path.write_text(content, encoding="utf-8")
            return summarize.read_samples(path)

    def test_wrap_adds_one_minute(self):
        self.assertEqual(
            self.read_text("host_ms,phone_ms\n59900,100\n"),
            [200],
        )

    def test_negative_delta_wraps(self):
        self.assertEqual(
            self.read_text("host_ms,phone_ms\n59950,150\n"),
            [200],
        )

    def test_empty_input_is_an_error(self):
        with self.assertRaisesRegex(ValueError, "empty"):
            self.read_text("")

    def test_header_only_is_an_error(self):
        with self.assertRaisesRegex(ValueError, "no samples"):
            self.read_text("host_ms,phone_ms\n")

    def test_p95_nearest_rank(self):
        values = list(range(1, 21))
        # ceil(0.95 * 20) = 19, so the 19th sorted value is selected.
        self.assertEqual(summarize.nearest_rank(values, 95), 19)
        self.assertEqual(summarize.nearest_rank([1, 2, 3, 4], 50), 2)
        self.assertEqual(summarize.nearest_rank([1, 2, 3, 4, 5], 50), 3)

    def test_one_sample_p95_is_the_sample(self):
        summary = summarize.summarize([4.2])
        self.assertEqual(summary.p95, 4.2)
        self.assertEqual(summarize.format_ms(4.2), "4.2 ms")

    def test_summary_and_markdown(self):
        summary = summarize.summarize([10, 20, 30, 40])
        self.assertEqual(summary.count, 4)
        self.assertEqual(summary.p50, 20)
        self.assertEqual(summary.p95, 40)
        self.assertEqual(summary.maximum, 40)
        self.assertEqual(summary.mean, 25)
        self.assertEqual(
            summarize.format_markdown(summary),
            "| 4 | 20 ms | 40 ms | 40 ms |",
        )

    def test_cli_reports_bad_csv(self):
        out = io.StringIO()
        err = io.StringIO()
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "bad.csv"
            path.write_text("host_ms,phone_ms\n1\n", encoding="utf-8")
            with redirect_stdout(out), redirect_stderr(err):
                code = summarize.main([str(path)])
        self.assertEqual(code, 2)
        self.assertIn("line 2", err.getvalue())
        self.assertEqual(out.getvalue(), "")

    def test_cli_markdown(self):
        out = io.StringIO()
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "samples.csv"
            path.write_text(
                "host_ms,phone_ms\n0,20\n20,60\n30,90\n10,90\n",
                encoding="utf-8",
            )
            with redirect_stdout(out):
                code = summarize.main(["--md", str(path)])
        self.assertEqual(code, 0)
        self.assertEqual(out.getvalue(), "| 4 | 40 ms | 80 ms | 80 ms |\n")


if __name__ == "__main__":
    unittest.main()
