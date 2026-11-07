#!/usr/bin/env bash
# ==============================================================================
# pdl (parallel-dl) Local Installation Script
# ==============================================================================
# Installs the modernized `pdl` package from this workspace via `uv tool install`
# and re-points ~/.local/bin/pdl and /opt/homebrew/bin/{pdl,parallel-dl} symlinks
# if writable.
#
# Usage:
#   ./scripts/install-local.sh
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

echo "==> Installing parallel-dl (pdl) from workspace: ${WORKSPACE_DIR}"

if command -v uv >/dev/null 2>&1; then
    uv tool install --force "${WORKSPACE_DIR}"
else
    echo "Error: 'uv' was not found in PATH. Install uv via 'brew install uv'." >&2
    exit 1
fi

UV_BIN_DIR="${HOME}/.local/bin"
INSTALLED_PARADL="${UV_BIN_DIR}/paradl"
INSTALLED_PARALLEL_DL="${UV_BIN_DIR}/parallel-dl"
INSTALLED_PDL="${UV_BIN_DIR}/pdl"

# Re-point /opt/homebrew/bin symlinks if present or writable
if [[ -d "/opt/homebrew/bin" && -w "/opt/homebrew/bin" ]]; then
    if [[ -x "${INSTALLED_PARADL}" ]]; then
        ln -sf "${INSTALLED_PARADL}" "/opt/homebrew/bin/paradl"
        echo "==> Updated symlink: /opt/homebrew/bin/paradl -> ${INSTALLED_PARADL}"
    fi
    if [[ -x "${INSTALLED_PARALLEL_DL}" ]]; then
        ln -sf "${INSTALLED_PARALLEL_DL}" "/opt/homebrew/bin/parallel-dl"
        echo "==> Updated symlink: /opt/homebrew/bin/parallel-dl -> ${INSTALLED_PARALLEL_DL}"
    fi
    if [[ -x "${INSTALLED_PDL}" ]]; then
        ln -sf "${INSTALLED_PDL}" "/opt/homebrew/bin/pdl"
        echo "==> Updated symlink: /opt/homebrew/bin/pdl -> ${INSTALLED_PDL}"
    fi
fi

echo "==> Installation complete!"
"${INSTALLED_PARADL}" --version
