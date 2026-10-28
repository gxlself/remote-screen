from __future__ import annotations

import copy
import importlib
import math
import re
import shlex
import time
from datetime import datetime, timezone
from pathlib import Path, PureWindowsPath

from benchlib import analyze, model
from benchlib.adapters import ADAPTER_IDS
from benchlib.adapters.base import Adapter, BenchError, Cell, Ctx
from benchlib.executor import SshExecutor

TIMING = {
    "baseline_idle_s": 5,
    "connect_wait_s": 8,
    "settle_s": 5,
    "page_after_capture_s": 2,
    "capture_calib_timeout_s": 30,
}


def dependency(name: str):
    try:
        return importlib.import_module(f"benchlib.{name}")
    except ImportError as exc:
        raise BenchError(f"benchlib.{name} is unavailable: {exc}") from exc


def netem_profiles(repo: Path) -> list[str]:
    text = (Path(repo) / "tools/netem/profiles.toml").read_text(encoding="utf-8")
    return re.findall(r"^\s*\[profiles\.([A-Za-z0-9_-]+)\]\s*(?:#.*)?$", text, re.MULTILINE)


def _integer(value, name: str, minimum: int, maximum: int | None = None) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
        raise BenchError(f"{name} must be an integer >= {minimum}")
    if maximum is not None and value > maximum:
        raise BenchError(f"{name} must be <= {maximum}")
    return value


def _selection(value, name: str, allowed) -> list[str]:
    if not isinstance(value, (list, tuple)) or not value:
        raise BenchError(f"{name} must be a non-empty list")
    if any(not isinstance(item, str) or item not in allowed for item in value):
        raise BenchError(f"invalid {name}: {value!r}")
    if len(set(value)) != len(value):
        raise BenchError(f"duplicate {name}")
    return list(value)


def validate_matrix(matrix: dict, profiles: list[str]) -> None:
    if not isinstance(matrix, dict) or matrix.get("schema") != 1:
        raise BenchError("matrix schema must be 1")
    _selection(matrix.get("products"), "products", ADAPTER_IDS)
    _selection(matrix.get("scenes"), "scenes", model.SCENES)
    allowed = {"lan", *profiles} - {"handover", "bw-step"}
    _selection(matrix.get("networks"), "networks", allowed)
    _integer(matrix.get("repeats"), "repeats", 1)
    _integer(matrix.get("seconds"), "seconds", 1, 3600)
    _integer(matrix.get("calib_s"), "calib_s", 1, 30)
    _integer(matrix.get("warmup_s", 2), "warmup_s", 0)
    timing = matrix.get("timing", {})
    if not isinstance(timing, dict):
        raise BenchError("timing must be an object")
    for key in TIMING:
        if key in timing:
            _integer(timing[key], f"timing.{key}", 1 if key == "capture_calib_timeout_s" else 0)


def validate_lab(lab: dict) -> None:
    if not isinstance(lab, dict) or lab.get("schema") != 1:
        raise BenchError("lab schema must be 1")
    for section in ("host", "controller", "products"):
        if not isinstance(lab.get(section), dict):
            raise BenchError(f"lab.{section} must be an object")
    host, ctl = lab["host"], lab["controller"]
    if host.get("os") != "windows" or ctl.get("os") != "macos":
        raise BenchError("lab requires a Windows host and macOS controller")
    alias = host.get("ssh")
    if not isinstance(alias, str) or re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]*", alias) is None:
        raise BenchError("lab.host.ssh must be an SSH alias")
    for section in ("host", "controller"):
        if not isinstance(lab[section].get("addr"), str) or not lab[section]["addr"].strip():
            raise BenchError(f"lab.{section}.addr must be a non-empty string")
    bench_dir = host.get("bench_dir")
    if not isinstance(bench_dir, str) or not PureWindowsPath(bench_dir).is_absolute():
        raise BenchError("lab.host.bench_dir must be an absolute Windows path")
    shell = host.get("shell", "cmd")
    if shell not in ("cmd", "powershell"):
        raise BenchError("lab.host.shell must be cmd or powershell")
    SshExecutor(alias, shell).remote_command([bench_dir])
    if ctl.get("netem") not in ("mac", "none"):
        raise BenchError("lab.controller.netem must be mac or none")
    if not isinstance(ctl.get("rs_bench"), str) or not ctl["rs_bench"]:
        raise BenchError("lab.controller.rs_bench must be a non-empty string")
    if any(not isinstance(value, dict) for value in lab["products"].values()):
        raise BenchError("lab.products entries must be objects")


