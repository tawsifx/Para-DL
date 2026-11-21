"""One-shot entry point: ``python -m paradl_media``.

Reads one request line from stdin, emits protocol frames to stdout (only), and
returns the process exit code derived from ``error.kind`` (Rust owns the full
ExitCode mapping; this worker never emits 3).
"""

from __future__ import annotations

import sys

from .worker import install_signal_handlers, run


def main() -> int:
    """Install signal handlers and execute exactly one protocol cycle."""
    install_signal_handlers()
    return run()


if __name__ == "__main__":
    sys.exit(main())
