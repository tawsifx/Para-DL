# ==============================================================================
# pdl (parallel-dl) Local Installation Script for Windows 10 & 11
# ==============================================================================
# Installs the modernized `pdl` package from this workspace via `uv tool install`
# or `pipx install`, and verifies PATH configuration.
#
# Usage (PowerShell):
#   .\scripts\install-local.ps1
# ==============================================================================

[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$WorkspaceDir = (Resolve-Path "$ScriptDir\..").Path

Write-Host "==> Installing parallel-dl (paradl) from workspace: $WorkspaceDir" -ForegroundColor Cyan

$Installed = $false

if (Get-Command "uv" -ErrorAction SilentlyContinue) {
    Write-Host "==> Found 'uv', installing tool..." -ForegroundColor Green
    & uv tool install --force "$WorkspaceDir"
    $Installed = $true
} elseif (Get-Command "pipx" -ErrorAction SilentlyContinue) {
    Write-Host "==> Found 'pipx', installing..." -ForegroundColor Green
    & pipx install --force "$WorkspaceDir"
    $Installed = $true
} elseif (Get-Command "pip" -ErrorAction SilentlyContinue) {
    Write-Host "==> Installing via 'pip install -e'..." -ForegroundColor Yellow
    & pip install -e "$WorkspaceDir"
    $Installed = $true
} else {
    Write-Error "Neither 'uv', 'pipx', nor 'pip' was found. Please install uv (e.g. 'winget install astral-sh.uv' or 'scoop install uv')."
    exit 1
}

# Check if paradl is discoverable on PATH
if (Get-Command "paradl" -ErrorAction SilentlyContinue) {
    Write-Host "==> Installation verified on PATH!" -ForegroundColor Green
    & paradl --version
} else {
    Write-Host "`n[Notice] 'paradl' was installed, but its directory is not yet in your PATH." -ForegroundColor Yellow
    $UserLocalBin = "$env:USERPROFILE\.local\bin"
    Write-Host "To add it to your user PATH permanently, run in PowerShell:" -ForegroundColor White
    Write-Host "  `$path = [Environment]::GetEnvironmentVariable('Path', 'User')" -ForegroundColor Cyan
    Write-Host "  [Environment]::SetEnvironmentVariable('Path', `"`$path;$UserLocalBin`", 'User')" -ForegroundColor Cyan
    Write-Host "Then restart your terminal." -ForegroundColor Gray
}