def plan_cells(
    matrix: dict,
    *,
    products=None,
    scenes=None,
    networks=None,
    repeats=None,
    seconds=None,
    started_at: datetime,
    out_dir: Path,
    bench_dir: str,
    profiles: list[str],
) -> list[Cell]:
    validate_matrix(matrix, profiles)
    selected_products = _selection(matrix["products"] if products is None else products, "products", ADAPTER_IDS)
    selected_scenes = _selection(matrix["scenes"] if scenes is None else scenes, "scenes", model.SCENES)
    allowed_networks = {"lan", *profiles} - {"handover", "bw-step"}
    selected_networks = _selection(matrix["networks"] if networks is None else networks, "networks", allowed_networks)
    count = _integer(matrix["repeats"] if repeats is None else repeats, "repeats", 1)
    duration = _integer(matrix["seconds"] if seconds is None else seconds, "seconds", 1, 3600)
    host_runs = bench_dir.rstrip("\\/") + "\\runs\\"
    cells: list[Cell] = []
    for product in selected_products:
        for network in selected_networks:
            if product == "local" and network != "lan":
                continue
            for scene in selected_scenes:
                for repeat in range(1, count + 1):
                    run_id = model.make_run_id(started_at, product, scene, network, repeat)
                    cells.append(Cell(
                        product=product, scene=scene, network=network, repeat=repeat,
                        seconds=duration, calib_s=matrix["calib_s"], run_id=run_id,
                        run_dir=Path(out_dir) / run_id,
                        host_run_dir=None if product == "local" else host_runs + run_id,
                    ))
    return cells


def _result(cell: Cell, base: dict) -> dict:
    started = datetime.strptime(cell.run_id.split("-", 1)[0], "%Y%m%dT%H%M%SZ").replace(tzinfo=timezone.utc)
    result = model.new_result(
        run_id=cell.run_id, started_at=started,
        product=copy.deepcopy(base["product"]), bench=copy.deepcopy(base["bench"]),
        topology=copy.deepcopy(base["topology"]), scene=cell.scene,
        network={
            "profile": cell.network,
            "shaping": "none" if cell.network == "lan" else "mac-dummynet",
            "jitter_applied": False,
        },
        repeat=cell.repeat, seconds=cell.seconds,
    )
    return result


def _clock_command(ctx: Ctx) -> str:
    host = ctx.lab["host"]
    exe = host["bench_dir"].rstrip("\\/") + "\\bin\\rs-bench.exe"
    SshExecutor(host["ssh"], host.get("shell", "cmd")).remote_command([exe])
    remote = ('& ' if host.get("shell", "cmd") == "powershell" else '') + f'"{exe}" clock serve --stdio'
    return " ".join(shlex.quote(value) for value in [
        getattr(ctx.host, "ssh", "ssh"), "-T", "-o", "BatchMode=yes", host["ssh"], remote,
    ])


def _clock_probe(ctx: Ctx, rs_bench: Path, path: Path) -> dict:
    try:
        reply = ctx.ctl.run(
            [str(rs_bench), "clock", "probe", "--cmd", _clock_command(ctx), "--json", str(path)],
            check=False,
        )
    except BenchError as exc:
        raise BenchError(f"clock probe failed: {exc}") from exc
    if reply.code:
        raise BenchError(f"clock probe failed: {reply.err[-500:] or reply.out[-500:] or reply.code}")
    try:
        probe = model.read_json(path)
        for key in ("t_us", "offset_us", "rtt_us"):
            value = probe.get(key)
            if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
                raise ValueError(f"invalid {key}")
        return probe
    except (model.ModelError, ValueError) as exc:
        raise BenchError(f"clock probe failed: {exc}") from exc


def _clock_block(clock: dict) -> dict | None:
    probes = [clock[key] for key in ("before", "after") if isinstance(clock.get(key), dict)]
    if not probes:
        return None
    return {
        "offset_before_us": clock["before"]["offset_us"] if clock.get("before") else None,
        "offset_after_us": clock["after"]["offset_us"] if clock.get("after") else None,
        "rtt_min_us": min(probe["rtt_us"] for probe in probes),
        "method": "stdio",
    }


