"""REST API server layer for pdl — FastAPI-based JSON-RPC / HTTP download API.

Provides:
- run_server(): starts the FastAPI server on a given host/port with optional auto-reload.
- Task submission, status, and result endpoints for remote download management.

Optional dependency: fastapi (install via 'pip install parallel-dl[api]').
"""

from __future__ import annotations

from paradl.api.server import run_server

__all__ = [
    "run_server",
]
