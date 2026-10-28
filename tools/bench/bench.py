from __future__ import annotations

import argparse
import json
import re
import sys
from datetime import datetime, timezone
from pathlib import Path

BENCH_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(BENCH_DIR))

from benchlib import model, runner
from benchlib.adapters import ADAPTER_IDS, load
from benchlib.adapters.base import BenchError, Ctx
from benchlib.executor import LocalExecutor, SshExecutor

REPO = BENCH_DIR.parent.parent
DEFAULT_LAB = "~/.config/rs-bench/lab.json"
DEFAULT_STATE = "~/.local/state/rs-bench"


def parser() -> argparse.ArgumentParser:
    root = argparse.ArgumentParser(description="Remote Screen comparison benchmarks")
    commands = root.add_subparsers(dest="command", required=True)

    def common(command):
        command.add_argument("--lab", type=Path)
        command.add_argument("--state-dir", type=Path, default=Path(DEFAULT_STATE).expanduser())

    run = commands.add_parser("run")
    common(run)
    run.add_argument("--matrix", type=Path, default=BENCH_DIR / "matrix.json")
    run.add_argument("--products")
    run.add_argument("--scenes")
    run.add_argument("--networks")
    run.add_argument("--repeats", type=int)
    run.add_argument("--seconds", type=int)
    run.add_argument("--out-root", type=Path, default=Path("target/bench"))
    run.add_argument("--baseline", type=Path)
    run.add_argument("--dry-run", action="store_true")
    selftest = commands.add_parser("selftest")
    common(selftest)
    selftest.add_argument("--seconds", type=int, default=10)
    selftest.add_argument("--out-root", type=Path, default=Path("target/bench"))
    status = commands.add_parser("status")
    common(status)
    status.add_argument("--json", action="store_true")
    deploy = commands.add_parser("deploy")
    common(deploy)
    deploy.add_argument("--exe", type=Path)
    for name in ("install", "uninstall"):
        command = commands.add_parser(name)
        common(command)
        command.add_argument("id", choices=ADAPTER_IDS)
        command.add_argument("--yes", action="store_true")
    report = commands.add_parser("report")
    report.add_argument("summary", type=Path)
    report.add_argument("--baseline", type=Path)
    report.add_argument("--out", type=Path)
    judge = commands.add_parser("judge")
    judge.add_argument("summary", type=Path)
    judge.add_argument("--baseline", type=Path)
    judge.add_argument("--thresholds", type=Path, default=BENCH_DIR / "thresholds.json")
    judge.add_argument("--json", type=Path)
    return root


def rs_version(repo: Path = REPO) -> str:
    cargo = (repo / "Cargo.toml").read_text(encoding="utf-8")
    workspace = re.search(r"(?ms)^\[workspace\.package\]\s*\n(.*?)(?=^\[|\Z)", cargo)
    version = re.search(r'^version\s*=\s*"([^"]+)"', workspace.group(1), re.MULTILINE) if workspace else None
    if version is None:
        raise BenchError("cannot find workspace.package.version in Cargo.toml")
    return version.group(1)


def _read_summary(path: Path) -> dict:
    value = model.read_json(path.expanduser())
    errors = model.validate_summary(value)
    if errors:
        raise BenchError(f"invalid summary {path}: " + "; ".join(errors))
    return value


def _lab(args, *, local_only: bool = False) -> dict:
    path = args.lab.expanduser() if args.lab is not None else Path(DEFAULT_LAB).expanduser()
    if local_only and args.lab is None and not path.exists():
        path = BENCH_DIR / "lab.example.json"
    lab = model.read_json(path)
    runner.validate_lab(lab)
    return lab


def _ctx(args, lab: dict, *, allow_install: bool = False) -> Ctx:
    return Ctx(
        lab=lab, host=SshExecutor(lab["host"]["ssh"], lab["host"].get("shell", "cmd")),
        ctl=LocalExecutor(), repo=REPO, state_dir=args.state_dir.expanduser(), allow_install=allow_install,
    )


def _adapter(product: str):
    try:
        return load(product)
    except (ImportError, KeyError) as exc:
        raise BenchError(f"adapter {product} is unavailable: {exc}") from exc


def _rs_bench(lab: dict) -> Path:
    path = Path(lab["controller"]["rs_bench"]).expanduser()
    return path if path.is_absolute() else REPO / path


def _base_for(ctx: Ctx, version: str):
    def text(command: list[str]) -> str | None:
        try:
            result = ctx.ctl.run(command, check=False)
            return result.out.strip() if result.code == 0 else None
        except BenchError as exc:
            ctx.log(str(exc))
            return None

    topology = {
        "controller_os": "macos", "controller_os_version": text(["sw_vers", "-productVersion"]),
        "controller_model": text(["sysctl", "-n", "hw.model"]), "link": ctx.lab.get("link"),
    }
    bench = {"git_rev": text(["git", "-C", str(REPO), "rev-parse", "--short=12", "HEAD"]), "rs_version": version}

    def base(adapter):
        try:
            status = adapter.status(ctx)
        except BenchError:
            status = {}
        return {
            "bench": bench,
            "topology": {**topology, "host_os": ctx.lab["host"]["os"] if adapter.needs_host else "macos"},
            "product": {"id": adapter.id, "label": adapter.label,
                        "client_version": status.get("client_version"), "host_version": status.get("host_version"),
                        "settings": adapter.settings()},
        }
    return base