def _cleanup(ctx: Ctx, name: str, action) -> None:
    try:
        action()
    except (Exception, KeyboardInterrupt) as exc:
        ctx.log(f"cleanup {name}: {exc}")


def _ssim(ctx: Ctx, rs_bench: Path, cell: Cell, analysis: dict) -> dict | None:
    if cell.scene != "static" or not analysis.get("saved"):
        return None
    try:
        calib = analysis["calib"]
        command = [str(rs_bench), "ssim", "--scene", "static", "--host-size", f"{calib['host_w']}x{calib['host_h']}"]
        for saved in analysis["saved"]:
            path = Path(saved)
            if path.is_absolute() or ".." in path.parts:
                raise BenchError(f"invalid saved frame path: {saved}")
            command += ["--frame", str(cell.run_dir / path)]
        command += ["--json", str(cell.run_dir / "ssim.json")]
        reply = ctx.ctl.run(command, check=False)
        if reply.code:
            raise BenchError(f"ssim exit {reply.code}: {reply.err[-500:]}")
        ssim = model.read_json(cell.run_dir / "ssim.json")
        return {"ssim": ssim["mean"], "frames": ssim.get("n", len(analysis["saved"]))}
    except (BenchError, OSError, ValueError, KeyError, TypeError) as exc:
        ctx.log(f"SSIM unavailable: {exc}")
        return None


