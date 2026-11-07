#!/usr/bin/env bash
set -euo pipefail

# pdl Quality Gate: Ruff, Mypy, and Fast Test Suite
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${ROOT_DIR}"

VENV_BIN="${ROOT_DIR}/.venv/bin"

echo "=== 1. Checking Code Formatting and Linting (Ruff) ==="
"${VENV_BIN}/ruff" check .
"${VENV_BIN}/ruff" format --check .

echo "=== 2. Checking Static Types (Mypy) ==="
"${VENV_BIN}/mypy" src

echo "=== 3. Executing Fast Test Suite ==="
"${ROOT_DIR}/scripts/test.sh" fast

echo "=== All Quality Checks Passed! ==="
