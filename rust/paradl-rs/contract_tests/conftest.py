"""Test setup for the S2 Task 2 contract guards.

These guards live under ``rust/paradl-rs/`` (S2-owned) rather than ``tests/``
(S3-owned). Pin the worktree's ``src`` at the front of ``sys.path`` so the tests
always exercise the worktree code — the venv's editable install points at the
main work tree, which does not carry in-flight edits.
"""

from __future__ import annotations

import sys
from pathlib import Path

_SRC = Path(__file__).resolve().parents[3] / "src"
if str(_SRC) not in sys.path:
    sys.path.insert(0, str(_SRC))
