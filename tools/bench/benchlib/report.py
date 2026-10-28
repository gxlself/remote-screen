"""Markdown rendering for V9 benchmark summaries."""

from __future__ import annotations

from datetime import datetime
from typing import Any

from benchlib.judge import (
    PRODUCT_LABELS,
    PRODUCT_ORDER,
    SCENES,
    aggregate,
    cell_key,
)


SCENE_LABELS = {
    "static": "静态桌面",
    "scroll": "滚动网页",
    "video": "视频播放",
    "drag": "拖窗口",
}
STATUS_LABELS = {"ok": "ok", "invalid": "无效", "failed": "失败"}
LEVEL_LABELS = {"regression": "回归", "fail": "失败", "warn": "告警", "info": "说明"}
TABLE_HEADER = "| 产品 | 版本 | 延迟 p50 | p95 | p99 | max | 帧率 | 最长卡顿 | >100ms 卡顿 | 带宽 | Host CPU | Host GPU | SSIM | 样本 | 状态 |"
TABLE_SEPARATOR = "|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|"
QUALITY_HEADER = "| 日期 | 控制端 | 网络 | 分辨率 · 码率 | 场景 | 样本数 | p50 | p95 | max | 对照 |"
QUALITY_SEPARATOR = "|---|---|---|---|---|---|---|---|---|---|"


def _number(value: Any, decimals: int, suffix: str = "") -> str:
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        return "—"
    return f"{value:.{decimals}f}{suffix}"


def _integer(value: Any, suffix: str = "") -> str:
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        return "—"
    return f"{value:.0f}{suffix}"


def _date(value: Any) -> str:
    if not isinstance(value, str) or not value:
        return "—"
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00")).date().isoformat()
    except ValueError:
        return value[:10] if len(value) >= 10 else "—"


def _field(obj: Any, name: str) -> str:
    if not isinstance(obj, dict):
        return "—"
    value = obj.get(name)
    if value is None or value == "":
        return "—"
    return str(value)


def _product_id(result: dict) -> str:
    product = result.get("product")
    return str(product.get("id")) if isinstance(product, dict) and product.get("id") is not None else ""


def _product_label(product_id: str, result: dict | None = None) -> str:
    if product_id in PRODUCT_LABELS:
        return PRODUCT_LABELS[product_id]
    if result and isinstance(result.get("product"), dict):
        return _field(result["product"], "label")
    return product_id or "—"


def _version(result: dict | None) -> str:
    if not result:
        return "—"
    product = result.get("product")
    if not isinstance(product, dict):
        return "—"
    client = _field(product, "client_version")
    host = product.get("host_version")
    if host is not None and host != "":
        return f"{client} / {host}"
    return client


def _representative(results: list[dict], product_id: str | None = None) -> dict | None:
    for result in results:
        if product_id is None or _product_id(result) == product_id:
            return result
    return None


def _metric(group: dict | None, name: str) -> Any:
    if not group:
        return None
    return group.get("metrics", {}).get(name)


def _cell_sort(key: tuple[str, str, str, str]) -> tuple:
    scene, network, host_os, controller_os = key
    return (
        SCENES.index(scene) if scene in SCENES else len(SCENES),
        scene,
        network,
        host_os,
        controller_os,
    )


def _environment(summary: dict, results: list[dict]) -> list[str]:
    topology_result = next(
        (result for result in results if isinstance(result.get("topology"), dict)), None
    )
    topology = topology_result.get("topology") if topology_result else None
    host = " ".join(
        _field(topology, field)
        for field in ("host_os", "host_os_version")
    )
    host += " · " + " · ".join(_field(topology, field) for field in ("host_cpu", "host_gpu", "host_display"))
    controller = _field(topology, "controller_os") + " " + _field(topology, "controller_os_version")
    controller += " · " + _field(topology, "controller_model")
    link = _field(topology, "link")

    product_results: dict[str, dict] = {}
    for result in results:
        product_id = _product_id(result)
        product_results.setdefault(product_id, result)
    products = [
        f"{_product_label(product_id, product_results[product_id])} {_version(product_results[product_id])}"
        for product_id in PRODUCT_ORDER
        if product_id in product_results
    ]
    products.extend(
        f"{_product_label(product_id, product_results[product_id])} {_version(product_results[product_id])}"
        for product_id in product_results
        if product_id not in PRODUCT_ORDER
    )
    return [
        f"- Host：{host}",
        f"- 控制端：{controller}",
        f"- 链路：{link}",
        f"- 第三方版本：{', '.join(products) if products else '—'}",
    ]


