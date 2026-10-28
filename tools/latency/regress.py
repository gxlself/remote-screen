#!/usr/bin/env python3
"""Compare the latest capture-to-receive p95 with recent nightly samples."""

from __future__ import annotations

import argparse
import json
import math
import re
import statistics
import sys
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Sequence

FILENAME_PATTERN = re.compile(r"latency-(\d{8}T\d{6}Z)\.json")
MIN_HISTORY = 3


@dataclass(frozen=True)
class Sample:
    path: Path
    timestamp: datetime
    shaping: str
    p95_ms: float


def _parse_timestamp(value: object) -> datetime | None:
    if not isinstance(value, str):
        return None
    text = value.strip()
    if text.endswith("Z"):
        text = text[:-1] + "+00:00"
    try:
        parsed = datetime.fromisoformat(text)
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed.astimezone(timezone.utc)


def _timestamp_us(report: dict[str, object]) -> datetime | None:
    value = report.get("timestamp_us")
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        return None
    try:
        return datetime.fromtimestamp(value / 1_000_000, tz=timezone.utc)
    except (OverflowError, OSError, ValueError):
        return None


def _filename_timestamp(path: Path) -> datetime | None:
    match = FILENAME_PATTERN.fullmatch(path.name)
    if not match:
        return None
    return datetime.strptime(match.group(1), "%Y%m%dT%H%M%SZ").replace(
        tzinfo=timezone.utc
    )


def _mtime_timestamp(path: Path) -> datetime | None:
    try:
        return datetime.fromtimestamp(path.stat().st_mtime, tz=timezone.utc)
    except OSError:
        return None


def read_sample(path: Path, fallback_to_mtime: bool = False) -> Sample | None:
    """Read one report, ignoring malformed or incomplete files."""
    try:
        report = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError):
        return None
    if not isinstance(report, dict):
        return None
    latency = report.get("capture_to_receive_ms")
    if not isinstance(latency, dict):
        return None
    p95 = latency.get("p95")
    shaping = report.get("shaping")
    if (
        not isinstance(p95, (int, float))
        or isinstance(p95, bool)
        or not math.isfinite(p95)
        or p95 < 0
        or not isinstance(shaping, str)
        or not shaping
    ):
        return None
    timestamp = _timestamp_us(report) or _parse_timestamp(report.get("timestamp"))
    if timestamp is None:
        timestamp = _filename_timestamp(path)
    if timestamp is None and fallback_to_mtime:
        timestamp = _mtime_timestamp(path)
    if timestamp is None:
        return None
    return Sample(path=path, timestamp=timestamp, shaping=shaping, p95_ms=float(p95))


def load_current(path: Path) -> Sample:
    sample = read_sample(path, fallback_to_mtime=True)
    if sample is None:
        raise ValueError(
            "current report must contain finite capture_to_receive_ms.p95, "
            "non-empty shaping, and a timestamp (content, filename, or mtime)"
        )
    return sample


def recent_history(
    history_dir: Path,
    current: Sample,
    days: int,
    exclude: Path | None = None,
) -> list[Sample]:
    cutoff = current.timestamp - timedelta(days=days)
    anchor = current.timestamp
    excluded = exclude.resolve() if exclude is not None else None
    samples: list[Sample] = []
    for path in sorted(history_dir.glob("*.json")):
        if excluded is not None and path.resolve() == excluded:
            continue
        sample = read_sample(path)
        if sample is None or sample.shaping != current.shaping:
            continue
        if cutoff <= sample.timestamp <= anchor:
            samples.append(sample)
    return samples


def _filename_for(timestamp: datetime) -> str:
    stamp = timestamp.astimezone(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    return f"latency-{stamp}.json"


def copy_current(current_path: Path, history_dir: Path, timestamp: datetime) -> Path:
    history_dir.mkdir(parents=True, exist_ok=True)
    base = _filename_for(timestamp)
    destination = history_dir / base
    suffix = 1
    while destination.exists():
        destination = history_dir / f"{base[:-5]}-{suffix}.json"
        suffix += 1
    temporary = destination.with_suffix(".json.tmp")
    temporary.write_bytes(current_path.read_bytes())
    temporary.replace(destination)
    return destination


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Compare capture-to-receive p95 against recent nightly history."
    )
    parser.add_argument("--history", required=True, type=Path, metavar="DIR")
    parser.add_argument("--current", required=True, type=Path, metavar="JSON")
    parser.add_argument("--days", type=int, default=7, metavar="N")
    parser.add_argument("--threshold", type=float, default=0.20, metavar="RATIO")
    args = parser.parse_args(argv)
    if args.days <= 0:
        parser.error("--days must be positive")
    if not math.isfinite(args.threshold) or args.threshold < 0:
        parser.error("--threshold must be a finite non-negative ratio")
    return args


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        current = load_current(args.current)
    except (OSError, ValueError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    history = recent_history(args.history, current, args.days, exclude=args.current)
    sample_count = len(history) + 1
    args.history.mkdir(parents=True, exist_ok=True)
    copy_current(args.current, args.history, current.timestamp)
    if sample_count < MIN_HISTORY:
        print(
            f"insufficient history: {sample_count} matching sample(s), "
            f"need {MIN_HISTORY}"
        )
        return 0

    median = statistics.median(sample.p95_ms for sample in history)
    limit = median * (1.0 + args.threshold)
    print(
        f"p95 {current.p95_ms:.3f} ms, median {median:.3f} ms, "
        f"limit {limit:.3f} ms (n={len(history)}, shaping={current.shaping})"
    )
    if current.p95_ms > limit:
        print(
            f"LATENCY REGRESSION: p95 {current.p95_ms:.3f} ms exceeds "
            f"{limit:.3f} ms ({args.threshold:.0%} over median {median:.3f} ms) "
            f"for shaping {current.shaping}"
        )
        return 1
    print("latency ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
