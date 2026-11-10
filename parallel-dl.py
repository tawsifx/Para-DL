#!/usr/bin/env python3
"""Backwards-compatible entrypoint shim delegating to the modular `paradl` package.

The untouched v3.1.0-pro original is preserved in `./_original/parallel-dl.py`.
"""

from __future__ import annotations

import sys
from pathlib import Path

_SRC_DIR = Path(__file__).resolve().parent / "src"
if str(_SRC_DIR) not in sys.path:
    sys.path.insert(0, str(_SRC_DIR))

from paradl.cli import main  # noqa: E402

if __name__ == "__main__":
    main()