def _csv(value: str | None) -> list[str] | None:
    if value is None:
        return None
    parts = [part.strip() for part in value.split(",")]
    if not all(parts):
        raise BenchError("filters must be non-empty comma-separated values")
    return parts


def run(args) -> int:
    lab = _lab(args)
    matrix = model.read_json(args.matrix.expanduser())
    started = datetime.now(timezone.utc)
    version = rs_version()
    out = args.out_root.expanduser() / version / model.utc_stamp(started)
    cells = runner.plan_cells(
        matrix, products=_csv(args.products), scenes=_csv(args.scenes), networks=_csv(args.networks),
        repeats=args.repeats, seconds=args.seconds, started_at=started, out_dir=out,
        bench_dir=lab["host"]["bench_dir"], profiles=runner.netem_profiles(REPO),
    )
    baseline = _read_summary(args.baseline) if args.baseline else None
    if args.dry_run:
        for cell in cells:
            print(cell.run_id)
        return 0
    ctx = _ctx(args, lab)
    selected = list(dict.fromkeys(cell.product for cell in cells))
    adapters = {product: _adapter(product) for product in selected}
    if any(adapter.needs_host for adapter in adapters.values()):
        for product in ADAPTER_IDS:
            if product not in adapters:
                try:
                    adapters[product] = _adapter(product)
                except BenchError as exc:
                    ctx.log(str(exc))
    summary = runner.run_matrix(
        ctx, adapters, cells, matrix, rs_bench=_rs_bench(lab), base_for=_base_for(ctx, version),
        out_dir=out, rs_version=version, baseline=baseline,
    )
    print(out / "summary.json")
    return 1 if summary["results"] and all(result["status"] == "failed" for result in summary["results"]) else 0


def selftest(args) -> int:
    lab = _lab(args, local_only=True)
    matrix = model.read_json(BENCH_DIR / "matrix.json")
    started = datetime.now(timezone.utc)
    out = args.out_root.expanduser() / "selftest" / model.utc_stamp(started)
    cells = runner.plan_cells(
        matrix, products=["local"], scenes=["static"], networks=["lan"], repeats=1,
        seconds=args.seconds, started_at=started, out_dir=out,
        bench_dir=lab["host"]["bench_dir"], profiles=runner.netem_profiles(REPO),
    )
    ctx = _ctx(args, lab)
    adapter = _adapter("local")
    version = rs_version()
    result = runner.run_cell(
        ctx, adapter, cells[0], matrix, rs_bench=_rs_bench(lab), base=_base_for(ctx, version)(adapter),
    )
    model.write_json(out / "summary.json", model.make_summary([result], version, datetime.now(timezone.utc)))
    latency, delivery = result.get("latency_ms") or {}, result.get("delivery") or {}
    print(f"p50={latency.get('p50')} ms p95={latency.get('p95')} ms fps={delivery.get('display_fps')}")
    return 0 if result["status"] == "ok" else 1


def status(ctx: Ctx, *, as_json: bool = False) -> int:
    checks = {}

    def check(name, action, valid=lambda value: True):
        try:
            value = action()
            checks[name] = {"ok": bool(valid(value)), "detail": value}
        except (BenchError, OSError, ValueError) as exc:
            checks[name] = {"ok": False, "error": str(exc)}

    def capture():
        reply = ctx.gui([str(_rs_bench(ctx.lab)), "capture", "--check"])
        if reply.code:
            raise BenchError(reply.err[-500:] or f"capture --check exit {reply.code}")
        return reply.out.strip()

    def netem():
        reply = ctx.ctl.run(["sudo", "-n", str(ctx.repo / "tools/netem/mac.sh"), "show"], check=False)
        if reply.code:
            raise BenchError(reply.err[-500:] or f"mac.sh show exit {reply.code}")
        return reply.out.strip()

    check("controller.capture", capture)
    check("controller.netem", netem)
    check("host.session", lambda: ctx.host_ps("session"),
          lambda value: value.get("logged_in") and not value.get("locked"))
    check("host.env", lambda: ctx.host_ps("env"),
          lambda value: value.get("rs_bench_exe") and value.get("page") and value.get("edge"))
    for product in ADAPTER_IDS:
        check(product, lambda product=product: _adapter(product).status(ctx),
              lambda value: value.get("host_installed") and value.get("client_installed"))
    if as_json:
        print(json.dumps(checks, indent=2, ensure_ascii=False))
    else:
        for name, value in checks.items():
            detail = value.get("error", json.dumps(value.get("detail"), ensure_ascii=False))
            print(f"{name:24} {'ok' if value['ok'] else 'failed':6} {detail}")
    return 0 if all(value["ok"] for value in checks.values()) else 1