def _table_row(product_id: str, group: dict, representative: dict | None) -> str:
    metrics = group.get("metrics", {})
    cells = [
        _product_label(product_id, representative),
        _version(representative),
        _number(metrics.get("latency_p50"), 1, " ms"),
        _number(metrics.get("latency_p95"), 1, " ms"),
        _number(metrics.get("latency_p99"), 1, " ms"),
        _number(metrics.get("latency_max"), 1, " ms"),
        _number(metrics.get("display_fps"), 1),
        _integer(metrics.get("longest_freeze_ms"), " ms"),
        _integer(metrics.get("freezes_over_100ms")),
        _number(metrics.get("host_tx_mbps"), 2, " Mbps"),
        _number(metrics.get("cpu_pct_mean"), 1, "%"),
        _number(metrics.get("gpu_pct_mean"), 1, "%"),
        _number(metrics.get("ssim"), 3),
        _integer(metrics.get("samples")),
        STATUS_LABELS.get(group.get("status"), str(group.get("status", "—"))),
    ]
    return "| " + " | ".join(cells) + " |"


def _opponent_line(groups: dict, key: tuple) -> str:
    product_group = groups.get(("remotescreen", key))
    if not product_group or product_group.get("status") != "ok":
        return "本产品比最佳对手：无可比对手"
    product_p50 = _metric(product_group, "latency_p50")
    product_p95 = _metric(product_group, "latency_p95")
    opponents = [
        (product, groups.get((product, key)))
        for product in ("rustdesk", "moonlight")
    ]
    best_p50 = min(
        ((product, _metric(group, "latency_p50")) for product, group in opponents if group and group.get("status") == "ok" and isinstance(_metric(group, "latency_p50"), (int, float))),
        key=lambda item: item[1],
        default=None,
    )
    best_p95 = min(
        ((product, _metric(group, "latency_p95")) for product, group in opponents if group and group.get("status") == "ok" and isinstance(_metric(group, "latency_p95"), (int, float))),
        key=lambda item: item[1],
        default=None,
    )
    if best_p50 is None or best_p95 is None or not isinstance(product_p50, (int, float)) or not isinstance(product_p95, (int, float)):
        return "本产品比最佳对手：无可比对手"
    p50_delta = product_p50 - best_p50[1]
    p95_delta = product_p95 - best_p95[1]
    p50_ratio = p50_delta / best_p50[1] * 100 if best_p50[1] else 0.0
    p95_ratio = p95_delta / best_p95[1] * 100 if best_p95[1] else 0.0
    return (
        "本产品比最佳对手："
        f"p50 {p50_delta:+.1f} ms（{p50_ratio:+.1f}%），"
        f"p95 {p95_delta:+.1f} ms（{p95_ratio:+.1f}%）"
    )


def _local_line(groups: dict, key: tuple) -> str | None:
    product = groups.get(("remotescreen", key))
    local = groups.get(("local", key))
    if not product or product.get("status") != "ok" or not local or local.get("status") != "ok":
        return None
    product_p50 = _metric(product, "latency_p50")
    local_p50 = _metric(local, "latency_p50")
    if not isinstance(product_p50, (int, float)) or not isinstance(local_p50, (int, float)):
        return None
    return (
        f"本机基线 p50 {local_p50:.1f} ms；"
        f"本产品扣除基线后 p50 {product_p50 - local_p50:.1f} ms"
    )


def _cell_section(summary_results: list[dict], groups: dict, key: tuple) -> list[str]:
    scene, network, _, _ = key
    lines = [f"## {SCENE_LABELS.get(scene, scene)} · {network}", "", TABLE_HEADER, TABLE_SEPARATOR]
    for product_id in PRODUCT_ORDER:
        group = groups.get((product_id, key))
        if not group:
            continue
        representative = _representative(summary_results, product_id)
        # Prefer a result from this exact cell when a product ran multiple cells.
        representative = next(
            (
                result
                for result in summary_results
                if _product_id(result) == product_id and cell_key(result) == key
            ),
            representative,
        )
        lines.append(_table_row(product_id, group, representative))
    lines.extend(["", _opponent_line(groups, key)])
    local_line = _local_line(groups, key)
    if local_line is not None:
        lines.extend(["", local_line])
    return lines


