"""Read and summarize Windows host sampling records."""

from __future__ import annotations

import json
import math
from pathlib import Path
from typing import Any, Iterable, Sequence


class HostStatsError(ValueError):
    """Raised when a host statistics JSONL file is malformed."""


def read_host_stats(path: Path) -> list[dict]:
    """Read host statistics records from a UTF-8 (optionally BOM-prefixed) file."""

    source = Path(path)
    records: list[dict] = []
    has_start = False
    try:
        handle = source.open(encoding="utf-8-sig")
    except OSError as exc:
        raise HostStatsError(f"{source}: {exc}") from exc

    with handle:
        for line_number, raw_line in enumerate(handle, start=1):
            if not raw_line.strip():
                continue
            try:
                value = json.loads(raw_line)
            except json.JSONDecodeError as exc:
                detail = exc.msg
                raise HostStatsError(f"{source}:{line_number}: {detail}") from exc
            if not isinstance(value, dict):
                raise HostStatsError(
                    f"{source}:{line_number}: expected a JSON object"
                )
            records.append(value)
            if value.get("type") == "start":
                has_start = True

    if not has_start:
        raise HostStatsError(f"{source}: no start line")
    return records


def _nearest_rank(values: Sequence[float], percentile: float = 95.0) -> float:
    if not values:
        raise ValueError("cannot compute a percentile of an empty sequence")
    rank = max(1, math.ceil(percentile / 100.0 * len(values)))
    return float(sorted(values)[rank - 1])


def _number(value: Any) -> float | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    number = float(value)
    if not math.isfinite(number):
        return None
    return number


def _values(records: Iterable[dict], key: str) -> list[float]:
    result: list[float] = []
    for record in records:
        value = _number(record.get(key))
        if value is not None:
            result.append(value)
    return result


def _rounded_mean(values: Sequence[float]) -> float | None:
    if not values:
        return None
    return round(sum(values) / len(values), 3)


def _rounded_p95(values: Sequence[float]) -> float | None:
    if not values:
        return None
    return round(_nearest_rank(values), 3)


def _counter_rate(records: Sequence[dict], key: str) -> float | None:
    """Return Mbps between the first and last usable counter records."""

    usable: list[tuple[int, float]] = []
    for record in records:
        timestamp = _number(record.get("t_us"))
        counter = _number(record.get(key))
        if timestamp is None or counter is None:
            continue
        usable.append((int(timestamp), counter))
    if len(usable) < 2:
        return None
    for (_, previous), (_, current) in zip(usable, usable[1:]):
        if current < previous:
            return None
    first_t, first_value = usable[0]
    last_t, last_value = usable[-1]
    elapsed_us = last_t - first_t
    delta = last_value - first_value
    if elapsed_us <= 0 or delta < 0:
        return None
    rate = delta * 8.0 / (elapsed_us / 1_000_000.0) / 1_000_000.0
    return round(rate, 3)


def _window(records: Sequence[dict], bounds: tuple[int, int]) -> list[dict]:
    start, end = bounds
    return [
        record
        for record in records
        if (
            (timestamp := _number(record.get("t_us"))) is not None
            and start <= timestamp <= end
        )
    ]


def summarize(
    lines: list[dict],
    window_us: tuple[int, int],
    baseline_us: tuple[int, int] | None = None,
) -> tuple[dict | None, dict | None]:
    """Summarize samples in a measurement window and optional idle baseline."""

    start = next((record for record in lines if record.get("type") == "start"), None)
    if start is None:
        raise HostStatsError("host statistics have no start line")

    samples = [
        record
        for record in lines
        if record.get("type") == "sample" and _number(record.get("t_us")) is not None
    ]
    samples.sort(key=lambda record: _number(record["t_us"]) or 0.0)
    if samples:
        samples = samples[1:]

    selected = _window(samples, window_us)
    if len(selected) < 2:
        return None, None

    cpu_values = _values(selected, "cpu_pct")
    if start.get("gpu_unavailable"):
        gpu_mean = None
        gpu_p95 = None
        gpu_encode_mean = None
    else:
        gpu_values: list[float] = []
        gpu_encode_values: list[float] = []
        for record in selected:
            gpu = record.get("gpu")
            numeric_gpu = (
                [_number(value) for value in gpu.values()]
                if isinstance(gpu, dict)
                else []
            )
            gpu_values.append(max((value for value in numeric_gpu if value is not None), default=0.0))
            encode = _number(gpu.get("VideoEncode", 0)) if isinstance(gpu, dict) else 0.0
            gpu_encode_values.append(encode if encode is not None else 0.0)
        gpu_mean = _rounded_mean(gpu_values)
        gpu_p95 = _rounded_p95(gpu_values)
        gpu_encode_mean = _rounded_mean(gpu_encode_values)

    host_usage = {
        "cpu_pct_mean": _rounded_mean(cpu_values),
        "cpu_pct_p95": _rounded_p95(cpu_values),
        "gpu_pct_mean": gpu_mean,
        "gpu_pct_p95": gpu_p95,
        "gpu_encode_pct_mean": gpu_encode_mean,
        "processes": start.get("names", []),
        "samples": len(selected),
    }

    tx_rate = _counter_rate(selected, "nic_tx_bytes")
    rx_rate = _counter_rate(selected, "nic_rx_bytes")
    baseline_tx: float | None = None
    baseline_rx: float | None = None
    baseline_records: list[dict] = []
    if baseline_us is not None:
        baseline_records = _window(samples, baseline_us)
        if len(baseline_records) >= 2:
            baseline_tx = _counter_rate(baseline_records, "nic_tx_bytes")
            baseline_rx = _counter_rate(baseline_records, "nic_rx_bytes")

    host_tx = tx_rate
    host_rx = rx_rate
    if baseline_tx is not None and tx_rate is not None:
        host_tx = round(max(0.0, tx_rate - baseline_tx), 3)
    if baseline_rx is not None and rx_rate is not None:
        host_rx = round(max(0.0, rx_rate - baseline_rx), 3)

    bandwidth = {
        "host_tx_mbps": host_tx,
        "host_rx_mbps": host_rx,
        "baseline_tx_mbps": baseline_tx,
        "method": "nic-counters",
    }
    return host_usage, bandwidth
