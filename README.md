# `parallel-dl` (`paradl`) v1.0.0

[![Python 3.11+](https://img.shields.io/badge/python-3.11+-blue.svg)](https://www.python.org/downloads/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Platform: Windows & macOS](https://img.shields.io/badge/platform-Windows%2010%2F11%20%7C%20macOS-brightgreen.svg)]()
[![Architecture: x64 & ARM64](https://img.shields.io/badge/arch-x64%20%7C%20ARM64-blue.svg)]()

> **High-Performance Multi-Channel Download Accelerator with Anti-Throttle Watchdog for Windows 10/11 & macOS (x64 and ARM64).**

`parallel-dl` (invoked as `paradl` or `parallel-dl`) bypasses single-stream ISP bandwidth throttling by orchestrating **16 parallel range channels** via an in-process Aria2 JSON-RPC daemon, backed by an adaptive rolling-p90 watchdog and verified cryptographic checksums.

---

## Key Features

* **16-Channel Parallel Engine**: Saturates your bandwidth connection with up to 16 concurrent HTTP range streams per transfer.
* **Anti-Throttle Watchdog v2**: Dynamically monitors transfer throughput. If ISP traffic policing or token-bucket shaping throttles the flow below the adaptive threshold (`max(user_floor, 0.5 * rolling_p90)`), the watchdog cycles RPC connections in ~150ms to instantly reset ISP flow meters.
* **Dual-Platform Native Architecture**:
  * **Windows 10 & 11**:
    * **Sleep Prevention**: Holds `SetThreadExecutionState` system assertions (`ES_CONTINUOUS | ES_SYSTEM_REQUIRED`) via `kernel32.dll` to prevent idle sleep during active transfers.
    * **Power Policy**: Monitors AC power status, battery percentage, and Battery Saver (Low Power Mode) via `GetSystemPowerStatus`.
    * **Mark of the Web & Zone Transfer**: Records download source URLs in NTFS Alternate Data Streams (`:Zone.Identifier` `[ZoneTransfer] HostUrl=...`) and automatically unblocks verified binaries.
    * **Token Security**: Backed by Windows Credential Manager (`wincred` / `cmdkey`) with DPAPI encrypted user store fallback.
    * **Native Notifications**: PowerShell-driven Windows 10/11 native toast notifications.
  * **macOS (Apple Silicon & Intel)**:
    * Holds `caffeinate` assertions, queries `scutil` / AirPort Wi-Fi identity, monitors battery via `pmset`, manages tokens via Apple Keychain Services (`security`), and sets `com.apple.metadata:kMDItemWhereFroms`.
* **Python / PyPI Wheelhouse Engine (`paradl pip`)**:
  * Resolves packages via PyPI JSON API and scores binary wheels for **Windows** (`win_amd64`, `win_arm64`, `win32`) and **macOS** (`cp312-macosx_...`).
  * **Transitive Wheelhouse Pre-Flight**: Resolves complete dependency trees via `uv pip compile` (~15ms) and pre-downloads missing wheels across 16 parallel channels into a temporary wheelhouse.
  * Installs offline via `uv pip install --find-links <wheelhouse>` into the active environment (respecting `VIRTUAL_ENV` or custom `--python`).
* **GitHub PowerSuite & Zero-Setup Binary Installer (`paradl install`)**:
  * Downloads release binaries scored for Windows (`.zip`, `.exe`, MSVC) or macOS (ARM64/Universal2) and verifies cryptographic SHA-256 digests before installation into user binary directories (`%LOCALAPPDATA%\Programs\pdl\bin` or `~/.local/bin`).
  * On Windows, audits Authenticode trust signatures via `Get-AuthenticodeSignature`.
* **Media & Audio Mastering**:
  * In-process single-pass `yt-dlp` resolution with pinned `yt-dlp-ejs` JavaScript challenge solving (`deno`, `node >= 22`, `quickjs`).
  * Protocol-segregated downloaders (`dash: native`, `m3u8: native`) complying with CVE-2026-50574 and multi-threaded native fragment downloading (`concurrent_fragment_downloads = 16`).
  * FFmpeg / Apple CoreAudio packaging and universal ID3v2.3 cover art and metadata tagging (`mutagen`).
* **Hugging Face Hub Engine (`hf:`)**:
  * 16-channel downloads for AI models, GGUF weights, and safetensors with Bearer authentication and `X-Linked-ETag` validation.
* **Homebrew Formula & Bottle Accelerator (`paradl brew`)** (macOS/Linux):
  * Pre-downloads Homebrew bottles directly into `~/Library/Caches/Homebrew/downloads/` with 16 parallel channels before pouring locally.
* **Apple IPSW Firmware Engine (`ipsw:`)**:
  * Resolves device names (`ipsw:iPhone 16 Pro`) to Apple CDN URLs and verifies TSS signatures.
* **"Copy as cURL" Import Engine (`--from-curl`, `--clipboard`)**:
  * Safely parses DevTools cURL commands and creates host-scoped temporary cookie jars for authenticated downloads.

---

## Installation & Setup

### Prerequisites

`parallel-dl` uses `aria2c` for 16-channel segmented transport and optionally `ffmpeg` for media extraction:

* **Windows 10 / 11** (via Winget, Scoop, or Chocolatey):
  ```powershell
  # Using winget (built into Windows 10 & 11)
  winget install aria2.aria2 Gyan.FFmpeg astral-sh.uv

  # Or using scoop
  scoop install aria2 ffmpeg uv

  # Or using chocolatey
  choco install aria2 ffmpeg uv
  ```

* **macOS** (via Homebrew):
  ```bash
  brew install aria2 ffmpeg uv
  ```

---

### Option 1: Via `uv tool` or `pipx` (Recommended for Windows & macOS)

With [uv](https://github.com/astral-sh/uv) (ultra-fast):
```bash
uv tool install git+https://github.com/tawsifx/Para-DL.git
```

Or with `pipx`:
```bash
pipx install git+https://github.com/tawsifx/Para-DL.git
```

---

### Option 2: Via Homebrew (macOS)

```bash
brew tap tawsifx/tap
brew install parallel-dl
```

---

### Option 3: Local Development Setup

* **On Windows (PowerShell)**:
  ```powershell
  git clone https://github.com/tawsifx/Para-DL.git $env:USERPROFILE\pdl
  cd $env:USERPROFILE\pdl
  .\scripts\install-local.ps1
  ```

* **On macOS / Linux (Bash/Zsh)**:
  ```bash
  git clone https://github.com/tawsifx/Para-DL.git ~/pdl
  cd ~/pdl
  ./scripts/install-local.sh
  ```

Verify the installation:
```bash
paradl --version
paradl doctor
```

---

## Quick Reference & Recipes

### 1. Python / PyPI Packages
```bash
# Install Python package and all transitive dependencies with 16 parallel channels
paradl pip install requests
paradl pip install fastapi

# Download package wheel (.whl) directly with SHA-256 validation (scored for your OS)
paradl pip:torch -o ./wheels
paradl pypi:numpy -o ./wheels
```

### 2. GitHub Releases & Tool Installation
```bash
# Install tool directly into your user bin directory (verified SHA-256)
paradl install BurntSushi/ripgrep
paradl install astral-sh/uv

# Download release assets scored for your host (Windows x64/MSVC or macOS ARM64)
paradl gh:sharkdp/fd -o ./downloads
```

### 3. Media & Audio Extraction
```bash
# Download track by search query or URL (ID3 tagged, 256kbps audio)
paradl "Song Title Artist Name" -o ./music
paradl "https://www.youtube.com/watch?v=..." -o ./music

# High-resolution video (4K or 1080p) with subtitles
paradl -v -Q 4k --subs "https://www.youtube.com/watch?v=..." -o ./videos
```

### 4. Hugging Face Models (`hf:`) & Apple IPSW (`ipsw:`)
```bash
# Download GGUF quantized models with 16 parallel streams
paradl "hf:TheBloke/Llama-2-7B-GGUF" --asset "*Q4_K_M.gguf" -o ./models

# Download iOS restore firmware by device model
paradl ipsw:iPhone14,5 -o ./downloads
paradl "ipsw:iPhone 16 Pro" -o ./downloads
```

### 5. General Large Files, Cloud Drives & cURL
```bash
# Large ISO or archive with 16 concurrent range streams and auto-extraction (-x)
paradl "https://releases.ubuntu.com/24.04/ubuntu-24.04.1-desktop-amd64.iso" -c 16 -o ./downloads

# Paste cURL from browser DevTools directly from clipboard
paradl --clipboard -o ./downloads
```

### 6. Diagnostics & System Doctor
```bash
# Check toolchain, versions, platform utilities, and security status
paradl doctor

# Probe server range support and speed limits across 1, 4, 16 connections
paradl netprobe "https://example.com/large-file.bin"

# Benchmark Watchdog v2 vs v1 vs off
paradl bench "https://example.com/large-file.bin" --duration 60
```

---

## Architecture & Design

`parallel-dl` is structured into clean, decoupled modules inside `src/pdl`:
* `pdl.platform`: Unified cross-platform subsystem dynamically dispatching between `pdl.platform.windows` (kernel32, SYSTEM_POWER_STATUS, Zone.Identifier ADS, WinCred) and `pdl.platform.macos` (caffeinate, scutil, pmset, Keychain, WhereFroms).
* `pdl.engines`: Specialized protocol accelerators (`Aria2Engine`, `MediaEngine`, `PyPIEngine`, `GitHubEngine`, `HFEngine`, `BrewEngine`, `IPSWEngine`, `CloudEngine`).
* `pdl.throttle`: Network probes, throughput sampling, burst-series analysis, and adaptive socket recycle watchdog state machine.
* `pdl.doctor`: Environment diagnostic, security posture, and toolchain validator for both Windows and macOS.
* `pdl.auth`: Secure token management backed by Windows Credential Manager or macOS Keychain Services API.
* `pdl.ui`: Rich live multi-progress rendering, diagnostic tables, and ANSI spinners.

---

## License

MIT License. See [LICENSE](LICENSE) for details.
