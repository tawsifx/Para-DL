"""Process entry point for the `pdl` and `parallel-dl` console scripts.

Installs SIGINT/SIGTERM handlers and delegates to run_cli().
"""

from __future__ import annotations

import signal
import sys

from paradl.cli.dispatch import run_cli
from paradl.core.models import ExitCode
from paradl.ui import UI, print_plain


def _install_signal_handlers() -> None:
    """Install SIGINT/SIGTERM handlers at runtime inside main(), never at import time."""

    def _handle_sig(signum: int, _frame: object) -> None:
        from paradl.engines.aria2 import cleanup_children_and_temps

        cleanup_children_and_temps(remove_partial_raw=False)
        if UI.json_mode:
            UI.emit_event("interrupted", signal=signum, exit_code=int(ExitCode.INTERRUPTED))
        else:
            print_plain(
                "\n  [warning]⚠ Download interrupted. Partial state preserved for instant resume.[/warning]",
                force=True,
            )
        sys.exit(int(ExitCode.INTERRUPTED))

    signal.signal(signal.SIGINT, _handle_sig)
    signal.signal(signal.SIGTERM, _handle_sig)


def main() -> None:
    """Entrypoint for the `pdl` and `parallel-dl` console scripts."""
    _install_signal_handlers()
    try:
        exit_code = run_cli()
    except KeyboardInterrupt:
        from paradl.engines.aria2 import cleanup_children_and_temps

        cleanup_children_and_temps(remove_partial_raw=False)
        exit_code = int(ExitCode.INTERRUPTED)
    sys.exit(exit_code)


if __name__ == "__main__":
    main()
