"""CLI subcommand dispatcher for `pdl throttle ...`."""

from __future__ import annotations

import argparse
import json
import os
import plistlib
import sys
import uuid
from collections.abc import Sequence
from dataclasses import asdict
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from rich.console import Console
from rich.table import Table

from pdl.clock import get_clock
from pdl.config import PDLConfig
from pdl.models import ExitCode
from pdl.paths import get_logs_dir, get_pdl_home, get_throttle_dir
from pdl.platform import macos
from pdl.throttle.budget import (
    DEFAULT_FULL_BUDGET_BYTES,
    DEFAULT_QUICK_BUDGET_BYTES,
    DEFAULT_SAMPLE_BUDGET_BYTES,
    DataBudgetTracker,
    parse_byte_budget,
)
from pdl.throttle.classifier import classify_dataset
from pdl.throttle.models import (
    BudgetSummary,
    MeasurementDataset,
    ThrottleReport,
    TimeVolumeUploadResult,
    ValidatedTarget,
)
from pdl.throttle.probes import (
    execute_diagnostic_run,
    execute_sample_probe,
)
from pdl.throttle.report import (
    rebuild_report_from_dict,
    render_terminal_report,
    report_from_dict,
    write_report_files,
)
from pdl.throttle.server import run_vps_server_blocking
from pdl.throttle.store import ThrottleStore
from pdl.throttle.targets import load_target_candidates, validate_candidates
from pdl.ui import UI, print_plain

THROTTLE_SUBCOMMANDS = (
    "quick",
    "full",
    "sample",
    "coverage",
    "launchd-plist",
    "report",
    "server",
    "targets",
)


def build_throttle_parser(config: PDLConfig) -> argparse.ArgumentParser:
    """Build the ArgumentParser for `pdl throttle <subcommand>`."""
    parser = argparse.ArgumentParser(
        prog="pdl throttle",
        description="ISP Throttling & Traffic Differentiation Diagnostic Suite.",
    )
    parser.add_argument(
        "subcommand",
        nargs="?",
        choices=THROTTLE_SUBCOMMANDS,
        default="quick",
        help="Diagnostic subcommand to run.",
    )
    parser.add_argument(
        "extra_args",
        nargs="*",
        help="Optional positional arguments (e.g. output path or run_id).",
    )
    parser.add_argument(
        "--targets",
        dest="targets_file",
        default=None,
        help="Path to custom targets.toml file.",
    )
    parser.add_argument(
        "--domestic",
        dest="domestic_urls",
        action="append",
        default=[],
        help="Domestic (in-country / BDIX / local peering) host or URL (repeatable).",
    )
    parser.add_argument(
        "--plan-mbps",
        dest="plan_mbps",
        type=float,
        default=None,
        help="Subscribed ISP plan speed in Mbps (e.g. 50).",
    )
    parser.add_argument(
        "--isp",
        dest="isp",
        default=config.isp,
        help=f"ISP display name for headers and evidence summary (default: '{config.isp}').",
    )
    parser.add_argument(
        "--via-proxy",
        dest="via_proxy",
        default=None,
        help="Optional HTTP or SOCKS5 proxy URL for paired M9 comparison (secrets redacted in logs/reports).",
    )
    parser.add_argument(
        "--interface",
        dest="interface",
        default=None,
        help="Preferred network interface name (e.g. en0).",
    )
    ip_group = parser.add_mutually_exclusive_group()
    ip_group.add_argument(
        "--ipv4",
        dest="ip_family",
        action="store_const",
        const="ipv4",
        default="auto",
        help="Force IPv4 target resolution.",
    )
    ip_group.add_argument(
        "--ipv6",
        dest="ip_family",
        action="store_const",
        const="ipv6",
        help="Force IPv6 target resolution.",
    )
    parser.add_argument(
        "--data-budget",
        dest="data_budget",
        default=None,
        help="Hard total data budget (e.g. 300MB, 3GB).",
    )
    parser.add_argument(
        "-o",
        "--output",
        dest="output_dir",
        default=None,
        help="Directory to write report files or LaunchAgent plist.",
    )
    parser.add_argument(
        "--html",
        dest="html",
        action="store_true",
        default=True,
        help="Generate self-contained HTML report with inline SVG charts (default: True).",
    )
    parser.add_argument(
        "--run-id",
        dest="run_id",
        default=None,
        help="Specific stored run ID to rebuild with `pdl throttle report`.",
    )
    parser.add_argument(
        "--force",
        dest="force",
        action="store_true",
        help="Bypass 45-minute sample idempotency window or battery skip.",
    )
    parser.add_argument(
        "--allow-32",
        dest="allow_32",
        action="store_true",
        help="Allow 32-stream probe in M3 flow scaling (default cap is 16).",
    )
    parser.add_argument(
        "--seed",
        dest="seed",
        type=int,
        default=42,
        help="RNG seed for deterministic jitter and test ordering.",
    )
    parser.add_argument(
        "--host",
        dest="server_host",
        default="127.0.0.1",
        help="Bind address for `pdl throttle server` (default: 127.0.0.1).",
    )
    parser.add_argument(
        "--port",
        dest="server_port",
        type=int,
        default=8765,
        help="TCP port for `pdl throttle server` (default: 8765).",
    )
    parser.add_argument(
        "--udp-port",
        dest="server_udp_port",
        type=int,
        default=8766,
        help="UDP echo port for `pdl throttle server` (default: 8766).",
    )
    parser.add_argument(
        "--duration",
        dest="server_duration",
        type=float,
        default=None,
        help="Optional duration in seconds for `pdl throttle server`.",
    )
    parser.add_argument(
        "--json",
        dest="json_mode",
        action="store_true",
        help="Emit structured JSON output to stdout.",
    )
    parser.add_argument(
        "-y",
        "--yes",
        dest="yes",
        action="store_true",
        help="Automatic confirmation for non-interactive runs.",
    )
    parser.add_argument(
        "--non-interactive",
        dest="non_interactive",
        action="store_true",
        help="Run without interactive prompts.",
    )
    return parser


