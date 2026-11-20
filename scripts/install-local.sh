#!/usr/bin/env bash
# ==============================================================================
# paraDL local installation script (native binary + Python media worker)
# ==============================================================================
# Builds the Rust reference binary, installs a binary-managed media venv via uv,
# and points `paradl` / `parallel-dl` at the native binary. The media package is
# pip-installed into the venv (never frozen); non-media paths never spawn Python.
#
# Usage:
#   ./scripts/install-local.sh
# Environment overrides:
#   PARADL_BIN_DIR     installation directory for the binary (default ~/.local/bin)
#   PARADL_MEDIA_VENV  media venv location (default <workspace>/.venv-media)
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUST_DIR="${WORKSPACE_DIR}/rust"
BIN_DIR="${PARADL_BIN_DIR:-${HOME}/.local/bin}"
MEDIA_VENV="${PARADL_MEDIA_VENV:-${WORKSPACE_DIR}/.venv-media}"
MEDIA_PYTHON="${MEDIA_VENV}/bin/python"

echo "==> Installing paraDL from workspace: ${WORKSPACE_DIR}"

for tool in cargo uv; do
    if ! command -v "${tool}" >/dev/null 2>&1; then
        echo "Error: '${tool}' was not found in PATH." >&2
        exit 1
    fi
done

# PyO3 0.27 rejects Python 3.15; pin 3.12 for any legacy member cargo touches.
if [[ -z "${PYO3_PYTHON:-}" ]]; then
    if UV_PYTHON_312="$(uv python find 3.12 2>/dev/null || true)" && [[ -n "${UV_PYTHON_312}" ]]; then
        export PYO3_PYTHON="${UV_PYTHON_312}"
    elif command -v python3.12 >/dev/null 2>&1; then
        export PYO3_PYTHON="$(command -v python3.12)"
    fi
fi

# ------------------------------------------------------------------------------
# 1. Native binary
# ------------------------------------------------------------------------------
echo "==> Building native binary (release)"
cargo build --release --manifest-path "${RUST_DIR}/Cargo.toml" --bin paradl

mkdir -p "${BIN_DIR}"
install -m 0755 "${RUST_DIR}/target/release/paradl" "${BIN_DIR}/paradl"
ln -sfn "${BIN_DIR}/paradl" "${BIN_DIR}/parallel-dl"
echo "==> Installed ${BIN_DIR}/paradl and ${BIN_DIR}/parallel-dl"

# Re-point Homebrew symlinks if the directory is present and writable.
if [[ -d "/opt/homebrew/bin" && -w "/opt/homebrew/bin" ]]; then
    ln -sfn "${BIN_DIR}/paradl" "/opt/homebrew/bin/paradl"
    ln -sfn "${BIN_DIR}/paradl" "/opt/homebrew/bin/parallel-dl"
    echo "==> Updated Homebrew symlinks to the native binary"
fi

# ------------------------------------------------------------------------------
# 2. Media worker venv (pip-installed, never frozen)
# ------------------------------------------------------------------------------
echo "==> Creating media venv: ${MEDIA_VENV}"
uv venv --python 3.12 "${MEDIA_VENV}"
uv pip install --python "${MEDIA_PYTHON}" --upgrade pip
uv pip install --python "${MEDIA_PYTHON}" "yt-dlp[default]>=2026.6.9" "mutagen>=1.47.0"

if [[ -f "${WORKSPACE_DIR}/media/pyproject.toml" ]]; then
    uv pip install --python "${MEDIA_PYTHON}" -e "${WORKSPACE_DIR}/media"
else
    echo "==> Note: media/ package not present yet (arrives in W3-4a); installed deps only"
fi

# Optional JavaScript engine support for yt-dlp challenges.
if ! uv pip install --python "${MEDIA_PYTHON}" yt-dlp-ejs; then
    echo "==> Note: yt-dlp-ejs unavailable; EJS challenges will require a JS runtime" >&2
fi

# ------------------------------------------------------------------------------
# 3. Point the binary at the media interpreter
# ------------------------------------------------------------------------------
export PARADL_MEDIA_PY="${MEDIA_PYTHON}"
ENV_FILE="${WORKSPACE_DIR}/.env-paradl-media"
printf 'export PARADL_MEDIA_PY=%q\n' "${MEDIA_PYTHON}" > "${ENV_FILE}"
echo "==> PARADL_MEDIA_PY=${MEDIA_PYTHON}"
echo "==> Add to your shell: source ${ENV_FILE}"

echo "==> Installation complete!"
"${BIN_DIR}/paradl" --version
