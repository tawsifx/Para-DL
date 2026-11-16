"""Deterministic stand-in for macOS CLI tools used by paradl.

Every command's output comes from a checked-in golden file under
``tests/support/golden``; no ``subprocess`` call is ever made. Golden strings
are shaped to be parseable by :mod:`paradl.platform.macos` helpers.
"""

from __future__ import annotations

from pathlib import Path

_GOLDEN_DIR = Path(__file__).resolve().parent / "golden"


class FakeMacOSRunner:
    """Returns fixed, golden-backed output for scutil/pmset/caffeinate/codesign/xattr."""

    def __init__(self, *, golden_dir: Path | None = None) -> None:
        self.golden_dir = Path(golden_dir) if golden_dir is not None else _GOLDEN_DIR
        self.calls: list[tuple[str, tuple[str, ...]]] = []

    def golden(self, name: str) -> str:
        """Read a golden output file by name."""
        return (self.golden_dir / name).read_text(encoding="utf-8")

    def _record(self, tool: str, args: tuple[str, ...]) -> None:
        self.calls.append((tool, args))

    def pmset(self, *args: str) -> str:
        self._record("pmset", args)
        if args == ("-g", "batt"):
            return self.golden("pmset_batt.txt")
        return self.golden("pmset_general.txt")

    def scutil(self, *args: str) -> str:
        self._record("scutil", args)
        if args == ("--nwi",):
            return self.golden("scutil_nwi.txt")
        return self.golden("scutil_query.txt")

    def caffeinate(self, *args: str) -> str:
        self._record("caffeinate", args)
        return self.golden("caffeinate.txt")

    def codesign(self, *args: str) -> str:
        self._record("codesign", args)
        return self.golden("codesign.txt")

    def xattr(self, *args: str) -> str:
        self._record("xattr", args)
        return self.golden("xattr.txt")

    def run(self, tool: str, *args: str) -> str:
        """Dispatch by tool name, for tests that drive commands generically."""
        runners = {
            "pmset": self.pmset,
            "scutil": self.scutil,
            "caffeinate": self.caffeinate,
            "codesign": self.codesign,
            "xattr": self.xattr,
        }
        runner = runners.get(tool)
        if runner is None:
            raise ValueError(f"No fake runner for tool: {tool!r}")
        return runner(*args)


__all__ = ["FakeMacOSRunner"]
