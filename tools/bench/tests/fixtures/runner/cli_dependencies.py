from __future__ import annotations

"""CLI wiring stand-ins used only until the V9F modules are merged."""

import importlib.util
import sys
import types


def install():
    if importlib.util.find_spec("benchlib.judge") is not None:
        return
    judge = types.ModuleType("benchlib.judge")

    def evaluate(current, baseline, thresholds):
        current_p95 = current["results"][0]["latency_ms"]["p95"]
        previous_p95 = baseline["results"][0]["latency_ms"]["p95"] if baseline else current_p95
        regression = current_p95 > previous_p95 * 1.2 and current_p95 - previous_p95 > 5
        return {"schema": 1, "items": [{"level": "regression" if regression else "info", "message": "CLI fixture"}]}

    judge.judge = evaluate
    judge.exit_code = lambda verdict: int(any(item["level"] == "regression" for item in verdict["items"]))
    sys.modules["benchlib.judge"] = judge
