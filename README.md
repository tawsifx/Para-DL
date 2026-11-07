# `parallel-dl` (`paradl`) v1.0.0

[![Python 3.11+](https://img.shields.io/badge/python-3.11+-blue.svg)](https://www.python.org/downloads/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Platform: macOS](https://img.shields.io/badge/platform-macOS%20ARM64-lightgrey.svg)](https://apple.com)

> **High-Performance Multi-Channel Download Accelerator with Anti-Throttle Watchdog for macOS (Apple Silicon ARM64).**

`parallel-dl` (invoked as `paradl` or `parallel-dl`) bypasses single-stream ISP bandwidth throttling by orchestrating **16 parallel range channels** via an in-process Aria2 JSON-RPC daemon, backed by an adaptive rolling-p90 watchdog and verified cryptographic checksums.

---

## Key Features

* **16-Channel Parallel Engine**: Saturates your connection with up to 16 concurrent HTTP range streams per transfer.
* **Anti-Throttle Watchdog v2**: Dynamically monitors transfer throughput. If ISP policing or token-bucket shaping throttles the flow below the adaptive threshold (`max(user_floor, 0.5 * rolling_p90)`), the watchdog cycles RPC connections in ~150ms to instantly reset ISP flow meters.
* **Homebrew Formula & Bottle Accelerator (`paradl brew`)**:
  * Pre-downloads Homebrew bottles directly into `~/Library/Caches/Homebrew/downloads/` with 16 parallel channels.
  * **Transitive Dependency Pre-Flight**: Resolves uninstalled formula dependencies and pre-fetches all missing dependency bottles before pouring.
  * Pours locally without network delays (`NONINTERACTIVE=1 HOMEBREW_NO_AUTO_UPDATE=1 brew install`).
  * Supports `paradl brew install <formula>`, `paradl brew update`, and `paradl brew upgrade <formula>`.
* **Python / PyPI Wheelhouse Engine (`paradl pip`)**:
  * Resolves packages via PyPI JSON API and scores binary wheels for macOS ARM64 (`cp312-macosx_...`).
  * **Transitive Wheelhouse Pre-Flight**: Resolves complete dependency trees via `uv pip compile` (~15ms) and pre-downloads missing wheels across 16 parallel channels into a temporary wheelhouse.
  * Installs offline via `uv pip install --find-links <wheelhouse>` into the active environment (respecting `VIRTUAL_ENV` or custom `--python`).
* **Media & Audio Mastering**:
  * In-process single-pass `yt-dlp` resolution with pinned `yt-dlp-ejs` JavaScript challenge solving (`deno`, `node >= 22`, `quickjs`).
  * Protocol-segregated downloaders (`dash: native`, `m3u8: native`) complying with CVE-2026-50574 and multi-threaded native fragment downloading (`concurrent_fragment_downloads = 16`).
  * CoreAudio hardware AAC/ALAC packaging and universal ID3v2.3 cover art and metadata tagging (`mutagen`).
* **GitHub PowerSuite & Zero-Setup Binary Installer (`paradl install`)**:
  * Downloads release binaries scored for macOS ARM64 and verifies cryptographic SHA-256 digests before installation into `~/.local/bin`.
* **Hugging Face Hub Engine (`hf:`)**:
  * 16-channel downloads for AI models, GGUF weights, and safetensors with Bearer authentication and `X-Linked-ETag` validation.
* **Apple IPSW Firmware Engine (`ipsw:`)**:
  * Resolves device names (`ipsw:iPhone 16 Pro`) to Apple CDN URLs and verifies TSS signatures.
* **"Copy as cURL" Import Engine (`--from-curl`, `--clipboard`)**:
  * Safely parses DevTools cURL commands and creates host-scoped temporary cookie jars for authenticated downloads.
* **macOS Power & Sleep Resilience**:
  * Automatically holds `caffeinate` assertions during active transfers.
  * Monitors network changes (`scutil`) and recovers dropped connections.

---

## Installation & Setup

### Option 1: Via Homebrew (Recommended for macOS)

```bash
brew tap tawsifx/tap
brew install parallel-dl
```

### Option 2: Via `uv tool` or `pipx`

```bash
uv tool install git+https://github.com/tawsifx/pdl.git
# or with pipx:
pipx install git+https://github.com/tawsifx/pdl.git
```

### Option 3: Local Development Setup

```bash
git clone https://github.com/tawsifx/pdl.git ~/pdl
cd ~/pdl
uv tool install --editable .
```

Verify the installation:
```bash
paradl --version
paradl doctor
```

---

## Quick Reference & Recipes

### 1. Homebrew Formulas & Bottles
```bash
# Accelerate formula install with 16 parallel channels (including dependencies)
paradl brew install wget
paradl brew install ffmpeg

# Upgrade an existing formula with 16 parallel streams
paradl brew upgrade ripgrep

# Download bottle archive (.tar.gz) directly without installing
paradl brew:tree -o ~/Downloads

# Update Homebrew formulae catalog non-interactively
paradl brew update
```

### 2. Python / PyPI Packages
```bash
# Install Python package and all transitive dependencies with 16 parallel channels
paradl pip install requests
paradl pip install fastapi

# Download package wheel (.whl) directly with SHA-256 validation
paradl pip:torch -o ./wheels
paradl pypi:numpy -o ./wheels
```

### 3. Media & Audio Extraction
```bash
# Download track by search query or URL (CoreAudio mastered AAC/ALAC)
paradl "Song Title Artist Name" -o ~/Music
paradl "https://www.youtube.com/watch?v=..." -o ~/Music

# High-resolution video (4K or 1080p) with subtitles
paradl -v -Q 4k --subs "https://www.youtube.com/watch?v=..." -o ~/Videos
```

### 4. GitHub Releases & Tool Installation
```bash
# Install tool directly into ~/.local/bin (verified SHA-256)
paradl install BurntSushi/ripgrep
paradl install astral-sh/uv

# Download release assets scored for macOS Apple Silicon
paradl gh:sharkdp/fd -o ~/Downloads
```

### 5. Hugging Face Models (`hf:`) & Apple IPSW (`ipsw:`)
```bash
# Download GGUF quantized models with 16 parallel streams
paradl "hf:TheBloke/Llama-2-7B-GGUF" --asset "*Q4_K_M.gguf" -o ./models

# Download iOS restore firmware by device model
paradl ipsw:iPhone14,5 -o ~/Downloads
paradl "ipsw:iPhone 15 Pro" -o ~/Downloads
```

### 6. General Large Files, Cloud Drives & cURL
```bash
# Large ISO or archive with auto-extraction (-x)
paradl "https://releases.ubuntu.com/24.04/ubuntu-24.04.1-desktop-amd64.iso" -c 16 -o ~/Downloads

# Paste cURL from browser DevTools directly from clipboard
paradl --clipboard -o ~/Downloads
```

### 7. Diagnostics & Environment Check
```bash
# Check toolchain, versions, and security status
paradl doctor

# Probe server range support and speed limits across 1, 4, 16 connections
paradl netprobe "https://example.com/large-file.bin"

# Benchmark Watchdog v1 vs v2 vs off
paradl bench "https://example.com/large-file.bin" --duration 60
```

---

## Architecture & Design

`parallel-dl` is structured into decoupled modules inside `src/pdl`:
* `pdl.engines`: Specialized protocol accelerators (`Aria2Engine`, `MediaEngine`, `BrewEngine`, `PyPIEngine`, `GitHubEngine`, `HFEngine`, `IPSWEngine`, `CloudEngine`).
* `pdl.throttle`: Network probes, throughput sampling, burst-series analysis, and adaptive socket recycle watchdog state machine.
* `pdl.doctor`: Environment diagnostic, security posture, and toolchain validator.
* `pdl.auth`: Secure token management backed by the macOS Keychain Services API.
* `pdl.ui`: Rich live multi-progress rendering, diagnostic tables, and ANSI spinners.

## License

MIT License. See [LICENSE](LICENSE) for details.
