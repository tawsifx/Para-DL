"""Diagnostics layer for pdl — system doctor, network probe, and watchdog benchmark.

Provides:
- run_doctor(): comprehensive system toolchain, platform, and macOS security diagnostics.
- run_netprobe(): 1/4/16-connection HTTP Range and ISP throttling diagnostic.
- run_bench(): watchdog v1/v2/off benchmark comparison.

These are standalone diagnostic commands invoked from the CLI.
"""

from __future__ import annotations

from paradl.diagnostics.bench import run_bench
from paradl.diagnostics.doctor import run_doctor
from paradl.diagnostics.netprobe import run_netprobe

__all__ = [
    "run_bench",
    "run_doctor",
    "run_netprobe",
]
