#!/usr/bin/env bash
# ==============================================================================
# paraDL local installation script (native binary + OPTIONAL Python media worker)
# ==============================================================================
# Builds the Rust reference binary and points `paradl` / `parallel-dl` at that
# single native binary. Python is NOT required for non-media paths. The optional
# media worker is pip-installed into a binary-managed venv (never frozen) and is
# discovered via PARADL_MEDIA_PY; it is invoked as `python -m paradl_media`.
#
# Usage:
#   ./scripts/install-local.sh
#   PARADL_SKIP_MEDIA=1 ./scripts/install-local.sh   # non-media only; no Python/uv
# Environment overrides:
#   PARADL_BIN_DIR     installation directory for the binary (default ~/.local/bin)
#   PARADL_MEDIA_VENV  media venv location (default <workspace>/.venv-media)
#   PARADL_SKIP_MEDIA  set to 1 to skip the media venv entirely
# ==============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
RUST_DIR="${WORKSPACE_DIR}/rust"
BIN_DIR="${PARADL_BIN_DIR:-${HOME}/.local/bin}"
MEDIA_VENV="${PARADL_MEDIA_VENV:-${WORKSPACE_DIR}/.venv-media}"
MEDIA_PYTHON="${MEDIA_VENV}/bin/python"
SKIP_MEDIA="${PARADL_SKIP_MEDIA:-0}"

echo "==> Installing paraDL from workspace: ${WORKSPACE_DIR}"

# The native binary only needs cargo. uv is needed ONLY for the media venv.
command -v cargo >/dev/null 2>&1 || { echo "Error: 'cargo' was not found in PATH." >&2; exit 1; }
if [[ "${SKIP_MEDIA}" != "1" ]]; then
    command -v uv >/dev/null 2>&1 || { echo "Error: 'uv' was not found in PATH (needed for the media venv; set PARADL_SKIP_MEDIA=1 for a non-media install)." >&2; exit 1; }
fi

# PyO3 0.27 rejects Python 3.15; pin 3.12 for any legacy member cargo touches.
if [[ -z "${PYO3_PYTHON:-}" ]]; then
    if command -v uv >/dev/null 2>&1 && UV_PYTHON_312="$(uv python find 3.12 2>/dev/null || true)" && [[ -n "${UV_PYTHON_312}" ]]; then
        export PYO3_PYTHON="${UV_PYTHON_312}"
    elif command -v python3.12 >/dev/null 2>&1; then
        export PYO3_PYTHON="$(command -v python3.12)"
    fi
fi

# ------------------------------------------------------------------------------
# 1. Native binary (owns BOTH `paradl` and `parallel-dl`)
# ------------------------------------------------------------------------------
echo "==> Building native binary (release)"
cargo build --release --manifest-path "${RUST_DIR}/Cargo.toml" --bin paradl

mkdir -p "${BIN_DIR}"
install -m 0755 "${RUST_DIR}/target/release/paradl" "${BIN_DIR}/paradl"
ln -sfn "${BIN_DIR}/paradl" "${BIN_DIR}/parallel-dl"
echo "==> Installed ${BIN_DIR}/paradl and ${BIN_DIR}/parallel-dl (same native binary)"

# Re-point Homebrew symlinks if the directory is present and writable.
if [[ -d "/opt/homebrew/bin" && -w "/opt/homebrew/bin" ]]; then
    ln -sfn "${BIN_DIR}/paradl" "/opt/homebrew/bin/paradl"
    ln -sfn "${BIN_DIR}/paradl" "/opt/homebrew/bin/parallel-dl"
    echo "==> Updated Homebrew symlinks to the native binary"
fi

# ------------------------------------------------------------------------------
# 2/3. OPTIONAL media worker venv (only for media routes)
# ------------------------------------------------------------------------------
if [[ "${SKIP_MEDIA}" == "1" ]]; then
    echo "==> Skipping media venv (PARADL_SKIP_MEDIA=1); non-media routes need no Python"
    echo "==> To enable media later, create a venv with yt-dlp[default]+mutagen+paradl-media and set PARADL_MEDIA_PY"
else
    echo "==> Creating media venv: ${MEDIA_VENV}"
    uv venv --python 3.12 "${MEDIA_VENV}"
    uv pip install --python "${MEDIA_PYTHON}" --upgrade pip
    uv pip install --python "${MEDIA_PYTHON}" "yt-dlp[default]>=2026.6.9" "mutagen>=1.47.0"

    if [[ -f "${WORKSPACE_DIR}/media/pyproject.toml" ]]; then
        uv pip install --python "${MEDIA_PYTHON}" -e "${WORKSPACE_DIR}/media"
    else
        echo "==> Note: media/ package not present; installed runtime deps only" >&2
    fi

    # Optional JavaScript engine support for yt-dlp challenges.
    if ! uv pip install --python "${MEDIA_PYTHON}" yt-dlp-ejs; then
        echo "==> Note: yt-dlp-ejs unavailable; EJS challenges will require a JS runtime" >&2
    fi

    export PARADL_MEDIA_PY="${MEDIA_PYTHON}"
    ENV_FILE="${WORKSPACE_DIR}/.env-paradl-media"
    printf 'export PARADL_MEDIA_PY=%q\n' "${MEDIA_PYTHON}" > "${ENV_FILE}"
    echo "==> PARADL_MEDIA_PY=${MEDIA_PYTHON}"
    echo "==> Add to your shell: source ${ENV_FILE}"
fi

echo "==> Installation complete!"
"${BIN_DIR}/paradl" --version
