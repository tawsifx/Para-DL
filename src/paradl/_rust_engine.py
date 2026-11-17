"""Python shim for the native ``mdm-engine`` PyO3 extension.

The compiled extension is published as ``paradl._rust_engine`` (maturin writes a
``.so`` that shadows this source file once built).  Before it is compiled — or
on a platform where the wheel is unavailable — this module is imported instead.
It keeps the public API importable and degrades gracefully: every callable
raises :class:`RustEngineUnavailable` with an actionable message, so the rest of
``paradl`` imports cleanly and the adapter can be exercised with an injected
fake native module.
"""

from __future__ import annotations

import contextlib
import importlib
from typing import Any

__all__ = [
    "NATIVE_MODULE",
    "DownloadHandle",
    "RustDownloadEngine",
    "RustEngineUnavailable",
    "__version__",
    "is_available",
    "probe_url",
    "resume_download",
    "start_download",
    "version",
]

# Candidate names the compiled extension may be published under.  When maturin
# emits ``paradl._rust_engine`` directly it shadows this file, so it is not a
# candidate here (importing it would recurse).
_NATIVE_CANDIDATES: tuple[str, ...] = (
    "paradl._rust_engine_native",
    "paradl._paradl_rs",
    "paradl._mdm_engine",
    "_paradl_rs",
    "_rust_engine_native",
)

_UNAVAILABLE_MSG = (
    "Native Rust engine (paradl._rust_engine) is not available. "
    "Build it with `maturin develop --release` inside rust/paradl-rs, "
    "or select the legacy backend with --engine aria2."
)


class RustEngineUnavailable(RuntimeError):
    """Raised when a native engine callable is used before the extension is built."""


class _UnavailableHandle:
    """Placeholder exposing the DownloadHandle surface without a live transfer."""

    def poll_snapshot(self) -> dict[str, Any]:
        raise RustEngineUnavailable(_UNAVAILABLE_MSG)

    def recycle_connections(self) -> None:
        raise RustEngineUnavailable(_UNAVAILABLE_MSG)

    def pause(self) -> None:
        raise RustEngineUnavailable(_UNAVAILABLE_MSG)

    def resume(self) -> None:
        raise RustEngineUnavailable(_UNAVAILABLE_MSG)

    def cancel(self) -> None:
        raise RustEngineUnavailable(_UNAVAILABLE_MSG)

    def wait(self, timeout: float | None = None) -> dict[str, Any]:
        raise RustEngineUnavailable(_UNAVAILABLE_MSG)


DownloadHandle = _UnavailableHandle


class _UnavailableEngine:
    """Placeholder exposing the RustDownloadEngine surface."""

    def start_download(self, *args: Any, **kwargs: Any) -> DownloadHandle:
        raise RustEngineUnavailable(_UNAVAILABLE_MSG)

    def probe_url(self, url: str, headers: dict[str, str] | None = None) -> dict[str, Any]:
        raise RustEngineUnavailable(_UNAVAILABLE_MSG)


RustDownloadEngine = _UnavailableEngine


def _unavailable(*args: Any, **kwargs: Any) -> Any:
    raise RustEngineUnavailable(_UNAVAILABLE_MSG)


start_download = _unavailable
resume_download = _unavailable
probe_url = _unavailable


def is_available() -> bool:
    """Return True when a compiled native extension backs this shim."""
    return bool(globals().get("_NATIVE_AVAILABLE", False))


def version() -> str | None:
    """Return the native engine version string, or None when unavailable."""
    value: Any = globals().get("__version__")
    return str(value) if value else None


__version__: str | None = None
NATIVE_MODULE: str | None = None


def _load_native() -> Any:
    """Import the first available compiled native module, or return None."""
    for name in _NATIVE_CANDIDATES:
        try:
            return importlib.import_module(name)
        except ImportError:
            continue
    return None


def _bind_native(module: Any) -> None:
    """Re-export the native module's public symbols at shim module scope."""
    globals()["NATIVE_MODULE"] = module.__name__
    for attr in ("RustDownloadEngine", "DownloadHandle"):
        replacement = getattr(module, attr, None)
        if replacement is not None:
            globals()[attr] = replacement
    for attr in ("probe_url", "start_download", "resume_download"):
        replacement = getattr(module, attr, None)
        if callable(replacement):
            globals()[attr] = replacement

    version_value = getattr(module, "__version__", None) or getattr(module, "VERSION", None)
    if callable(getattr(module, "version", None)):
        with contextlib.suppress(Exception):  # defensive around third-party code
            version_value = module.version() or version_value
    globals()["__version__"] = version_value

    def _available() -> bool:
        return True

    def _native_version() -> str | None:
        value: Any = globals().get("__version__")
        return str(value) if value else None

    globals()["is_available"] = _available
    globals()["version"] = _native_version
    globals()["_NATIVE_AVAILABLE"] = True


_external_native = _load_native()
if _external_native is not None:
    _bind_native(_external_native)
else:
    globals()["_NATIVE_AVAILABLE"] = False
