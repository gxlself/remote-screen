from __future__ import annotations

import json
import math
import os
import re
from datetime import datetime, timezone
from pathlib import Path

from benchlib.adapters import ADAPTER_IDS

SCHEMA = 1
KIND_RESULT = "rs-bench-result"
KIND_SUMMARY = "rs-bench-summary"
SCENES = ("static", "scroll", "video", "drag")
STATUSES = ("ok", "invalid", "failed")
MIN_SAMPLES = 300
MAX_DECODE_FAIL = 0.05
MAX_INVALID_RATIO = 0.01

_RUN_ID_RE = re.compile(
    r"^\d{8}T\d{6}Z-[a-z]+-[a-z]+-[a-z0-9-]+-r\d+$"
)
_RESULT_KEYS = (
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
)


class ModelError(ValueError):
    """Raised when a benchmark model file cannot be read or represented."""


def _utc_datetime(dt: datetime) -> datetime:
    if dt.tzinfo is None or dt.utcoffset() is None:
        raise ValueError("datetime must include a timezone")
    return dt.astimezone(timezone.utc)


def utc_stamp(dt: datetime) -> str:
    """Format an aware datetime as a compact UTC timestamp."""
    return _utc_datetime(dt).strftime("%Y%m%dT%H%M%SZ")


def iso_utc(dt: datetime) -> str:
    """Format an aware datetime as an ISO-8601 UTC timestamp to seconds."""
    return _utc_datetime(dt).strftime("%Y-%m-%dT%H:%M:%SZ")


def make_run_id(
    started_at: datetime, product: str, scene: str, net: str, repeat: int
) -> str:
    return f"{utc_stamp(started_at)}-{product}-{scene}-{net}-r{repeat}"


def new_result(
    *,
    run_id: str,
    started_at: datetime,
    product: dict,
    scene: str,
    network: dict,
    seconds: int,
    repeat: int,
    bench: dict,
    topology: dict,
) -> dict:
    """Construct an empty result with the I3 field order and defaults."""
    return {
        "schema": SCHEMA,
        "kind": KIND_RESULT,
        "run_id": run_id,
        "started_at": iso_utc(started_at),
        "bench": bench,
        "product": product,
        "topology": topology,
        "scene": scene,
        "network": network,
        "repeat": repeat,
        "seconds": seconds,
        "status": "failed",
        "error": "not run",
        "clock": None,
        "latency_ms": None,
        "delivery": None,
        "bandwidth": None,
        "host_usage": None,
        "quality": None,
        "artifacts": {
            "frames": "frames.jsonl",
            "clock": "clock.json",
            "host_stats": "host_stats.jsonl",
            "log": "run.log",
        },
    }


def classify(result: dict) -> tuple[str, str | None]:
    status = result.get("status")
    if status == "failed":
        return "failed", result.get("error")

    latency = result.get("latency_ms")
    if latency is None:
        return "invalid", "no latency samples"
    n = latency.get("n", 0) if isinstance(latency, dict) else 0
    if not isinstance(n, (int, float)) or n < MIN_SAMPLES:
        return "invalid", f"too few samples: {n} < {MIN_SAMPLES}"

    delivery = result.get("delivery")
    if not isinstance(delivery, dict):
        return "invalid", "missing delivery metrics"
    decode_fail = delivery.get("decode_fail_ratio", 0)
    if decode_fail > MAX_DECODE_FAIL:
        return "invalid", (
            f"decode fail ratio: {decode_fail} > {MAX_DECODE_FAIL}"
        )
    invalid_latency = delivery.get("invalid_latency", 0)
    distinct_codes = delivery.get("distinct_codes", 0)
    invalid_ratio = invalid_latency / max(1, distinct_codes)
    if invalid_ratio > MAX_INVALID_RATIO:
        return "invalid", (
            "invalid latency ratio: "
            f"{invalid_latency}/{max(1, distinct_codes)} > {MAX_INVALID_RATIO}"
        )
    return "ok", None


def _is_finite_number(value: object) -> bool:
    if not isinstance(value, (int, float)) or isinstance(value, bool):
        return False
    try:
        return math.isfinite(value)
    except (OverflowError, TypeError):
        return False