def _settings(result: dict | None) -> dict:
    if not result or not isinstance(result.get("product"), dict):
        return {}
    settings = result["product"].get("settings")
    return settings if isinstance(settings, dict) else {}


def _comparison_value(group: dict | None, metric: str) -> str:
    if not group or group.get("status") != "ok":
        return "—"
    value = _metric(group, metric)
    return _number(value, 1)


def _quality_section(summary: dict, results: list[dict], groups: dict, keys: list[tuple]) -> list[str]:
    generated = _date(summary.get("generated_at"))
    lines = ["## 09 格式（本产品）", "", QUALITY_HEADER, QUALITY_SEPARATOR]
    for key in keys:
        scene, network, _, _ = key
        group = groups.get(("remotescreen", key))
        representative = next(
            (
                result
                for result in results
                if _product_id(result) == "remotescreen" and cell_key(result) == key
            ),
            next((result for result in results if cell_key(result) == key), None),
        )
        settings = _settings(representative)
        topology = representative.get("topology") if representative else None
        controller = _field(topology, "controller_os") + " · " + _field(settings, "client")
        bandwidth = _number(_metric(group, "host_tx_mbps"), 2, " Mbps")
        resolution = _field(settings, "resolution")
        opponents = []
        for product_id, label in (("rustdesk", "RustDesk"), ("moonlight", "Moonlight")):
            opponent = groups.get((product_id, key))
            if not opponent or opponent.get("status") != "ok":
                opponents.append(f"{label} —")
            else:
                opponents.append(
                    f"{label} {_comparison_value(opponent, 'latency_p50')}/{_comparison_value(opponent, 'latency_p95')}"
                )
        lines.append(
            "| "
            + " | ".join(
                (
                    generated,
                    controller,
                    network,
                    f"{resolution} · {bandwidth}",
                    SCENE_LABELS.get(scene, scene),
                    _integer(_metric(group, "samples")),
                    _number(_metric(group, "latency_p50"), 1, " ms"),
                    _number(_metric(group, "latency_p95"), 1, " ms"),
                    _number(_metric(group, "latency_max"), 1, " ms"),
                    "；".join(opponents),
                )
            )
            + " |"
        )
    return lines


def _verdict_section(verdict: dict, baseline: dict | None) -> list[str]:
    baseline_version = verdict.get("baseline_version")
    if baseline_version is None and baseline is not None:
        baseline_version = baseline.get("rs_version")
    baseline_version = baseline_version or "无基线"
    lines = [
        f"## 回归判定（对比 {baseline_version}）",
        "",
        f"回归 {verdict.get('regressions', 0)} · 绝对失败 {verdict.get('fails', 0)} · 告警 {verdict.get('warns', 0)}",
    ]
    for item in verdict.get("items", []):
        scene = SCENE_LABELS.get(item.get("scene", ""), item.get("scene") or "—")
        network = item.get("network") or "—"
        level = LEVEL_LABELS.get(item.get("level"), item.get("level", "说明"))
        lines.append(f"- [{level}] {scene}·{network} {item.get('message', '')}")
    return lines


def render_report(summary: dict, baseline: dict | None = None, verdict: dict | None = None) -> str:
    """Render a summary in the fixed Chinese Markdown report format."""

    results = summary.get("results", [])
    if not isinstance(results, list):
        results = []
    groups = aggregate(results)
    keys = sorted({key for (_, key) in groups}, key=_cell_sort)
    date = _date(summary.get("generated_at"))
    version = summary.get("rs_version", "—")
    lines = [f"# 对照基准 {version}（{date}）", ""]
    lines.extend(_environment(summary, results))
    lines.append("")
    for key in keys:
        lines.extend(_cell_section(results, groups, key))
        lines.append("")
    lines.extend(_quality_section(summary, results, groups, keys))
    if verdict is not None:
        lines.append("")
        lines.extend(_verdict_section(verdict, baseline))
    lines.extend(
        [
            "",
            "## 说明",
            "",
            "- 延迟为「Host 页面出帧 → 控制端窗口合成」，不含控制端面板出光，不能直接对照 §01 手机指标。",
            "- 本产品客户端为 iOS 模拟器 App，含 Simulator 合成开销。",
        ]
    )
    if any(
        isinstance(result.get("network"), dict)
        and result["network"].get("jitter_applied") is False
        for result in results
    ):
        lines.append("- weak 网络在 macOS dummynet 上整形，未加抖动。")
    return "\n".join(lines) + "\n"
