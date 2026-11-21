"""Enforce the hard constraint: no ``paradl.*`` imports anywhere in ``media/``."""

from __future__ import annotations

import ast
from pathlib import Path

MEDIA_ROOT = Path(__file__).parents[1]
PACKAGE = MEDIA_ROOT / "paradl_media"


def _import_roots(node: ast.AST) -> list[str]:
    roots: list[str] = []
    if isinstance(node, ast.Import):
        roots.extend(alias.name for alias in node.names)
    elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
        roots.append(node.module)
    return roots


def test_no_paradl_dot_imports() -> None:
    offenders: list[str] = []
    for path in PACKAGE.rglob("*.py"):
        tree = ast.parse(path.read_text(), filename=str(path))
        for node in ast.walk(tree):
            offenders.extend(
                f"{path.relative_to(MEDIA_ROOT)}: {root}"
                for root in _import_roots(node)
                if root == "paradl" or root.startswith("paradl.")
            )
    assert offenders == [], f"forbidden paradl.* imports: {offenders}"


def test_no_console_script_in_pyproject() -> None:
    text = (MEDIA_ROOT / "pyproject.toml").read_text()
    assert "[project.scripts]" not in text
    assert "paradl_media" in text