def deploy(ctx: Ctx, exe: Path | None = None) -> int:
    if exe is not None and not exe.expanduser().is_file():
        raise BenchError(f"exe does not exist: {exe}")
    bench_dir = ctx.lab["host"]["bench_dir"].rstrip("\\/")
    directories = [bench_dir + "\\" + suffix for suffix in ("bin", "page", "host\\products", "downloads", "runs")]
    quoted = ",".join("'" + path.replace("'", "''") + "'" for path in directories)
    ctx.host.run_ps(f"New-Item -ItemType Directory -Force -Path @({quoted}) | Out-Null")
    source = ctx.repo / "tools/bench"
    for pattern in ("page/*.html", "page/*.js", "host/*.ps1", "host/products/*.ps1"):
        for path in sorted(source.glob(pattern)):
            ctx.host_put(path, str(path.relative_to(source)))
    if exe is not None:
        ctx.host_put(exe.expanduser(), "bin\\rs-bench.exe")
    return 0


def _inventory(ctx: Ctx, adapter) -> dict:
    host = ctx.host_ps("inventory") if adapter.needs_host else {}
    return {
        "host": {key: value for key, value in host.items() if key != "ok"},
        "mac": runner.inventory_mac(Path.home()),
    }


def install(ctx: Ctx, adapter) -> int:
    ctx.allow_install = True
    before = ctx.state_dir / "inventory" / f"{adapter.id}-before.json"
    ctx.state_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    before.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    if not before.exists():
        model.write_json(before, _inventory(ctx, adapter))
    adapter.install(ctx)
    print(json.dumps(adapter.status(ctx), indent=2, ensure_ascii=False))
    return 0


def _has_leftovers(value) -> bool:
    if isinstance(value, dict):
        return any(_has_leftovers(child) for child in value.values())
    return bool(value)


def uninstall(ctx: Ctx, adapter) -> int:
    ctx.allow_install = True
    before = ctx.state_dir / "inventory" / f"{adapter.id}-before.json"
    adapter.uninstall(ctx)
    after = _inventory(ctx, adapter)
    if before.exists():
        leftover = runner.inventory_diff(model.read_json(before), after)
    else:
        leftover = {"error": "before inventory missing; cannot verify leftovers"}
    model.write_json(before.with_name(f"{adapter.id}-leftover.json"), leftover)
    print(json.dumps(leftover, indent=2, ensure_ascii=False))
    if _has_leftovers(leftover):
        return 3
    before.unlink()
    return 0


def _preview(command: str, product: str) -> int:
    manifest = model.read_json(BENCH_DIR / "thirdparty.json")
    print(f"{command} {product} requires --yes")
    for item_id, item in manifest.get("items", {}).items():
        if item.get("product") == product:
            print(f"{item_id}: {item['version']} {item['license']} {item['url']}")
    return 1


def _report(args) -> int:
    current = _read_summary(args.summary)
    baseline = _read_summary(args.baseline) if args.baseline else None
    report = runner.dependency("report")
    rendered = report.render_report(current, baseline, None)
    if args.out:
        args.out.expanduser().write_text(rendered, encoding="utf-8")
    else:
        print(rendered, end="" if rendered.endswith("\n") else "\n")
    return 0


def _judge(args) -> int:
    current = _read_summary(args.summary)
    baseline = _read_summary(args.baseline) if args.baseline else None
    thresholds = model.read_json(args.thresholds.expanduser())
    judge = runner.dependency("judge")
    verdict = judge.judge(current, baseline, thresholds)
    for item in verdict.get("items", []):
        print(json.dumps(item, ensure_ascii=False, sort_keys=True))
    # Always end with a summary line so a clean run is not silent.
    counts = {key: verdict.get(key, 0) for key in ("regressions", "fails", "warns")}
    print(json.dumps({"summary": True, **counts}, sort_keys=True))
    if args.json:
        model.write_json(args.json.expanduser(), verdict)
    return judge.exit_code(verdict)


def main(argv=None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command in ("install", "uninstall") and not args.yes:
            return _preview(args.command, args.id)
        if args.command == "run":
            return run(args)
        if args.command == "selftest":
            return selftest(args)
        if args.command == "report":
            return _report(args)
        if args.command == "judge":
            return _judge(args)
        ctx = _ctx(args, _lab(args))
        if args.command == "status":
            return status(ctx, as_json=args.json)
        if args.command == "deploy":
            return deploy(ctx, args.exe)
        adapter = _adapter(args.id)
        return install(ctx, adapter) if args.command == "install" else uninstall(ctx, adapter)
    except (BenchError, OSError, ValueError) as exc:
        print(f"bench.py: {exc}", file=sys.stderr)
        return 2
    except KeyboardInterrupt:
        print("bench.py: interrupted", file=sys.stderr)
        return 130


if __name__ == "__main__":
    sys.exit(main())
