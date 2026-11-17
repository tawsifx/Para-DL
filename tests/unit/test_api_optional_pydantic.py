"""api/server.py must import even when pydantic/FastAPI are not installed.

CI installs only the `dev` group (no `api` extra), so pydantic is absent there.
The module's ImportError fallback must stub every name used at class-definition
time, otherwise `import paradl.api.server` raises NameError and every test that
imports it fails on CI (this regressed once via ``ConfigDict``).
"""

from __future__ import annotations

import builtins
import importlib
import sys

import pytest


def test_server_module_imports_without_pydantic(monkeypatch: pytest.MonkeyPatch) -> None:
    """Simulate pydantic/fastapi being absent and import the module from scratch."""
    real_import = builtins.__import__

    def blocking_import(name: str, *args: object, **kwargs: object) -> object:
        if name.split(".", 1)[0] in {"pydantic", "fastapi", "uvicorn"}:
            raise ImportError(f"blocked optional dependency: {name}")
        return real_import(name, *args, **kwargs)  # type: ignore[misc]

    # Drop any cached copy so the module body re-executes under the blocked imports.
    for mod in ("paradl.api.server",):
        sys.modules.pop(mod, None)
    monkeypatch.setattr(builtins, "__import__", blocking_import)
    try:
        module = importlib.import_module("paradl.api.server")
        # Class bodies that use ConfigDict must have executed without NameError.
        assert hasattr(module, "CreateTaskRequest")
        assert hasattr(module, "CreateTaskResponse")
    finally:
        monkeypatch.undo()
        sys.modules.pop("paradl.api.server", None)
