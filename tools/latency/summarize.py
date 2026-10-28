#!/usr/bin/env python3
"""Summarize glass-to-glass latency samples from a two-column CSV.

Input CSV has a header and rows of ``host_ms,phone_ms``.  Latency is
``phone_ms - host_ms``; a negative result wraps across a minute boundary and
gets 60000 ms added.  Percentiles use the nearest-rank method: for a sorted
sample of size n, the p-th percentile (0 < p <= 100) is the value at the
ceil(p/100 * n)-th position.
"""

from __future__ import annotations

import argparse
import csv
import math
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Sequence, TextIO

MINUTE_MS = 60_000
HEADER = ("host_ms", "phone_ms")


@dataclass(frozen=True)
class Summary:
    count: int
    p50: float
    p95: float
    maximum: float
    mean: float


def _normalize_header(value: object) -> str:
    return str(value or "").strip().lower().replace(" ", "_")


def _parse_number(value: str, line: int, column: str) -> float:
    text = value.strip()
    if text == "":
        raise ValueError(f"line {line}: missing {column}")
    try:
        number = float(text)
    except ValueError as exc:
        raise ValueError(f"line {line}: invalid {column}: {value!r}") from exc
    if not math.isfinite(number) or number < 0:
        raise ValueError(f"line {line}: {column} must be finite and non-negative")
    return number


def _read_samples(reader: Iterable[list[str]]) -> list[float]:
    rows = iter(reader)
    first_row = next(rows, None)
    if first_row is None:
        raise ValueError("CSV is empty")

    header = tuple(_normalize_header(cell) for cell in first_row)
    if header == HEADER:
        numbered_rows = enumerate(rows, start=2)
    else:
        if len(first_row) < 2:
            raise ValueError("CSV must have two columns: host_ms,phone_ms")
        # A header is optional, so a first row without one is data.
        numbered_rows = _chain_first_row(first_row, rows, start=2)

    samples: list[float] = []
    for line, row in numbered_rows:
        if not row or all(cell.strip() == "" for cell in row):
            continue
        if len(row) < 2:
            raise ValueError(f"line {line}: expected host_ms,phone_ms")
        host_ms = _parse_number(row[0], line, "host_ms")
        phone_ms = _parse_number(row[1], line, "phone_ms")
        delta = phone_ms - host_ms
        if delta < 0:
            delta += MINUTE_MS
        samples.append(delta)

    if not samples:
        raise ValueError("CSV contains no samples")
    return samples


def _chain_first_row(
    first_row: list[str], rows: Iterable[list[str]], start: int
) -> Iterable[tuple[int, list[str]]]:
    yield 1, first_row
    yield from enumerate(rows, start=start)


def read_samples(path: Path | str) -> list[float]:
    """Read host_ms/phone_ms pairs and return unwrapped latencies in ms."""
    with Path(path).open(newline="", encoding="utf-8-sig") as handle:
        return _read_samples(csv.reader(handle))


def nearest_rank(sorted_values: Sequence[float], percentile: float) -> float:
    """Return the nearest-rank percentile (1-based rank ceil(p/100*n))."""
    if not sorted_values:
        raise ValueError("cannot compute a percentile of an empty sample")
    if not 0 < percentile <= 100:
        raise ValueError("percentile must be in (0, 100]")
    rank = max(1, math.ceil(percentile / 100 * len(sorted_values)))
    return float(sorted_values[rank - 1])


def summarize(samples: Iterable[float]) -> Summary:
    values = sorted(samples)
    if not values:
        raise ValueError("cannot summarize an empty sample")
    return Summary(
        count=len(values),
        p50=nearest_rank(values, 50),
        p95=nearest_rank(values, 95),
        maximum=values[-1],
        mean=sum(values) / len(values),
    )


def format_text(summary: Summary) -> str:
    return "\n".join(
        (
            f"Samples: {summary.count}",
            f"p50: {format_ms(summary.p50)}",
            f"p95: {format_ms(summary.p95)}",
            f"max: {format_ms(summary.maximum)}",
            f"mean: {summary.mean:.1f} ms",
        )
    )


def format_ms(value: float) -> str:
    """Format milliseconds without noisy binary-float tails."""
    text = f"{value:.3f}".rstrip("0").rstrip(".")
    return f"{text} ms"


def format_markdown(summary: Summary) -> str:
    return (
        "| "
        f"{summary.count} | {format_ms(summary.p50)} "
        f"| {format_ms(summary.p95)} | {format_ms(summary.maximum)} |"
    )


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Summarize host_ms,phone_ms glass-to-glass latency samples."
    )
    parser.add_argument(
        "csv",
        nargs="?",
        default="-",
        metavar="CSV",
        help="input CSV path, or '-' for stdin (default: -)",
    )
    parser.add_argument(
        "--md",
        action="store_true",
        help="print one Markdown table row for the m0-latency section 3 table",
    )
    return parser.parse_args(argv)


def _read_stdin(handle: TextIO) -> list[float]:
    return _read_samples(csv.reader(handle))


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        if args.csv == "-":
            samples = _read_stdin(sys.stdin)
        else:
            samples = read_samples(args.csv)
        summary = summarize(samples)
    except (OSError, ValueError) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2

    print(format_markdown(summary) if args.md else format_text(summary))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
