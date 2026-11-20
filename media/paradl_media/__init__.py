"""Standalone one-shot Python media worker for paraDL (`paradl-media/1`).

This package deliberately has **no** ``paradl.*`` imports. It is invoked as
``python -m paradl_media`` and communicates strictly over the versioned
JSON-lines protocol specified in ``docs/superpowers/framework-charter.md`` §4.
"""

from __future__ import annotations

PROTOCOL_ID = "paradl-media/1"
CURRENT_MAJOR = 1
WORKER_VERSION = "0.1.0"

__all__ = [
    "CURRENT_MAJOR",
    "PROTOCOL_ID",
    "WORKER_VERSION",
]
