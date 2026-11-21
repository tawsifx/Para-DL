"""Make the benchmark modules importable when tests run from the repo root."""

from __future__ import annotations

import sys
from pathlib import Path

BENCHMARKS_DIR = Path(__file__).parent
if str(BENCHMARKS_DIR) not in sys.path:
    sys.path.insert(0, str(BENCHMARKS_DIR))
