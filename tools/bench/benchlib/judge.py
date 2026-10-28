"""Regression and comparison rules for the V9 benchmark."""

from __future__ import annotations

from collections import Counter, defaultdict
from statistics import median
from typing import Any


METRICS: dict[str, tuple[str, str]] = {
    "latency_p50": ("latency_ms", "p50"),
    "latency_p95": ("latency_ms", "p95"),
    "latency_p99": ("latency_ms", "p99"),
    "latency_max": ("latency_ms", "max"),
    "display_fps": ("delivery", "display_fps"),
    "longest_freeze_ms": ("delivery", "longest_freeze_ms"),
    "freezes_over_100ms": ("delivery", "freezes_over_100ms"),
    "host_tx_mbps": ("bandwidth", "host_tx_mbps"),
    "cpu_pct_mean": ("host_usage", "cpu_pct_mean"),
    "gpu_pct_mean": ("host_usage", "gpu_pct_mean"),
    "ssim": ("quality", "ssim"),
    "samples": ("latency_ms", "n"),
}

PRODUCT_LABELS = {
    "remotescreen": "本产品",
    "rustdesk": "RustDesk",
    "moonlight": "Moonlight + Sunshine",
    "local": "本机基线",
}
PRODUCT_ORDER = ("remotescreen", "rustdesk", "moonlight", "local")
SCENES = ("static", "scroll", "video", "drag")
LEVEL_ORDER = {"regression": 0, "fail": 1, "warn": 2, "info": 3}


def _value(result: dict[str, Any], metric: str) -> Any:
    """Read one metric from an I3 result, returning ``None`` when absent."""

    path = METRICS.get(metric)
    if path is None:
        return None
    value: Any = result
    for key in path:
        if not isinstance(value, dict):
            return None
        value = value.get(key)
    return value


def cell_key(result: dict[str, Any]) -> tuple[str, str, str, str]:
    """Return ``(scene, network, host OS, controller OS)`` for a result."""

    scene = result.get("scene")
    network = result.get("network")
    topology = result.get("topology")
    return (
        str(scene) if scene is not None else "",
        str(network.get("profile")) if isinstance(network, dict) and network.get("profile") is not None else "",
        str(topology.get("host_os")) if isinstance(topology, dict) and topology.get("host_os") is not None else "",
        str(topology.get("controller_os")) if isinstance(topology, dict) and topology.get("controller_os") is not None else "",
    )


def _numeric(value: Any) -> bool:
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def aggregate(results: list[dict]) -> dict:
    """Aggregate repeated results by product and benchmark cell.

    A cell is usable when at least one of its runs is ``ok``.  Every metric is
    then the median of the available values from those usable runs.  Failed
    cells retain the most common status so callers can explain why they could
    not be compared.
    """

    grouped: defaultdict[tuple[str, tuple[str, str, str, str]], list[dict]] = defaultdict(list)
    for result in results:
        product = result.get("product")
        product_id = product.get("id") if isinstance(product, dict) else ""
        grouped[(str(product_id), cell_key(result))].append(result)

    aggregated: dict = {}
    for key, group in grouped.items():
        ok_results = [result for result in group if result.get("status") == "ok"]
        if ok_results:
            metrics: dict[str, Any] = {}
            for metric in METRICS:
                values = [_value(result, metric) for result in ok_results]
                values = [value for value in values if _numeric(value)]
                metrics[metric] = median(values) if values else None
            status = "ok"
        else:
            counts = Counter(result.get("status") for result in group)
            highest = max(counts.values())
            candidates = {status for status, count in counts.items() if count == highest}
            if "failed" in candidates:
                status = "failed"
            elif "invalid" in candidates:
                status = "invalid"
            else:
                status = sorted((str(candidate) for candidate in candidates))[0]
            metrics = {metric: None for metric in METRICS}
        aggregated[key] = {
            "status": status,
            "metrics": metrics,
            "n_runs": len(group),
            "results": group,
        }
    return aggregated


def _round(value: Any) -> Any:
    if _numeric(value):
        return round(value, 3)
    return value


def _message_number(value: Any) -> str:
    if not _numeric(value):
        return "—"
    text = f"{value:.3f}".rstrip("0").rstrip(".")
    return text if "." in text else f"{text}.0"


