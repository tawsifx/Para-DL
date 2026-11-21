# ==============================================================================
# paraDL local installation script for Windows 10/11
# (native binary + OPTIONAL Python media worker)
# ==============================================================================
# Builds the Rust reference binary and points `paradl` / `parallel-dl` at that
# single native binary. Python is NOT required for non-media paths. The optional
# media worker is pip-installed into a binary-managed venv (never frozen) and is
# discovered via PARADL_MEDIA_PY; it is invoked as `python -m paradl_media`.
#
# Usage (PowerShell):
#   .\scripts\install-local.ps1
#   $env:PARADL_SKIP_MEDIA = "1"; .\scripts\install-local.ps1  # non-media only
# Environment overrides:
#   PARADL_BIN_DIR     installation directory (default $env:USERPROFILE\.local\bin)
#   PARADL_MEDIA_VENV  media venv location (default <workspace>\.venv-media)
#   PARADL_SKIP_MEDIA  set to 1 to skip the media venv entirely
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
$SkipMedia = $env:PARADL_SKIP_MEDIA -eq "1"

function Assert-LastExitCode {
    if ($LASTEXITCODE -ne 0) {
        throw "Command failed with exit code $LASTEXITCODE"
    }
}

Write-Host "==> Installing paraDL from workspace: $WorkspaceDir" -ForegroundColor Cyan

# The native binary only needs cargo. uv is needed ONLY for the media venv.
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "'cargo' was not found in PATH. Install it before running this script."
    exit 1
}
if (-not $SkipMedia -and -not (Get-Command uv -ErrorAction SilentlyContinue)) {
    Write-Error "'uv' was not found in PATH (needed for the media venv; set PARADL_SKIP_MEDIA=1 for a non-media install)."
    exit 1
}

# PyO3 0.27 rejects Python 3.15; pin 3.12 for any legacy member cargo touches.
if (-not $env:PYO3_PYTHON) {
    if (Get-Command uv -ErrorAction SilentlyContinue) {
        $py312 = & uv python find 3.12 2>$null
        if ($LASTEXITCODE -eq 0 -and $py312) {
            $env:PYO3_PYTHON = ($py312 | Select-Object -First 1)
        }
    }
}

# ------------------------------------------------------------------------------
# 1. Native binary (owns BOTH `paradl` and `parallel-dl`)
# ------------------------------------------------------------------------------
Write-Host "==> Building native binary (release, media routes enabled)" -ForegroundColor Green
# Media routes are advertised by this script (it creates the worker venv), so
# compile the `media` feature in; without it paradl-app returns
# missing_dependency instead of spawning the worker.
& cargo build --release --features media --manifest-path (Join-Path $RustDir "Cargo.toml") --bin paradl
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
Write-Host "==> Installed native binary in $BinDir (paradl.exe + parallel-dl.exe)" -ForegroundColor Green

# ------------------------------------------------------------------------------
# 2/3. OPTIONAL media worker venv (only for media routes)
# ------------------------------------------------------------------------------
if ($SkipMedia) {
    Write-Host "==> Skipping media venv (PARADL_SKIP_MEDIA=1); non-media routes need no Python" -ForegroundColor Yellow
    Write-Host "==> To enable media later, create a venv with yt-dlp[default]+mutagen+paradl-media and set PARADL_MEDIA_PY" -ForegroundColor Yellow
} else {
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
        Write-Host "==> Note: media/ package not present; installed runtime deps only" -ForegroundColor Yellow
    }

    # Optional JavaScript engine support for yt-dlp challenges.
    & uv pip install --python $MediaPython yt-dlp-ejs
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "yt-dlp-ejs unavailable; EJS challenges will require a JS runtime"
    }

    $env:PARADL_MEDIA_PY = $MediaPython
    [Environment]::SetEnvironmentVariable("PARADL_MEDIA_PY", $MediaPython, "User")
    Write-Host "==> PARADL_MEDIA_PY=$MediaPython" -ForegroundColor Green
}

Write-Host "==> Installation complete!" -ForegroundColor Cyan
& (Join-Path $BinDir "paradl.exe") --version
