"""CLI layer for pdl — argument parser, subcommand dispatch, and process entry point.

Provides:
- build_parser(): constructs the argparse.ArgumentParser with all pdl flags and recipes.
- run_cli(): parses argv, dispatches subcommands or downloads, returns an ExitCode int.
- main(): process entry point with signal handling.
- Subcommand dispatch logic for install, info, hash, extract, history, clean, config,
  diagnose, netprobe, bench, profiles, queue, resume, server, tui, throttle, pip, pypi, brew.

This is the user-facing command-line interface. It imports from the app and engines
layers but is not imported by them (no circular dependency).
"""

from __future__ import annotations

from paradl.cli.dispatch import SUBCOMMANDS, run_cli
from paradl.cli.main import _install_signal_handlers, main
from paradl.cli.parser import build_parser, configure_logging

__all__ = [
    "SUBCOMMANDS",
    "_install_signal_handlers",
    "build_parser",
    "configure_logging",
    "main",
    "run_cli",
]
