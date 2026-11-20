# ==============================================================================
# paraDL local installation script for Windows 10/11
# (native binary + Python media worker)
# ==============================================================================
# Builds the Rust reference binary, installs a binary-managed media venv via uv,
# and points `paradl` / `parallel-dl` at the native binary. The media package is
# pip-installed into the venv (never frozen); non-media paths never spawn Python.
#
# Usage (PowerShell):
#   .\scripts\install-local.ps1
# Environment overrides:
#   PARADL_BIN_DIR     installation directory (default $env:USERPROFILE\.local\bin)
#   PARADL_MEDIA_VENV  media venv location (default <workspace>\.venv-media)
# ==============================================================================

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$WorkspaceDir = (Resolve-Path "$ScriptDir\..").Path
$RustDir = Join-Path $WorkspaceDir "rust"
$BinDir = if ($env:PARADL_BIN_DIR) { $env:PARADL_BIN_DIR } else { Join-Path $env:USERPROFILE ".local\bin" }
$MediaVenv = if ($env:PARADL_MEDIA_VENV) { $env:PARADL_MEDIA_VENV } else { Join-Path $WorkspaceDir ".venv-media" }
$MediaPython = Join-Path $MediaVenv "Scripts\python.exe"

function Assert-LastExitCode {
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed with exit code $LASTEXITCODE"
    }
}

Write-Host "==> Installing paraDL from workspace: $WorkspaceDir" -ForegroundColor Cyan

foreach ($tool in @("cargo", "uv")) {
    if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) {
        Write-Error "'$tool' was not found in PATH. Install it before running this script."
        exit 1
    }
}

# PyO3 0.27 rejects Python 3.15; pin 3.12 for any legacy member cargo touches.
if (-not $env:PYO3_PYTHON) {
    $py312 = & uv python find 3.12 2>$null
    if ($LASTEXITCODE -eq 0 -and $py312) {
        $env:PYO3_PYTHON = ($py312 | Select-Object -First 1)
    }
}

# ------------------------------------------------------------------------------
# 1. Native binary
# ------------------------------------------------------------------------------
Write-Host "==> Building native binary (release)" -ForegroundColor Green
& cargo build --release --manifest-path (Join-Path $RustDir "Cargo.toml") --bin paradl
Assert-LastExitCode

New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
$SourceBinary = Join-Path $RustDir "target\release\paradl.exe"
Copy-Item $SourceBinary (Join-Path $BinDir "paradl.exe") -Force
# `paradl.exe` is the real copied file; only the alias is a symlink.
# Never symlink paradl.exe onto itself.
foreach ($linkName in @("parallel-dl")) {
    $LinkPath = Join-Path $BinDir "$linkName.exe"
    try {
        New-Item -ItemType SymbolicLink -Path $LinkPath -Target (Join-Path $BinDir "paradl.exe") -Force | Out-Null
    } catch {
        # Symlinks can require developer mode / elevation; a copy works too.
        Copy-Item (Join-Path $BinDir "paradl.exe") $LinkPath -Force
    }
}
Write-Host "==> Installed native binary in $BinDir" -ForegroundColor Green

# ------------------------------------------------------------------------------
# 2. Media worker venv (pip-installed, never frozen)
# ------------------------------------------------------------------------------
Write-Host "==> Creating media venv: $MediaVenv" -ForegroundColor Green
& uv venv --python 3.12 $MediaVenv
Assert-LastExitCode
& uv pip install --python $MediaPython --upgrade pip
Assert-LastExitCode
& uv pip install --python $MediaPython "yt-dlp[default]>=2026.6.9" "mutagen>=1.47.0"
Assert-LastExitCode

$MediaProject = Join-Path $WorkspaceDir "media\pyproject.toml"
if (Test-Path $MediaProject) {
    & uv pip install --python $MediaPython -e (Join-Path $WorkspaceDir "media")
    Assert-LastExitCode
} else {
    Write-Host "==> Note: media/ package not present yet (arrives in W3-4a); installed deps only" -ForegroundColor Yellow
}

# Optional JavaScript engine support for yt-dlp challenges.
& uv pip install --python $MediaPython yt-dlp-ejs
if ($LASTEXITCODE -ne 0) {
    Write-Warning "yt-dlp-ejs unavailable; EJS challenges will require a JS runtime"
}

# ------------------------------------------------------------------------------
# 3. Point the binary at the media interpreter
# ------------------------------------------------------------------------------
$env:PARADL_MEDIA_PY = $MediaPython
[Environment]::SetEnvironmentVariable("PARADL_MEDIA_PY", $MediaPython, "User")
Write-Host "==> PARADL_MEDIA_PY=$MediaPython" -ForegroundColor Green

Write-Host "==> Installation complete!" -ForegroundColor Cyan
& (Join-Path $BinDir "paradl.exe") --version
