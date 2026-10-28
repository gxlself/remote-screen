from __future__ import annotations

import math


def percentile(sorted_values: list[float], p: float) -> float:
    """Return a nearest-rank percentile without mutating the input sample."""
    if not sorted_values:
        raise ValueError("cannot compute a percentile of an empty sample")
    try:
        finite_p = math.isfinite(p)
    except (OverflowError, TypeError):
        finite_p = False
    if not finite_p or not 0 < p <= 100:
        raise ValueError("percentile must be in (0, 100]")
    ordered = sorted(sorted_values)
    rank = max(1, math.ceil(p / 100.0 * len(ordered)))
    return ordered[rank - 1]


def summarize(values: list[float]) -> dict | None:
    """Return the standard latency summary, or ``None`` for no samples."""
    if not values:
        return None
    ordered = sorted(values)
    return {
        "n": len(ordered),
        "p50": round(percentile(ordered, 50), 3),
        "p95": round(percentile(ordered, 95), 3),
        "p99": round(percentile(ordered, 99), 3),
        "max": round(ordered[-1], 3),
        "mean": round(sum(ordered) / len(ordered), 3),
    }
