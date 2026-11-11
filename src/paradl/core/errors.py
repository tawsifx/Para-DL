"""Custom exception types for pdl.

Provides a base PDLError and specific exception categories used across
the download, engine, resilience, and storage layers.
"""

from __future__ import annotations


class PDLError(Exception):
    """Base exception for all pdl errors."""


class DownloadError(PDLError):
    """Raised when a download fails after all retries and fallbacks."""


class ChecksumError(PDLError):
    """Raised when checksum verification fails."""


class ThrottleError(PDLError):
    """Raised when ISP throttling is detected and cannot be mitigated."""


class EngineError(PDLError):
    """Raised when a download engine encounters an unrecoverable error."""


class ValidationError(PDLError):
    """Raised when configuration or input validation fails."""


__all__ = [
    "ChecksumError",
    "DownloadError",
    "EngineError",
    "PDLError",
    "ThrottleError",
    "ValidationError",
]