def generate_throttle_launchd_plist(
    dest_path: Path | None = None,
    *,
    label: str = "com.pdl.throttle.sample",
    isp: str = "Duronto Internet",
) -> Path:
    """Generate (never load) a macOS LaunchAgent plist for sparse opportunistic sampling.

    Fires on login/wake (`RunAtLoad: True`) and hourly (`StartCalendarInterval: {"Minute": 0}`),
    invoking `pdl throttle sample --non-interactive`.
    """
    pdl_home = get_pdl_home()
    logs_dir = get_logs_dir()
    logs_dir.mkdir(parents=True, exist_ok=True)

    if dest_path is None:
        out_plist = Path.cwd() / f"{label}.plist"
    else:
        cand = Path(dest_path).expanduser().resolve()
        out_plist = cand / f"{label}.plist" if cand.is_dir() else cand

    out_plist.parent.mkdir(parents=True, exist_ok=True)

    prog_args = [
        sys.executable,
        "-m",
        "pdl.cli",
        "throttle",
        "sample",
        "--isp",
        isp,
        "--non-interactive",
    ]

    plist_dict: dict[str, Any] = {
        "Label": label,
        "ProgramArguments": prog_args,
        "RunAtLoad": True,
        "StartCalendarInterval": {"Minute": 0},
        "WorkingDirectory": str(out_plist.parent),
        "EnvironmentVariables": {
            "PDL_HOME": str(pdl_home),
            "PATH": os.environ.get(
                "PATH",
                "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            ),
        },
        "StandardOutPath": str(logs_dir / "throttle-sample.out.log"),
        "StandardErrorPath": str(logs_dir / "throttle-sample.err.log"),
    }

    xml_bytes = plistlib.dumps(plist_dict, fmt=plistlib.FMT_XML, sort_keys=True)
    out_plist.write_bytes(xml_bytes)

    ok, lint_msg = macos.validate_plist(out_plist)
    if not ok:
        raise ValueError(f"Generated throttle LaunchAgent plist failed validation: {lint_msg}")

    if UI.json_mode:
        sys.stdout.write(
            json.dumps(
                {
                    "event": "throttle_launchd_plist_generated",
                    "path": str(out_plist),
                    "label": label,
                    "lint": lint_msg,
                }
            )
            + "\n"
        )
        sys.stdout.flush()
    elif not UI.quiet:
        print_plain(
            f"  [green]✓ Generated opportunistic sampler LaunchAgent plist:[/green] [bold cyan]{out_plist}[/bold cyan]"
        )
        print_plain(f"  [dim]• Validation:[/dim] {lint_msg}")
        print_plain("\n  [bold white]Manual Load / Unload Instructions (never loaded automatically):[/bold white]")
        print_plain(f"    1. cp {out_plist} ~/Library/LaunchAgents/{label}.plist")
        print_plain(f"    2. launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/{label}.plist")
        print_plain(f"    3. To unload: launchctl bootout gui/$(id -u)/{label}\n")

    return out_plist


def _handle_targets_cmd(args: argparse.Namespace) -> int:
    t_path = Path(args.targets_file) if args.targets_file else None
    candidates = load_target_candidates(
        targets_file=t_path,
        domestic_urls=args.domestic_urls,
    )
    validated = validate_candidates(candidates)
    if args.json_mode:
        sys.stdout.write(
            json.dumps(
                {
                    "candidates": [asdict(c) for c in candidates],
                    "validated": [asdict(v) for v in validated],
                },
                indent=2,
            )
            + "\n"
        )
        sys.stdout.flush()
        return int(ExitCode.OK)

    con = Console()
    table = Table(title=f"Validated Diagnostic Targets (ISP: {args.isp})")
    table.add_column("ID", style="bold")
    table.add_column("Group")
    table.add_column("Locality")
    table.add_column("Host / IP")
    table.add_column("ASN / Country")
    table.add_column("Range Support")
    for v in validated:
        table.add_row(
            v.id,
            v.group,
            v.locality,
            f"{v.host} ({v.ip})",
            f"{v.asn} ({v.country})",
            "206 OK" if v.supports_range else "200-only",
        )
    con.print(table)
    return int(ExitCode.OK)


def _handle_coverage_cmd(args: argparse.Namespace) -> int:
    samples = ThrottleStore.get_samples(limit=1000)
    cells, next_hours = ThrottleStore.build_coverage_matrix(samples)
    populated = [c for c in cells if c.has_data]

    if args.json_mode:
        sys.stdout.write(
            json.dumps(
                {
                    "isp": args.isp,
                    "populated_cells": len(populated),
                    "total_cells": len(cells),
                    "next_best_hours": next_hours,
                    "cells": [asdict(c) for c in cells],
                },
                indent=2,
            )
            + "\n"
        )
        sys.stdout.flush()
        return int(ExitCode.OK)

    con = Console()
    table = Table(title=f"Time-of-Day Coverage Matrix — {args.isp} ({len(populated)}/{len(cells)} cells populated)")
    table.add_column("Hour", style="bold")
    table.add_column("Weekday (Active / Passive / Median)", justify="right")
    table.add_column("Weekend (Active / Passive / Median)", justify="right")

    cell_by_key = {(c.hour, c.is_weekend): c for c in cells}
    for hour in range(24):
        wd = cell_by_key[(hour, False)]
        we = cell_by_key[(hour, True)]

        def _fmt_cell(c: Any) -> str:
            if not c.has_data:
                return "[dim]no data[/dim]"
            mbps = ((c.median_bps or 0.0) * 8.0) / 1_000_000.0
            return f"{c.active_count}A / {c.passive_count}P ({mbps:.1f} Mbps)"

        table.add_row(f"{hour:02d}:00", _fmt_cell(wd), _fmt_cell(we))

    con.print(table)
    con.print(f"[bold cyan]Next Most Informative Hours to Sample:[/bold cyan] {', '.join(next_hours)}")
    return int(ExitCode.OK)


def _handle_sample_cmd(args: argparse.Namespace) -> int:
    budget_bytes = parse_byte_budget(args.data_budget, DEFAULT_SAMPLE_BUDGET_BYTES)
    budget = DataBudgetTracker(
        limit_bytes=budget_bytes,
        seed=args.seed,
    )
    planned = budget.planned_worst_case_bytes
    if not args.json_mode:
        print_plain(
            f"  [cyan]Planned worst-case data use for 20s sample probe ({args.isp}): "
            f"{planned / (1024 * 1024):.1f} MB (hard cap: {budget_bytes / (1024 * 1024):.1f} MB)[/cyan]",
            force=True,
        )

    # Check idempotency / battery before validating network targets when not forced
    can_run, elapsed = ThrottleStore.can_record_active_sample()
    if not can_run and not (args.force or args.yes):
        msg = f"Skipped active sample (idempotent: last sample stored {elapsed / 60.0:.1f} min ago, < 45 min window)."
        if args.json_mode:
            sys.stdout.write(json.dumps({"status": "skipped", "reason": "idempotency_window", "message": msg}) + "\n")
            sys.stdout.flush()
        else:
            print_plain(f"  [dim]{msg}[/dim]", force=True)
        return int(ExitCode.OK)

    from pdl.throttle.probes import _run_tool

    on_ac, _, _ = macos.parse_pmset_batt(_run_tool(["pmset", "-g", "batt"]))
    if not on_ac and not (args.force or args.yes):
        msg = "Skipped active sample on Battery Power (pass --force or --yes to run on battery)."
        if args.json_mode:
            sys.stdout.write(json.dumps({"status": "skipped", "reason": "on_battery", "message": msg}) + "\n")
            sys.stdout.flush()
        else:
            print_plain(f"  [yellow]{msg}[/yellow]", force=True)
        return int(ExitCode.OK)

    t_path = Path(args.targets_file) if args.targets_file else None
    candidates = load_target_candidates(
        targets_file=t_path,
        domestic_urls=args.domestic_urls,
    )
    validated = validate_candidates(candidates)
    ran, message, sample = execute_sample_probe(
        validated,
        budget,
        force=bool(args.force or args.yes),
        duration_sec=1.2,
    )
    if not ran or sample is None:
        if args.json_mode:
            sys.stdout.write(json.dumps({"status": "skipped", "message": message}) + "\n")
            sys.stdout.flush()
        else:
            print_plain(f"  [yellow]{message}[/yellow]", force=True)
        return int(ExitCode.OK)

    if args.json_mode:
        sys.stdout.write(json.dumps({"status": "recorded", "sample": asdict(sample)}, indent=2) + "\n")
        sys.stdout.flush()
    else:
        mbps = (sample.download_bps * 8.0) / 1_000_000.0
        print_plain(
            f"  [green]✓ {message} ({mbps:.2f} Mbps, RTT {sample.rtt_ms:.1f} ms)[/green]",
            force=True,
        )
    return int(ExitCode.OK)


def _handle_report_cmd(args: argparse.Namespace) -> int:
    out_dir = Path(args.output_dir).expanduser().resolve() if args.output_dir else get_throttle_dir()
    stored = ThrottleStore.get_latest_run_dict()
    if stored is None:
        samples = ThrottleStore.get_samples(limit=1000)
        dataset = MeasurementDataset(
            isp=args.isp,
            mode="report",
            plan_mbps=args.plan_mbps,
            m8_time_volume_upload=TimeVolumeUploadResult(samples=samples),
        )
        classification = classify_dataset(dataset)
        report = ThrottleReport(
            run_id="from-samples",
            mode="report",
            isp=args.isp,
            plan_mbps=args.plan_mbps,
            timestamp="rebuilt",
            budget=BudgetSummary(limit_bytes=0, planned_worst_case_bytes=0, actual_used_bytes=0),
            targets=[],
            dataset=dataset,
            classification=classification,
        )
        paths = write_report_files(report, out_dir, include_html=bool(args.html))
        if args.json_mode:
            sys.stdout.write(json.dumps(report.to_dict(), indent=2) + "\n")
            sys.stdout.flush()
        else:
            render_terminal_report(report)
            print_plain(
                f"  [green]✓ Rebuilt report files in {out_dir}: {', '.join(str(p) for p in paths.values())}[/green]",
                force=True,
            )
        return int(ExitCode.OK)

    if args.isp:
        stored["isp"] = args.isp
    paths = rebuild_report_from_dict(stored, out_dir, include_html=bool(args.html))
    if args.json_mode:
        sys.stdout.write(json.dumps(stored, indent=2) + "\n")
        sys.stdout.flush()
    else:
        render_terminal_report(report_from_dict(stored))
        print_plain(
            f"  [green]✓ Rebuilt report files in {out_dir}: {', '.join(str(p) for p in paths.values())}[/green]",
            force=True,
        )
    return int(ExitCode.OK)


def _handle_quick_or_full_cmd(args: argparse.Namespace, mode: str) -> int:
    clock = get_clock()
    default_budget = DEFAULT_QUICK_BUDGET_BYTES if mode == "quick" else DEFAULT_FULL_BUDGET_BYTES
    budget_bytes = parse_byte_budget(args.data_budget, default_budget)
    budget = DataBudgetTracker(
        limit_bytes=budget_bytes,
        seed=args.seed,
    )
    planned_bytes = budget.planned_worst_case_bytes

    # Always print planned worst-case data use before starting
    print_plain(
        f"  [bold cyan]Planned worst-case data use ({mode} mode, ISP: {args.isp}): "
        f"{planned_bytes / (1024 * 1024):.1f} MB "
        f"(hard budget cap: {budget_bytes / (1024 * 1024):.1f} MB)[/bold cyan]",
        force=True,
    )

    t_path = Path(args.targets_file) if args.targets_file else None
    candidates = load_target_candidates(
        targets_file=t_path,
        domestic_urls=args.domestic_urls,
    )
    validated: list[ValidatedTarget] = validate_candidates(candidates)

    dataset = execute_diagnostic_run(
        mode=mode,
        isp=args.isp,
        targets=validated,
        budget=budget,
        plan_mbps=args.plan_mbps,
        proxy_url=args.via_proxy,
        m2_duration_sec=0.8 if mode == "quick" else 1.5,
        m3_step_duration_sec=0.25 if mode == "quick" else 0.4,
    )
    classification = classify_dataset(dataset)
    now_dt = datetime.fromtimestamp(clock.now(), tz=UTC).astimezone()
    run_id = f"{mode}-{now_dt.strftime('%Y%m%d-%H%M%S')}-{uuid.uuid4().hex[:6]}"

    report = ThrottleReport(
        run_id=run_id,
        mode=mode,
        isp=args.isp,
        plan_mbps=args.plan_mbps,
        timestamp=now_dt.strftime("%Y-%m-%dT%H:%M:%S%z"),
        budget=budget.to_summary(),
        targets=validated,
        dataset=dataset,
        classification=classification,
    )

    ThrottleStore.save_run(report)
    ThrottleStore.persist_recommended_profile(
        classification.recommended_profile,
        validated,
        dataset,
    )

    out_dir = Path(args.output_dir).expanduser().resolve() if args.output_dir else get_throttle_dir()
    paths = write_report_files(report, out_dir, include_html=bool(args.html))

    if args.json_mode:
        sys.stdout.write(json.dumps(report.to_dict(), indent=2) + "\n")
        sys.stdout.flush()
    else:
        render_terminal_report(report)
        print_plain(
            f"  [green]✓ Wrote diagnostic report files:[/green] "
            f"{paths['md']}, {paths['json']}" + (f", {paths['html']}" if "html" in paths else ""),
            force=True,
        )
    return int(ExitCode.OK)


def run_throttle_cli(argv: Sequence[str], config: PDLConfig) -> int:
    """Parse and execute `pdl throttle <subcommand>`."""
    parser = build_throttle_parser(config)
    try:
        args = parser.parse_args(list(argv))
    except SystemExit as exc:
        return exc.code if isinstance(exc.code, int) else 0

    UI.json_mode = bool(args.json_mode)
    UI.non_interactive = bool(args.non_interactive)

    sub = args.subcommand
    if sub == "targets":
        return _handle_targets_cmd(args)
    if sub == "coverage":
        return _handle_coverage_cmd(args)
    if sub == "sample":
        return _handle_sample_cmd(args)
    if sub == "launchd-plist":
        dest = Path(args.output_dir) if args.output_dir else (Path(args.extra_args[0]) if args.extra_args else None)
        generate_throttle_launchd_plist(dest, isp=args.isp)
        return int(ExitCode.OK)
    if sub == "report":
        return _handle_report_cmd(args)
    if sub == "server":
        run_vps_server_blocking(
            host=args.server_host,
            tcp_port=args.server_port,
            udp_port=args.server_udp_port,
            duration_sec=args.server_duration,
        )
        return int(ExitCode.OK)
    if sub in ("quick", "full"):
        return _handle_quick_or_full_cmd(args, mode=sub)

    return int(ExitCode.USAGE)
