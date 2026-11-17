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

from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from paradl.core.config import PDLConfig
    from paradl.ui.display import LiveTransferDisplay as LiveTransferDisplayType

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


def run_tui(config: PDLConfig | None = None) -> None:
    """Launch the Textual-based TUI (lazy import to avoid circular imports)."""
    from paradl.ui.tui import run_tui as _run_tui

    _run_tui(config)


def LiveTransferDisplay(
    title: str, total_bytes: int | None = None, *, simple: bool = False
) -> LiveTransferDisplayType:
    """Rich Live progress display (lazy import to avoid circular imports)."""
    from paradl.ui.display import LiveTransferDisplay as _LTD

    # The core display class on this branch has no `simple` parameter; the flag is
    # accepted here only for call-site compatibility and is a no-op on this branch.
    return _LTD(title, total_bytes)


__all__ += ["LiveTransferDisplay", "run_tui"]
