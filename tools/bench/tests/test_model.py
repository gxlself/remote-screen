from __future__ import annotations

import json
import tempfile
import unittest
from datetime import datetime, timezone
from pathlib import Path

from benchlib import model


STARTED = datetime(2026, 10, 23, 10, 15, tzinfo=timezone.utc)


def make_result(run_id: str = "20261023T101500Z-moonlight-scroll-weak-r1") -> dict:
    return model.new_result(
        run_id=run_id,
        started_at=STARTED,
        product={"id": "moonlight"},
        scene="scroll",
        network={"profile": "weak"},
        seconds=60,
        repeat=1,
        bench={"git_rev": "abc", "rs_version": "0.5.0"},
        topology={"host_os": "windows"},
    )


class ModelTests(unittest.TestCase):
    def test_run_id_and_validation(self) -> None:
        self.assertEqual(
            model.make_run_id(STARTED, "moonlight", "scroll", "weak", 1),
            "20261023T101500Z-moonlight-scroll-weak-r1",
        )
        result = make_result(
            "20261023T101500Z-moonlight-scroll-wifi-good-r1"
        )
        self.assertEqual(model.validate_result(result), [])

    def test_new_result_order_and_defaults(self) -> None:
        result = make_result()
        self.assertEqual(
            list(result),
            [
                "schema",
                "kind",
                "run_id",
                "started_at",
                "bench",
                "product",
                "topology",
                "scene",
                "network",
                "repeat",
                "seconds",
                "status",
                "error",
                "clock",
                "latency_ms",
                "delivery",
                "bandwidth",
                "host_usage",
                "quality",
                "artifacts",
            ],
        )
        self.assertEqual(result["status"], "failed")
        self.assertEqual(result["error"], "not run")
        self.assertIsNone(result["clock"])
        self.assertEqual(model.validate_result(result), [])

    def test_classify(self) -> None:
        result = make_result()
        self.assertEqual(model.classify(result), ("failed", "not run"))
        result["status"] = "ok"
        result["latency_ms"] = {"n": 120}
        self.assertIn("120 < 300", model.classify(result)[1])
        result["latency_ms"] = {"n": 300}
        result["delivery"] = {"decode_fail_ratio": 0.06}
        self.assertEqual(model.classify(result)[0], "invalid")
        result["delivery"] = {
            "decode_fail_ratio": 0.0,
            "invalid_latency": 50,
            "distinct_codes": 3000,
        }
        self.assertEqual(model.classify(result)[0], "invalid")
        result["delivery"]["invalid_latency"] = 0
        self.assertEqual(model.classify(result), ("ok", None))

    def test_validation_errors(self) -> None:
        result = make_result()
        result["scene"] = "foo"
        self.assertTrue(any("scene" in error for error in model.validate_result(result)))
        result = make_result()
        result["product"]["id"] = "vnc"
        self.assertTrue(any("product.id" in error for error in model.validate_result(result)))
        result = make_result()
        result["latency_ms"] = {
            "n": 300,
            "p50": 20,
            "p95": 10,
            "p99": 30,
            "max": 40,
            "mean": 15,
        }
        self.assertTrue(any("p50" in error and "p95" in error for error in model.validate_result(result)))
        result = make_result()
        del result["delivery"]
        self.assertTrue(any("delivery" in error for error in model.validate_result(result)))

    def test_json_round_trip_and_bom(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "nested" / "result.json"
            value = {"text": "中文", "number": 3}
            model.write_json(path, value)
            self.assertEqual(model.read_json(path), value)
            self.assertFalse(path.with_name("result.json.tmp").exists())
            path.write_text("\ufeff" + json.dumps(value), encoding="utf-8")
            self.assertEqual(model.read_json(path), value)

    def test_load_results(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            model.write_json(root / "a" / "result.json", make_result("20261023T101500Z-moonlight-scroll-weak-r1"))
            model.write_json(root / "b" / "result.json", make_result("20261023T101500Z-moonlight-scroll-weak-r2"))
            bad = make_result()
            del bad["kind"]
            model.write_json(root / "c" / "result.json", bad)
            results, errors = model.load_results(root)
            self.assertEqual(len(results), 2)
            self.assertEqual(len(errors), 1)
            self.assertIn(str(root / "c" / "result.json"), errors[0])

    def test_summary(self) -> None:
        results = [
            make_result("20261023T101500Z-moonlight-scroll-weak-r2"),
            make_result("20261023T101500Z-moonlight-scroll-weak-r1"),
        ]
        summary = model.make_summary(results, "0.5.0", STARTED)
        self.assertEqual([item["run_id"] for item in summary["results"]], sorted(item["run_id"] for item in results))
        summary["results"][1]["scene"] = "bad"
        errors = model.validate_summary(summary)
        self.assertTrue(any(error.startswith("results[1]:") for error in errors))


if __name__ == "__main__":
    unittest.main()
