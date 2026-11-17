"""CLI subcommand dispatch and download orchestration entry point.

Parses argv, dispatches subcommands (install/info/hash/extract/history/clean/config/
diagnose/netprobe/bench/profiles/queue/resume/server/tui/throttle/pip/pypi/brew) or
downloads, and returns an ExitCode int.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from collections.abc import Sequence

from paradl import VERSION
from paradl.core.config import PDLConfig
from paradl.core.models import ExitCode
from paradl.ui import UI, check_optional_dep, print_plain


def run_cli(argv: Sequence[str] | None = None) -> int:
    """Parse CLI arguments, dispatch subcommands or downloads, and return an ExitCode int."""
    raw_args = list(sys.argv[1:] if argv is None else argv)

    # Fast path for --version
    if raw_args == ["--version"]:
        sys.stdout.write(f"parallel-dl (paradl) v{VERSION}\n")
        sys.stdout.flush()
        return int(ExitCode.OK)

    config = PDLConfig.load()

    if raw_args and raw_args[0].lower() == "throttle":
        from paradl.throttle.cli import run_throttle_cli

        return run_throttle_cli(raw_args[1:], config)

    from paradl.cli.parser import build_parser

    parser = build_parser(config)

    try:
        args = parser.parse_args(raw_args)
    except SystemExit as exc:
        return (
            exc.code
            if isinstance(exc.code, int)
            else (0 if exc.code is None else int(ExitCode.USAGE))
        )

    # Configure UI and logging state
    UI.quiet = bool(args.quiet)
    UI.json_mode = bool(args.json)
    UI.non_interactive = bool(args.non_interactive)
    from paradl.cli.parser import configure_logging

    configure_logging(verbose=bool(args.verbose), debug=bool(args.debug), json_mode=bool(args.json))

    # Disambiguate positional subcommands while preserving free-text targets
    pos: list[str] = list(args.targets or [])
    if pos and pos[0].lower() in SUBCOMMANDS:
        sub = pos[0].lower()
        rest = pos[1:]
        args.targets = []

        if sub == "install":
            if not rest:
                print_plain("  [error]Usage: pdl install <target> [--yes][/error]", force=True)
                return int(ExitCode.USAGE)
            args.install_target = rest[0]
        elif sub in ("pip", "pypi"):
            if not rest:
                print_plain(
                    f"  [error]Usage: pdl {sub} [install|upgrade] <package>[/error]", force=True
                )
                return int(ExitCode.USAGE)
            cmd0 = rest[0].lower()
            if cmd0 in ("install", "upgrade"):
                if len(rest) < 2:
                    print_plain(f"  [error]Usage: pdl {sub} {cmd0} <package>[/error]", force=True)
                    return int(ExitCode.USAGE)
                args.install_target = f"pip:{rest[1]}"
            else:
                args.targets = [f"pip:{r}" if not r.startswith("-") else r for r in rest]
        elif sub == "brew":
            if not rest:
                print_plain(
                    "  [error]Usage: pdl brew [install|update|upgrade] <formula>[/error]",
                    force=True,
                )
                return int(ExitCode.USAGE)
            cmd0 = rest[0].lower()
            if cmd0 == "update":
                brew_bin = shutil.which("brew")
                if not brew_bin:
                    print_plain(
                        "  [error]Homebrew ('brew') binary not found on PATH.[/error]", force=True
                    )
                    return int(ExitCode.MISSING_DEPENDENCY)
                print_plain(
                    "  [dim]•[/dim] [white]Updating Homebrew formulae catalog (brew update)...[/white]"
                )
                env = dict(os.environ)
                env["NONINTERACTIVE"] = "1"
                proc = subprocess.run([brew_bin, "update", *rest[1:]], env=env, check=False)
                return proc.returncode
            if cmd0 in ("install", "upgrade"):
                if len(rest) < 2:
                    if cmd0 == "upgrade":
                        brew_bin = shutil.which("brew")
                        if not brew_bin:
                            print_plain(
                                "  [error]Homebrew ('brew') binary not found on PATH.[/error]",
                                force=True,
                            )
                            return int(ExitCode.MISSING_DEPENDENCY)
                        print_plain(
                            "  [dim]•[/dim] [white]Upgrading outdated Homebrew packages...[/white]"
                        )
                        env = dict(os.environ)
                        env["NONINTERACTIVE"] = "1"
                        proc = subprocess.run([brew_bin, "upgrade"], env=env, check=False)
                        return proc.returncode
                    print_plain(f"  [error]Usage: pdl brew {cmd0} <formula>[/error]", force=True)
                    return int(ExitCode.USAGE)
                args.install_target = f"brew:{rest[1]}"
            else:
                args.targets = [f"brew:{r}" if not r.startswith("-") else r for r in rest]
        elif sub == "info":
            if not rest:
                print_plain("  [error]Usage: pdl info <target>[/error]", force=True)
                return int(ExitCode.USAGE)
            args.info_target = rest[0]
        elif sub == "hash":
            if not rest:
                print_plain("  [error]Usage: pdl hash <file>[/error]", force=True)
                return int(ExitCode.USAGE)
            args.hash_file = rest[0]
        elif sub == "extract":
            if not rest:
                print_plain("  [error]Usage: pdl extract <archive> [dest_dir][/error]", force=True)
                return int(ExitCode.USAGE)
            args.extract_file = rest[0]
            if len(rest) > 1 and args.output is None:
                args.output = rest[1]
        elif sub == "history":
            args.history = True
            if rest:
                try:
                    args.history_limit = int(rest[0])
                except ValueError:
                    print_plain(f"  [error]Invalid history limit: {rest[0]!r}[/error]", force=True)
                    return int(ExitCode.USAGE)
        elif sub == "clean":
            args.clean = True
            if rest and args.output is None:
                args.output = rest[0]
        elif sub == "config":
            from paradl.app import PDLApp

            app = PDLApp(config=config)
            action = rest[0] if len(rest) > 0 else "show"
            key = rest[1] if len(rest) > 1 else None
            val = rest[2] if len(rest) > 2 else None
            return app.handle_config_command(action, key, val)
        elif sub in ("diagnose", "doctor"):
            args.doctor = True
            args.diagnose = True
        elif sub == "auth":
            from paradl.auth import handle_auth_command

            action = rest[0] if len(rest) > 0 else "status"
            service = rest[1] if len(rest) > 1 else None
            return handle_auth_command(action, service)
        elif sub == "netprobe":
            if not rest:
                print_plain(
                    "  [error]Usage: pdl netprobe <url> [--duration 60] [--http3-probe][/error]",
                    force=True,
                )
                return int(ExitCode.USAGE)
            args.netprobe_url = rest[0]
        elif sub == "bench":
            if not rest:
                print_plain(
                    "  [error]Usage: pdl bench <url> [--watchdog v1,v2,off] [--duration 60][/error]",
                    force=True,
                )
                return int(ExitCode.USAGE)
            args.bench_url = rest[0]
        elif sub == "profiles":
            from paradl.app import PDLApp

            app = PDLApp(config=config)
            action = rest[0] if len(rest) > 0 else "list"
            target_host = rest[1] if len(rest) > 1 else None
            return app.handle_profiles_command(action, target_host)
        elif sub == "queue":
            from paradl.app import PDLApp

            app = PDLApp(config=config)
            action = rest[0] if len(rest) > 0 else "list"
            q_args = rest[1:]
            return app.handle_queue_command(action, q_args, args)
        elif sub == "resume":
            from paradl.app import PDLApp

            app = PDLApp(config=config)
            return app.handle_resume_command(rest, args)
        elif sub == "server":
            check_optional_dep("fastapi", "api", install_cmd="pip install 'parallel-dl[api]'")
            from paradl.api import run_server

            run_server(
                host=args.host,
                port=args.port,
                reload=args.reload,
            )
            return int(ExitCode.OK)
        elif sub == "tui":
            check_optional_dep("textual", "tui")
            from paradl.ui.tui import run_tui

            run_tui()
            return int(ExitCode.OK)

    # Fast path for `pdl hash <file>` without loading PDLApp banner when --quiet or --json
    if getattr(args, "hash_file", None) and not args.targets:
        from paradl.app import PDLApp

        app = PDLApp(config=config)
        return app.execute(args)

    from paradl.app import PDLApp

    app = PDLApp(config=config)
    return app.execute(args)


# Re-export SUBCOMMANDS for backward compatibility
from paradl.cli.parser import SUBCOMMANDS  # noqa: E402

__all__ = [
    "SUBCOMMANDS",
    "run_cli",
]
