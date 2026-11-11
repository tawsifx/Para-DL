"""UI and display layer for pdl — rich console output, JSON-lines events, and status display.

Provides:
- UI: shared console, print helpers, JSON-lines event emission, quiet/json modes.
- print_plain: force-printed plain text messages.
- SecretRedactingFilter: logging filter that redacts tokens/secrets.
- check_optional_dep: verify optional dependencies (fastapi, textual).
- LiveTransferDisplay: Rich Live progress and watchdog status display.
- run_tui: Textual-based TUI (lazily imported to avoid circular imports).

Used by the CLI and app layers for all user-facing output.
"""

from __future__ import annotations

from paradl.ui.display import (
    UI,
    SecretRedactingFilter,
    check_optional_dep,
    print_plain,
    redact_secrets,
)

__all__ = [
    "UI",
    "SecretRedactingFilter",
    "check_optional_dep",
    "print_plain",
    "redact_secrets",
]


def run_tui():
    """Launch the Textual-based TUI (lazy import to avoid circular imports)."""
    from paradl.ui.tui import run_tui as _run_tui

    return _run_tui()


def LiveTransferDisplay(*args, **kwargs):
    """Rich Live progress display (lazy import to avoid circular imports)."""
    from paradl.ui.display import LiveTransferDisplay as _LTD

    return _LTD(*args, **kwargs)


__all__ += ["LiveTransferDisplay", "run_tui"]