def run_cell(
    ctx: Ctx, adapter: Adapter, cell: Cell, matrix: dict, *, rs_bench: Path,
    base: dict, now=time.time,
) -> dict:
    cell.run_dir.mkdir(parents=True, exist_ok=True)
    previous_log = ctx.log_path
    ctx.log_path = cell.run_dir / "run.log"
    result = _result(cell, base)
    timing = {**TIMING, **matrix.get("timing", {})}
    total = (
        sum(timing[key] for key in ("connect_wait_s", "settle_s", "page_after_capture_s"))
        + cell.calib_s + cell.seconds + 30
    )
    gui = str(ctx.repo / "tools/bench/mac/gui-run.sh")
    netem = ["sudo", "-n", str(ctx.repo / "tools/netem/mac.sh")]
    host_stats = cell.host_run_dir + "\\host_stats.jsonl" if cell.host_run_dir else None
    clock = {"schema": 1, "before": None, "after": None}
    shaped = False
    label = None
    capture_running = False
    captured = False
    baseline = None

    def start_capture() -> str:
        win = adapter.window(ctx)
        command = [str(rs_bench), "capture", "--app", win["app"]]
        if win.get("title"):
            command += ["--title", win["title"]]
        command += ["--out", str(cell.run_dir / "frames.jsonl"), "--seconds", str(cell.seconds),
                    "--calib-timeout", str(timing["capture_calib_timeout_s"]), "--wait-window", "10"]
        if cell.scene == "static":
            command.append("--save-frames")
        reply = ctx.gui(command, detach=True, timeout=timing["capture_calib_timeout_s"] + cell.seconds + 60)
        if reply.code:
            raise BenchError(f"capture exit {reply.code}: {reply.err[-500:]}")
        capture_label = reply.out.strip()
        if re.fullmatch(r"[A-Za-z0-9_.-]+", capture_label) is None or capture_label in (".", ".."):
            raise BenchError("capture: missing or invalid GUI label")
        return capture_label

    try:
        ctx.log(f"starting {cell.run_id}")
        try:
            if adapter.needs_host:
                if cell.host_run_dir is None:
                    raise BenchError("remote cell has no host_run_dir")
                ctx.host_ps("mkdir", "-Path", cell.host_run_dir)
                ctx.host_ps("sample-start", "-Names", ",".join(adapter.host_processes),
                            "-Seconds", str(total + 60), "-RemoteAddress", ctx.lab["controller"]["addr"],
                            "-Out", host_stats)
                clock["before"] = _clock_probe(ctx, rs_bench, cell.run_dir / "clock_before.json")
                b0 = now()
                ctx.sleep(timing["baseline_idle_s"])
                baseline = (b0, now())
                adapter.client_start(ctx, cell, seconds=total + 30)
                ctx.sleep(timing["connect_wait_s"])
                if cell.network != "lan":
                    shaped = True
                    if ctx.lab["controller"].get("netem") == "none":
                        raise BenchError("netem unavailable: controller.netem is none")
                    try:
                        reply = ctx.ctl.run(
                            netem + ["--ports", ",".join(str(port) for port in adapter.ports), cell.network],
                            check=False,
                        )
                    except BenchError as exc:
                        raise BenchError(f"netem unavailable: {exc}") from exc
                    if reply.code:
                        raise BenchError(f"netem unavailable: {reply.err[-500:] or reply.out[-500:] or reply.code}")
                    ctx.sleep(timing["settle_s"])
                label = start_capture()
                capture_running = True
                ctx.sleep(timing["page_after_capture_s"])
                ctx.host_ps("page-start", "-Scene", cell.scene, "-Seconds", str(cell.seconds),
                            "-Calib", str(cell.calib_s), "-Delay", "0")
            else:
                label = start_capture()
                capture_running = True
                adapter.client_start(ctx, cell, seconds=total + 30)
            timeout = cell.calib_s + cell.seconds + 60
            reply = ctx.ctl.run([gui, "--wait", label, "--timeout", str(timeout)], timeout=timeout + 15, check=False)
            capture_running = reply.code != 0
            errors = {3: "calib-timeout", 4: "source-closed", 5: "no-window", 124: "timeout"}
            if reply.code in errors:
                raise BenchError("capture: " + errors[reply.code])
            if reply.code:
                raise BenchError(f"capture exit {reply.code}")
            captured = True
            result["status"], result["error"] = "ok", None
        except BenchError as exc:
            result["status"], result["error"] = "failed", str(exc)
            ctx.log(str(exc))
        finally:
            if shaped:
                def off():
                    ctx.ctl.run(netem + ["off"])
                _cleanup(ctx, "netem off", off)
            if capture_running and label is not None:
                _cleanup(ctx, "capture stop", lambda: ctx.ctl.run([gui, "--stop", label]))
            if adapter.needs_host:
                _cleanup(ctx, "page-stop", lambda: ctx.host_ps("page-stop"))
            _cleanup(ctx, "client_stop", lambda: adapter.client_stop(ctx, cell))
            if adapter.needs_host:
                _cleanup(ctx, "sample-stop", lambda: ctx.host_ps("sample-stop", "-Out", host_stats))
                def copy_stats():
                    (cell.run_dir / "host_stats.jsonl").write_text(ctx.host_cat(host_stats), encoding="utf-8")
                _cleanup(ctx, "host stats", copy_stats)
                def after_probe():
                    clock["after"] = _clock_probe(ctx, rs_bench, cell.run_dir / "clock_after.json")
                _cleanup(ctx, "clock after", after_probe)
                _cleanup(ctx, "clock.json", lambda: model.write_json(cell.run_dir / "clock.json", clock))
                result["clock"] = _clock_block(clock)
        if captured:
            try:
                analysis = analyze.analyze_run(cell.run_dir, warmup_s=matrix.get("warmup_s", 2))
                result["latency_ms"] = analysis["latency_ms"]
                result["delivery"] = analysis["delivery"]
                if adapter.needs_host:
                    hoststats = dependency("hoststats")
                    bounds = (analysis["window"]["start_us"], analysis["window"]["end_us"])
                    host_window = tuple(int(t + analyze.offset_at(t, clock)) for t in bounds)
                    host_baseline = tuple(
                        int(t * 1_000_000 + analyze.offset_at(t * 1_000_000, clock))
                        for t in baseline
                    )
                    usage, bandwidth = hoststats.summarize(
                        hoststats.read_host_stats(cell.run_dir / "host_stats.jsonl"), host_window, host_baseline,
                    )
                    result["host_usage"], result["bandwidth"] = usage, bandwidth
                result["quality"] = _ssim(ctx, rs_bench, cell, analysis)
            except (BenchError, OSError, ValueError) as exc:
                result["status"], result["error"] = "failed", str(exc)
                ctx.log(f"analysis failed: {exc}")
        result["status"], result["error"] = model.classify(result)
        ctx.log(f"finished {cell.run_id}: {result['status']}")
        model.write_json(cell.run_dir / "result.json", result)
        return result
    finally:
        ctx.log_path = previous_log


