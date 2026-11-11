"""Application orchestrator layer for pdl — PDLApp, subcommand handlers, and target inspector.

Provides:
- PDLApp: main application controller that wires together config, engines, queue,
  history, checksum verification, archive extraction, ADB sync, and platform features.
- Subcommand command handlers: config, profiles, queue, resume, etc.
- TargetInspector: metadata classification without downloading.

This is the top-level orchestration layer. It imports from engines, resilience,
storage, net, ui, and platform, but is not imported by them.
"""

from __future__ import annotations

from paradl.app.commands import PDLApp
from paradl.app.inspector import TargetInspector

__all__ = [
    "PDLApp",
    "TargetInspector",
]