def _item(
    level: str,
    key: tuple[str, str, str, str],
    metric: str,
    current: Any,
    baseline: Any,
    limit: Any,
    message: str,
) -> dict:
    return {
        "level": level,
        "product": "remotescreen",
        "scene": key[0],
        "network": key[1],
        "metric": metric,
        "current": _round(current),
        "baseline": _round(baseline),
        "limit": _round(limit),
        "message": message,
    }


def _comparison_message(metric: str, current: float, baseline: float, limit: float) -> str:
    names = {
        "latency_p50": "延迟 p50",
        "latency_p95": "延迟 p95",
        "display_fps": "帧率",
        "longest_freeze_ms": "最长卡顿",
        "ssim": "SSIM",
        "host_tx_mbps": "Host 带宽",
        "cpu_pct_mean": "Host CPU",
        "gpu_pct_mean": "Host GPU",
    }
    units = {
        "latency_p50": " ms",
        "latency_p95": " ms",
        "display_fps": "",
        "longest_freeze_ms": " ms",
        "ssim": "",
        "host_tx_mbps": " Mbps",
        "cpu_pct_mean": "%",
        "gpu_pct_mean": "%",
    }
    name = names.get(metric, metric)
    unit = units.get(metric, "")
    if metric in {"display_fps", "ssim"}:
        return (
            f"{name} {_message_number(current)}{unit} 低于下限 "
            f"{_message_number(limit)}{unit}（基线 {_message_number(baseline)}{unit}）"
        )
    return (
        f"{name} {_message_number(current)}{unit} 超过上限 "
        f"{_message_number(limit)}{unit}（基线 {_message_number(baseline)}{unit}）"
    )


def _best_opponent(aggregated: dict, key: tuple, metric: str) -> tuple[str, float] | None:
    candidates: list[tuple[str, float]] = []
    for product in ("rustdesk", "moonlight"):
        group = aggregated.get((product, key))
        if not group or group.get("status") != "ok":
            continue
        value = group.get("metrics", {}).get(metric)
        if _numeric(value):
            candidates.append((product, float(value)))
    if not candidates:
        return None
    return min(candidates, key=lambda item: item[1])


def _cell_sort(item: dict) -> tuple:
    return (
        SCENES.index(item["scene"]) if item["scene"] in SCENES else len(SCENES),
        item["scene"],
        item["network"],
        item.get("metric", ""),
    )


