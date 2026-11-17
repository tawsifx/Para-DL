# parallel-dl (`paradl`)

[![Python 3.11+](https://img.shields.io/badge/python-3.11+-blue.svg)](https://www.python.org/downloads/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Platform: Windows & macOS](https://img.shields.io/badge/platform-Windows%2010%2F11%20%7C%20macOS-brightgreen.svg)]()
[![Architecture: x64 & ARM64](https://img.shields.io/badge/arch-x64%20%7C%20ARM64-blue.svg)]()

> Fast, multi-channel download accelerator that bypasses ISP throttling with 16 parallel streams, adaptive watchdog recovery, and cryptographic verification — on Windows and macOS.

`parallel-dl` (invokable as `paradl` or `parallel-dl`) saturates your connection by splitting downloads across concurrent range channels, then automatically resets throttled flows with an adaptive p90 watchdog. Every file is verified with SHA-256 checksums before it lands on disk.

---

## Features

| Feature | What it does |
|---------|--------------|
| **16-Channel Parallel Engine** | Native in-process Rust (`rapids`) engine splits HTTP(S) transfers into up to 16 concurrent range streams. A legacy aria2 backend is available via `--engine aria2`. |
| **Anti-Throttle Watchdog** | Detects ISP shaping and recycles connections in-process (no daemon restart) to restore full speed. On no-Range single-stream servers it stays active for telemetry but does not cycle sockets. |
| **SHA-256 Verification** | Cryptographically validates every download before writing to disk. |
| **Cross-Platform** | Native Windows 10/11 and macOS (Intel & Apple Silicon) support. |
| **Python Packages** | `paradl pip install <pkg>` — resolves and installs wheels with transitive deps via `uv`. |
| **GitHub Releases** | `paradl install <owner/repo>` — downloads and verifies release binaries. |
| **Media & Audio** | YouTube/yt-dlp downloads with ID3 tagging, cover art, and 256kbps AAC extraction. |
| **Hugging Face Models** | `hf:` prefixed downloads for GGUF weights, safetensors, and AI models. |
| **Apple IPSW Firmware** | `ipsw:` prefixed downloads with TSS signature verification. |
| **REST API** | `paradl server` — FastAPI server with task CRUD (mirrors Gopeed's MCP set). |
| **Terminal UI** | `paradl tui` — Textual-based download manager with progress bars and categories. |
| **Download History** | `paradl history` — searchable, filterable SQLite-backed download log. |

---

## Installation

### Option 1: Via `uv` (recommended)

```bash
uv tool install git+https://github.com/tawsifx/Para-DL.git
```

### Option 2: Via `pipx`

```bash
pipx install git+https://github.com/tawsifx/Para-DL.git
```

### Option 3: Via Homebrew (macOS)

```bash
brew tap tawsifx/tap
brew install parallel-dl
```

### Prerequisites

HTTP(S) downloads use the **native Rust `rapids` engine** built into `paradl` — no external download daemon is needed for the default path. [`aria2`](https://aria2.github.io/) is only required if you force `--engine aria2` or download torrents/magnets/metalinks, and [`ffmpeg`](https://ffmpeg.org/) is optional for media extraction:

- **Windows:** `winget install Gyan.FFmpeg` (add `aria2.aria2` only for the legacy engine / torrents)
- **macOS:** `brew install ffmpeg` (add `aria2` only for the legacy engine / torrents)

If the native extension is not built, `paradl` gracefully falls back to aria2 for HTTP(S). Build it with `maturin develop --release` from `rust/paradl-rs` (see the [contributor setup](#contributor-setup) section).

Verify the install:

```bash
paradl --version
paradl doctor
```

---

## Quick Start

### Download a large file

```bash
paradl "https://releases.ubuntu.com/24.04/ubuntu-24.04.1-desktop-amd64.iso" -o ./downloads
```

> **GitHub archive URLs:** `codeload.github.com` ignores `Range`, so a plain `/archive/` URL is a single stream. `paradl` auto-routes these through a Range-capable mirror so they can use all 16 channels, keeping the canonical URL as failover. Override the mirror with `PARADL_GITHUB_ARCHIVE_MIRROR` (default `https://gh-proxy.com/`); set it to an empty string to disable acceleration.
>
> **Other no-Range servers:** `paradl` detects them up-front, downloads in a single stream, and keeps the watchdog active for telemetry only (no socket cycling — those servers reject mid-stream resume). Parallel chunking is impossible on such URLs regardless of downloader.

### Download with custom channel count

```bash
paradl "https://example.com/large-file.zip" -c 16 -o ./downloads
```

### Install a Python package

```bash
paradl pip install requests
paradl pip install fastapi
```

### Install a GitHub release binary

```bash
paradl install BurntSushi/ripgrep
paradl install astral-sh/uv
```

### Download media (YouTube, etc.)

```bash
# Audio track (256kbps AAC, ID3 tagged)
paradl "Song Title Artist Name" -o ./music

# High-res video with subtitles
paradl -v -Q 4k --subs "https://youtube.com/watch?v=..." -o ./videos
```

### Download a Hugging Face model

```bash
paradl "hf:TheBloke/Llama-2-7B-GGUF" --asset "*Q4_K_M.gguf" -o ./models
```

### Download Apple IPSW firmware

```bash
paradl ipsw:iPhone14,5 -o ./downloads
paradl "ipsw:iPhone 16 Pro" -o ./downloads
```

### Import cURL from browser DevTools

```bash
paradl --clipboard -o ./downloads
```

---

## Diagnostics & History

```bash
# Check toolchain, platform utilities, and security status
paradl doctor

# Probe server range support and speed across 1, 4, 16 connections
paradl netprobe "https://example.com/large-file.bin"

# View recent download history
paradl history

# Search history
paradl history --search ubuntu --limit 20

# Re-download a previous entry by ID
paradl resume --from-history 42 -o ~/Downloads
```

---

## REST API Server

Start a local API server for programmatic control:

```bash
pip install "parallel-dl[api]"
paradl server
```

Default: `http://127.0.0.1:8000`

Endpoints:

| Method | Endpoint | Description |
|--------|----------|-------------|
| `POST` | `/resolve` | Classify a target without downloading |
| `POST` | `/tasks` | Create and queue a download task |
| `GET` | `/tasks` | List all tasks with status & progress |
| `GET` | `/tasks/{id}` | Get a single task's details |
| `POST` | `/tasks/{id}/pause` | Pause a task |
| `POST` | `/tasks/{id}/resume` | Resume a paused/failed task |
| `DELETE` | `/tasks/{id}` | Cancel and remove a task |

Example:

```bash
curl -X POST http://127.0.0.1:8000/tasks \
  -H "Content-Type: application/json" \
  -d '{"target": "https://releases.ubuntu.com/24.04/ubuntu-24.04.1-desktop-amd64.iso", "connections": 16}'
```

---

## Terminal UI

A visual download manager inside your terminal:

```bash
pip install "parallel-dl[tui]"
paradl tui
```

Features: task list with progress bars, pause/resume/delete, categories, search, and a persistent SQLite queue.

---

## Engine selection

`paradl` picks a transfer backend per target:

- **HTTP(S)** uses the native Rust `rapids` engine by default (`--engine rust`).
- `--engine aria2` selects the legacy JSON-RPC daemon backend.
- **Torrents, magnets, and metalinks** are always delegated to aria2, regardless of selection.

If the compiled extension is unavailable, HTTP(S) transfers automatically fall back to aria2. Check which engine is active with `paradl doctor`.

---

## Configuration

| Environment Variable | Description | Default |
|----------------------|-------------|---------|
| `PARADL_HOME` | Config & data directory | `~/.config/paradl` (macOS/Linux) <br> `%LOCALAPPDATA%\paradl` (Windows) |
| `PARADL_GITHUB_ARCHIVE_MIRROR` | Range-capable mirror prefix for GitHub `/archive/` URLs (empty disables acceleration) | `https://gh-proxy.com/` |

---

## Contributor setup

To build the native `rapids` engine from source (Rust 1.98+ and `maturin` required):

```bash
cd rust/paradl-rs
maturin develop --release
cp python/paradl/_rust_engine*.so ../../src/paradl/
cd ../..
python -m pytest -q
```

The workspace lives under `rust/` (`rapids` core + `paradl-rs` PyO3 bindings). Run `cargo test --all` and `cargo clippy --all -- -D warnings` from `rust/` to verify the Rust side.

---

## CLI Entry Points

Both commands invoke the same application:

- `paradl` — primary command
- `parallel-dl` — full package name

---

## License

MIT License. See [LICENSE](LICENSE) for details.