def validate_result(d: dict) -> list[str]:
    errors: list[str] = []
    if not isinstance(d, dict):
        return ["result must be an object"]

    for key in _RESULT_KEYS:
        if key not in d:
            errors.append(f"missing key: {key}")

    if "schema" in d and d.get("schema") != SCHEMA:
        errors.append(f"schema must be {SCHEMA}")
    if "kind" in d and d.get("kind") != KIND_RESULT:
        errors.append(f"kind must be {KIND_RESULT}")

    run_id = d.get("run_id")
    if not isinstance(run_id, str) or _RUN_ID_RE.fullmatch(run_id) is None:
        errors.append("run_id has invalid format")

    scene = d.get("scene")
    if scene not in SCENES:
        errors.append(f"scene must be one of {SCENES}")
    status = d.get("status")
    if status not in STATUSES:
        errors.append(f"status must be one of {STATUSES}")

    product = d.get("product")
    product_id = product.get("id") if isinstance(product, dict) else None
    if product_id not in ADAPTER_IDS:
        errors.append(f"product.id must be one of {ADAPTER_IDS}")

    network = d.get("network")
    profile = network.get("profile") if isinstance(network, dict) else None
    if not isinstance(profile, str) or not profile:
        errors.append("network.profile must be a non-empty string")

    latency = d.get("latency_ms")
    if latency is not None:
        if not isinstance(latency, dict):
            errors.append("latency_ms must be an object or null")
        else:
            latency_keys = ("n", "p50", "p95", "p99", "max", "mean")
            for key in latency_keys:
                if key not in latency:
                    errors.append(f"latency_ms missing key: {key}")
            for key in latency_keys:
                if key not in latency:
                    continue
                value = latency[key]
                if not _is_finite_number(value):
                    errors.append(f"latency_ms.{key} must be finite")
                    continue
                if key == "p50":
                    if value < VALID_MIN_P50:
                        errors.append("latency_ms.p50 must be at least -5")
                elif value < 0:
                    errors.append(f"latency_ms.{key} must be non-negative")
            if all(key in latency and _is_finite_number(latency[key]) for key in ("p50", "p95", "p99", "max")):
                if latency["p50"] > latency["p95"]:
                    errors.append("latency_ms p50 must be <= p95")
                if latency["p95"] > latency["p99"]:
                    errors.append("latency_ms p95 must be <= p99")
                if latency["p99"] > latency["max"]:
                    errors.append("latency_ms p99 must be <= max")
    return errors


VALID_MIN_P50 = -5.0


def make_summary(results: list[dict], rs_version: str, generated_at: datetime) -> dict:
    return {
        "schema": SCHEMA,
        "kind": KIND_SUMMARY,
        "rs_version": rs_version,
        "generated_at": iso_utc(generated_at),
        "results": sorted(results, key=lambda result: result.get("run_id", "")),
    }


def validate_summary(d: dict) -> list[str]:
    errors: list[str] = []
    if not isinstance(d, dict):
        return ["summary must be an object"]
    required = ("schema", "kind", "rs_version", "generated_at", "results")
    for key in required:
        if key not in d:
            errors.append(f"missing key: {key}")
    if "schema" in d and d.get("schema") != SCHEMA:
        errors.append(f"schema must be {SCHEMA}")
    if "kind" in d and d.get("kind") != KIND_SUMMARY:
        errors.append(f"kind must be {KIND_SUMMARY}")
    results = d.get("results")
    if not isinstance(results, list):
        errors.append("results must be a list")
    else:
        for index, result in enumerate(results):
            for error in validate_result(result):
                errors.append(f"results[{index}]: {error}")
    return errors


def write_json(path: Path, obj: dict) -> None:
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    try:
        with temporary.open("w", encoding="utf-8", newline="\n") as handle:
            json.dump(obj, handle, ensure_ascii=False, indent=2)
            handle.write("\n")
        os.replace(temporary, path)
    except Exception:
        try:
            temporary.unlink()
        except FileNotFoundError:
            pass
        raise


def read_json(path: Path) -> dict:
    path = Path(path)
    try:
        with path.open("r", encoding="utf-8-sig") as handle:
            value = json.load(handle)
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        raise ModelError(f"cannot read JSON {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise ModelError(f"JSON root must be an object: {path}")
    return value


def load_results(root: Path) -> tuple[list[dict], list[str]]:
    results: list[dict] = []
    errors: list[str] = []
    for path in sorted(Path(root).rglob("result.json")):
        try:
            result = read_json(path)
        except ModelError as exc:
            errors.append(f"{path}: {exc}")
            continue
        validation_errors = validate_result(result)
        if validation_errors:
            errors.extend(f"{path}: {error}" for error in validation_errors)
            continue
        results.append(result)
    return results, errors
