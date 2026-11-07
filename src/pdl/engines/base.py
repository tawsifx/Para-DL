"""Engine protocol definition for swappable download backends."""

from __future__ import annotations

from pathlib import Path
from typing import Any, Protocol, runtime_checkable

from pdl.models import TargetInfo, TransferResult


@runtime_checkable
class Engine(Protocol):
    """Protocol implemented by all pdl download engines."""

    def execute(
        self,
        target_info: TargetInfo,
        out_dir: Path,
        *,
        connections: int = 16,
        **options: Any,
    ) -> TransferResult:
        """Execute transfer for target_info into out_dir and return a TransferResult."""
        ...
