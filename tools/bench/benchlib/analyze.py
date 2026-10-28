from __future__ import annotations

import json
import math
from pathlib import Path

from benchlib import model, stats

TIME_MOD = 1 << 36
VALID_MIN_MS = -5.0
VALID_MAX_MS = 10000.0


class AnalyzeError(ValueError):
    """Raised when captured benchmark data cannot be analyzed."""


def read_jsonl(path: Path) -> list[dict]:
    path = Path(path)
    rows: list[dict] = []
    try:
        with path.open("r", encoding="utf-8-sig") as handle:
            for line_number, line in enumerate(handle, start=1):
                if not line.strip():
                    continue
                try:
                    value = json.loads(line)
                except (json.JSONDecodeError, UnicodeError) as exc:
                    detail = getattr(exc, "msg", str(exc))
                    raise AnalyzeError(f"{path}:{line_number}: invalid JSON: {detail}") from exc
                if not isinstance(value, dict):
                    raise AnalyzeError(
                        f"{path}:{line_number}: JSON value must be an object"
                    )
                rows.append(value)
    except (OSError, UnicodeError) as exc:
        raise AnalyzeError(f"{path}: {exc}") from exc
    return rows


def _probe(clock: dict | None, name: str) -> tuple[float, float] | None:
    if not isinstance(clock, dict):
        return None
    value = clock.get(name)
    if not isinstance(value, dict):
        return None
    t_us = value.get("t_us")
    offset_us = value.get("offset_us")
    if not isinstance(t_us, (int, float)) or isinstance(t_us, bool):
        return None
    if not isinstance(offset_us, (int, float)) or isinstance(offset_us, bool):
        return None
    try:
        t_value = float(t_us)
        offset_value = float(offset_us)
    except (OverflowError, TypeError):
        return None
    if not math.isfinite(t_value) or not math.isfinite(offset_value):
        return None
    return t_value, offset_value


def offset_at(t_us: float, clock: dict | None) -> float:
    """Interpolate Host-clock minus controller-clock offset at ``t_us``."""
    before = _probe(clock, "before")
    after = _probe(clock, "after")
    if before is None and after is None:
        return 0.0
    if before is None:
        return after[1]
    if after is None:
        return before[1]
    if before[0] == after[0]:
        return (before[1] + after[1]) / 2.0
    if t_us <= before[0]:
        return before[1]
    if t_us >= after[0]:
        return after[1]
    ratio = (t_us - before[0]) / (after[0] - before[0])
    return before[1] + ratio * (after[1] - before[1])


def unwrap_ms(code: int, approx_host_ms: float) -> int:
    """Choose the 36-bit epoch nearest an approximate Host millisecond."""
    code = int(code)
    base = math.floor((approx_host_ms - code) / TIME_MOD)
    candidates = (base, base + 1)
    best = min(candidates, key=lambda k: (abs(code + k * TIME_MOD - approx_host_ms), k))
    return code + best * TIME_MOD


def _calibration(lines: list[dict]) -> dict:
    for line in lines:
        if line.get("type") == "calib":
            rect = line.get("rect")
            host_w = line.get("host_w")
            host_h = line.get("host_h")
            if not isinstance(rect, list):
                rect = []
            if not isinstance(host_w, int) or isinstance(host_w, bool):
                host_w = int(host_w) if isinstance(host_w, (int, float)) else 0
            if not isinstance(host_h, int) or isinstance(host_h, bool):
                host_h = int(host_h) if isinstance(host_h, (int, float)) else 0
            return {"rect": rect, "host_w": host_w, "host_h": host_h}
    raise AnalyzeError("no calibration")


def analyze_frames(
    lines: list[dict], clock: dict | None, *, warmup_s: float = 2.0
) -> dict:
    calibration = _calibration(lines)
    frames = [line for line in lines if line.get("type") == "frame"]
    if any(
        frames[index].get("t_us", 0) < frames[index - 1].get("t_us", 0)
        for index in range(1, len(frames))
    ):
        frames = sorted(frames, key=lambda line: line.get("t_us", 0))

    captured = len(frames)
    decoded = sum(1 for line in frames if line.get("code") is not None)
    decode_fail_ratio = round(1.0 - decoded / captured, 4) if captured else 0.0

    first_seen: dict[int, int] = {}
    for line in frames:
        code = line.get("code")
        if code is None:
            continue
        try:
            code_int = int(code)
            t_us = int(line["t_us"])
        except (KeyError, TypeError, ValueError) as exc:
            raise AnalyzeError("frame has invalid t_us or code") from exc
        previous = first_seen.get(code_int)
        if previous is None or t_us < previous:
            first_seen[code_int] = t_us

    ordered = sorted(first_seen.items(), key=lambda item: item[1])
    if ordered:
        win_start = int(ordered[0][1] + warmup_s * 1_000_000)
        win_end = int(ordered[-1][1])
        window_items = [item for item in ordered if item[1] >= win_start]
    else:
        win_start = 0
        win_end = 0
        window_items = []

    latency_values: list[float] = []
    invalid_latency = 0
    for code, t_us in window_items:
        offset_us = offset_at(t_us, clock)
        host_ms = unwrap_ms(code, (t_us + offset_us) / 1000.0)
        latency_ms = (t_us - (host_ms * 1000.0 - offset_us)) / 1000.0
        if VALID_MIN_MS <= latency_ms <= VALID_MAX_MS:
            latency_values.append(latency_ms)
        else:
            invalid_latency += 1

    if len(window_items) < 2:
        latency = None
        display_fps = 0.0
        longest_freeze = 0.0
        freezes_over_100 = 0
        freezes_over_500 = 0
    else:
        latency = stats.summarize(latency_values)
        first_t = window_items[0][1]
        last_t = window_items[-1][1]
        duration_s = (last_t - first_t) / 1_000_000.0
        display_fps = round((len(window_items) - 1) / duration_s, 3) if duration_s > 0 else 0.0
        gaps = [
            (window_items[index][1] - window_items[index - 1][1]) / 1000.0
            for index in range(1, len(window_items))
        ]
        longest_freeze = round(max(gaps), 3) if gaps else 0.0
        freezes_over_100 = sum(1 for gap in gaps if gap > 100.0)
        freezes_over_500 = sum(1 for gap in gaps if gap > 500.0)

    end = next((line for line in reversed(lines) if line.get("type") == "end"), None)
    saved = [line["path"] for line in lines if line.get("type") == "saved" and "path" in line]
    return {
        "latency_ms": latency,
        "delivery": {
            "frames_captured": captured,
            "frames_decoded": decoded,
            "decode_fail_ratio": decode_fail_ratio,
            "distinct_codes": len(window_items),
            "display_fps": display_fps,
            "longest_freeze_ms": longest_freeze,
            "freezes_over_100ms": freezes_over_100,
            "freezes_over_500ms": freezes_over_500,
            "invalid_latency": invalid_latency,
        },
        "window": {"start_us": win_start, "end_us": win_end},
        "calib": calibration,
        "end": end,
        "saved": saved,
    }


def analyze_run(run_dir: Path, *, warmup_s: float = 2.0) -> dict:
    run_dir = Path(run_dir)
    frames = read_jsonl(run_dir / "frames.jsonl")
    clock_path = run_dir / "clock.json"
    clock = model.read_json(clock_path) if clock_path.exists() else None
    return analyze_frames(frames, clock, warmup_s=warmup_s)