def _failed_cell(ctx: Ctx, cell: Cell, base: dict, error: str) -> dict:
    result = _result(cell, base)
    result["error"] = error
    cell.run_dir.mkdir(parents=True, exist_ok=True)
    old_log = ctx.log_path
    try:
        ctx.log_path = cell.run_dir / "run.log"
        ctx.log(error)
    finally:
        ctx.log_path = old_log
    model.write_json(cell.run_dir / "result.json", result)
    return result


def run_matrix(
    ctx, adapters: dict, cells: list[Cell], matrix: dict, *, rs_bench, base_for,
    out_dir: Path, rs_version: str, baseline: dict | None = None,
) -> dict:
    netem = ["sudo", "-n", str(ctx.repo / "tools/netem/mac.sh")]
    _cleanup(ctx, "initial netem off", lambda: ctx.ctl.run(netem + ["off"]))
    remote_error = None
    topology = {}
    if any(adapters[cell.product].needs_host for cell in cells):
        try:
            session = ctx.host_ps("session")
            if not session.get("logged_in") or session.get("locked"):
                remote_error = "host console session not available"
            info = ctx.host_ps("hostinfo")
            topology = {
                "host_os": ctx.lab["host"]["os"], "host_os_version": info.get("os_version"),
                "host_cpu": info.get("cpu"), "host_gpu": ", ".join(info.get("gpus", [])),
                "host_display": info.get("display"),
            }
            if not str(info.get("display", "")).startswith("1920x1080@"):
                ctx.log(f"warning: Host display is {info.get('display')}, expected 1920x1080")
        except BenchError as exc:
            remote_error = remote_error or str(exc)
    statuses = {}
    for key, adapter in adapters.items():
        try:
            statuses[key] = adapter.status(ctx)
        except BenchError as exc:
            ctx.log(f"{key} status: {exc}")
            statuses[key] = {"host_installed": False, "client_installed": False}
    results = []
    products = list(dict.fromkeys(cell.product for cell in cells))
    for product in products:
        adapter = adapters[product]
        block = [cell for cell in cells if cell.product == product]
        base = copy.deepcopy(base_for(adapter))
        if adapter.needs_host:
            base["topology"].update(topology)
        status = statuses[product]
        error = remote_error if adapter.needs_host else None
        if error is None and (not status.get("host_installed") or not status.get("client_installed")):
            error = f"not installed: run bench.py install {product} --yes"
        if error:
            results.extend(_failed_cell(ctx, cell, base, error) for cell in block)
            continue
        try:
            if adapter.needs_host:
                for key, other in adapters.items():
                    if key != product and other.needs_host and statuses[key].get("host_installed"):
                        other.host_stop(ctx)
                adapter.host_start(ctx)
            for cell in block:
                results.append(run_cell(ctx, adapter, cell, matrix, rs_bench=rs_bench, base=base))
        except BenchError as exc:
            done = {result["run_id"] for result in results}
            results.extend(_failed_cell(ctx, cell, base, str(exc)) for cell in block if cell.run_id not in done)
        finally:
            if adapter.needs_host:
                _cleanup(ctx, f"{product} host_stop", lambda: adapter.host_stop(ctx))
    summary = model.make_summary(results, rs_version, datetime.now(timezone.utc))
    model.write_json(Path(out_dir) / "summary.json", summary)
    verdict = None
    if baseline is not None:
        judge = dependency("judge")
        thresholds = model.read_json(ctx.repo / "tools/bench/thresholds.json")
        verdict = judge.judge(summary, baseline, thresholds)
    report = dependency("report")
    (Path(out_dir) / "report.md").write_text(report.render_report(summary, baseline, verdict), encoding="utf-8")
    return summary


def inventory_mac(home: Path) -> dict:
    library = Path(home) / "Library"
    result = {}
    for name in ("Preferences", "Application Support", "Caches", "LaunchAgents"):
        directory = library / name
        result[name] = sorted(entry.name for entry in directory.iterdir()) if directory.is_dir() else []
    return result


def inventory_diff(before: dict, after: dict) -> dict:
    result = {}
    for key in sorted(set(before) | set(after)):
        previous, current = before.get(key, []), after.get(key, [])
        if isinstance(previous, dict) or isinstance(current, dict):
            result[key] = inventory_diff(
                previous if isinstance(previous, dict) else {},
                current if isinstance(current, dict) else {},
            )
        else:
            result[key] = sorted(set(current) - set(previous))
    return result
