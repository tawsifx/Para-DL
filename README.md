# parallel-dl (`paradl`)

[![Python 3.11+](https://img.shields.io/badge/python-3.11+-blue.svg)](https://www.python.org/downloads/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Platform: Windows & macOS](https://img.shields.io/badge/platform-Windows%2010%2F11%20%7C%20macOS-brightgreen.svg)]()
[![Architecture: x64 & ARM64](https://img.shields.io/badge/arch-x64%20%7C%20ARM64-blue.svg)]()

> Fast, multi-channel download accelerator that bypasses ISP throttling with 16 parallel streams, adaptive watchdog recovery, and cryptographic verification — on Windows and macOS.

`parallel-dl` (invokable as `paradl`, `pdl`, or `parallel-dl`) saturates your connection by splitting downloads across concurrent range channels, then automatically resets throttled flows with an adaptive p90 watchdog. Every file is verified with SHA-256 checksums before it lands on disk.

---

## Features

| Feature | What it does |
|---------|--------------|
| **16-Channel Parallel Engine** | Splits transfers into concurrent HTTP range streams to maximize bandwidth. |
| **Anti-Throttle Watchdog** | Detects ISP shaping and recycles connections in ~150ms to restore full speed. |
| **SHA-256 Verification** | Cryptographically validates every download before writing to disk. |
| **Cross-Platform** | Native Windows 10/11 and macOS (Intel & Apple Silicon) support. |
| **Python Packages** | `paradl pip install <pkg>` — resolves and installs wheels with transitive deps via `uv`. |
| **GitHub Releases** | `paradl install <owner/repo>` — downloads and verifies release binaries. |
| **Media & Audio** | YouTube/yt-dlp downloads with ID3 tagging, cover art, and 256kbps AAC extraction. |
| **Hugging Face Models** | `hf:` prefixed downloads for GGUF weights, safetensors, and AI models. |
| **Apple IPSW Firmware** | `ipsw:` prefixed downloads with TSS signature verification. |
| **REST API** | `pdl server` — FastAPI server with task CRUD (mirrors Gopeed's MCP set). |
| **Terminal UI** | `pdl tui` — Textual-based download manager with progress bars and categories. |
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

`paradl` uses [`aria2`](https://aria2.github.io/) for segmented transport and optionally [`ffmpeg`](https://ffmpeg.org/) for media extraction:

- **Windows:** `winget install aria2.aria2 Gyan.FFmpeg` (or scoop / chocolatey)
- **macOS:** `brew install aria2 ffmpeg`

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
pdl server
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
pdl tui
```

Features: task list with progress bars, pause/resume/delete, categories, search, and a persistent SQLite queue.

---

## Configuration

| Environment Variable | Description | Default |
|----------------------|-------------|---------|
| `PARADL_HOME` | Config & data directory | `~/.config/paradl` (macOS/Linux) <br> `%LOCALAPPDATA%\paradl` (Windows) |

---

## CLI Entry Points

All three commands invoke the same application:

- `paradl` — primary command
- `pdl` — short alias
- `parallel-dl` — full package name

---

## License

MIT License. See [LICENSE](LICENSE) for details.