def judge(current: dict, baseline: dict | None, thresholds: dict) -> dict:
    """Evaluate the current summary against an optional previous summary."""

    current_aggregated = aggregate(current.get("results", []))
    baseline_aggregated = aggregate(baseline.get("results", [])) if baseline else {}
    self_product = thresholds.get("self_product", "remotescreen")
    items: list[dict] = []

    if baseline is None:
        items.append(
            {
                "level": "info",
                "product": self_product,
                "scene": "",
                "network": "",
                "metric": "baseline",
                "current": None,
                "baseline": None,
                "limit": None,
                "message": "no baseline",
            }
        )

    self_groups = {
        key: group
        for (product, key), group in current_aggregated.items()
        if product == self_product
    }
    regression_thresholds = thresholds.get("regression", {})
    warn_thresholds = thresholds.get("warn", {})
    absolute_thresholds = thresholds.get("absolute", {})

    for key, current_group in self_groups.items():
        current_status = current_group.get("status")
        baseline_group = baseline_aggregated.get((self_product, key))

        if baseline is not None:
            if baseline_group is None:
                items.append(_item("info", key, "status", current_status, None, None, "new cell"))
            elif current_status != "ok" or baseline_group.get("status") != "ok":
                baseline_status = baseline_group.get("status")
                items.append(
                    _item(
                        "info",
                        key,
                        "status",
                        current_status,
                        baseline_status,
                        None,
                        f"cannot compare: current={current_status} baseline={baseline_status}",
                    )
                )
            else:
                current_metrics = current_group.get("metrics", {})
                baseline_metrics = baseline_group.get("metrics", {})

                comparisons = (
                    ("latency_p50", "regression"),
                    ("latency_p95", "regression"),
                    ("display_fps", "regression"),
                    ("longest_freeze_ms", "regression"),
                    ("ssim", "regression"),
                    ("host_tx_mbps", "warn"),
                    ("cpu_pct_mean", "warn"),
                    ("gpu_pct_mean", "warn"),
                )
                for metric, level in comparisons:
                    cur = current_metrics.get(metric)
                    base = baseline_metrics.get(metric)
                    if not _numeric(cur) or not _numeric(base):
                        continue
                    config = (regression_thresholds if level == "regression" else warn_thresholds).get(metric, {})
                    ratio = config.get("ratio")
                    abs_key = "abs_ms" if metric in {"latency_p50", "latency_p95", "longest_freeze_ms"} else "abs"
                    abs_limit = config.get(abs_key)
                    drop = config.get("drop_ratio")
                    drop_amount = config.get("drop")
                    triggered = False
                    limit: float | None = None
                    if metric in {"latency_p50", "latency_p95", "longest_freeze_ms"} and _numeric(ratio) and _numeric(abs_limit):
                        triggered = cur > base * (1 + ratio) and cur - base > abs_limit
                        limit = max(base * (1 + ratio), base + abs_limit)
                    elif metric == "display_fps" and _numeric(drop):
                        triggered = cur < base * (1 - drop)
                        limit = base * (1 - drop)
                    elif metric == "ssim" and _numeric(drop_amount):
                        triggered = cur < base - drop_amount
                        limit = base - drop_amount
                    elif metric == "host_tx_mbps" and _numeric(ratio):
                        triggered = cur > base * (1 + ratio)
                        limit = base * (1 + ratio)
                    elif metric in {"cpu_pct_mean", "gpu_pct_mean"} and _numeric(ratio) and _numeric(abs_limit):
                        triggered = cur > base * (1 + ratio) and cur - base > abs_limit
                        limit = max(base * (1 + ratio), base + abs_limit)
                    if triggered and limit is not None:
                        items.append(
                            _item(
                                level,
                                key,
                                metric,
                                cur,
                                base,
                                limit,
                                _comparison_message(metric, cur, base, limit),
                            )
                        )

        if current_status == "ok":
            current_metrics = current_group.get("metrics", {})
            profile_limits = absolute_thresholds.get(key[1], {})
            freeze_limit = profile_limits.get("longest_freeze_ms") if isinstance(profile_limits, dict) else None
            freeze = current_metrics.get("longest_freeze_ms")
            if _numeric(freeze_limit) and _numeric(freeze) and freeze > freeze_limit:
                items.append(
                    _item(
                        "fail",
                        key,
                        "longest_freeze_ms",
                        freeze,
                        None,
                        freeze_limit,
                        f"最长卡顿 {_message_number(freeze)} ms 超过绝对上限 {_message_number(freeze_limit)} ms",
                    )
                )

            for metric, ratio_key in (("latency_p50", "latency_p50_ratio"), ("latency_p95", "latency_p95_ratio")):
                current_value = current_metrics.get(metric)
                best = _best_opponent(current_aggregated, key, metric)
                ratio = thresholds.get("vs_best", {}).get(ratio_key)
                if best is None or not _numeric(current_value) or not _numeric(ratio):
                    continue
                opponent_id, opponent_value = best
                limit = opponent_value * (1 + ratio)
                if current_value > limit:
                    opponent_label = PRODUCT_LABELS.get(opponent_id, opponent_id)
                    items.append(
                        _item(
                            "warn",
                            key,
                            f"vs_best_{'p50' if metric == 'latency_p50' else 'p95'}",
                            current_value,
                            opponent_value,
                            limit,
                            f"本产品 {metric.replace('latency_', '延迟 ')} {_message_number(current_value)} ms 高于最佳对手 {opponent_label} {_message_number(opponent_value)} ms（上限 {_message_number(limit)} ms）",
                        )
                    )

    items.sort(key=_cell_sort_with_level)
    return {
        "schema": 1,
        "rs_version": current.get("rs_version"),
        "baseline_version": baseline.get("rs_version") if baseline else None,
        "items": items,
        "regressions": sum(item["level"] == "regression" for item in items),
        "fails": sum(item["level"] == "fail" for item in items),
        "warns": sum(item["level"] == "warn" for item in items),
    }


def _cell_sort_with_level(item: dict) -> tuple:
    return (
        SCENES.index(item["scene"]) if item["scene"] in SCENES else len(SCENES),
        item.get("scene", ""),
        item.get("network", ""),
        LEVEL_ORDER.get(item.get("level"), len(LEVEL_ORDER)),
        item.get("metric", ""),
    )


def exit_code(verdict: dict) -> int:
    """Return the command exit code for a verdict."""

    return 1 if verdict.get("regressions", 0) + verdict.get("fails", 0) > 0 else 0
