# parallel-dl (`paradl`)

[![Rust 1.98+](https://img.shields.io/badge/rust-1.98+-orange.svg)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Platform: Windows & macOS](https://img.shields.io/badge/platform-Windows%2010%2F11%20%7C%20macOS-brightgreen.svg)]()
[![Architecture: x64 & ARM64](https://img.shields.io/badge/arch-x64%20%7C%20ARM64-blue.svg)]()

> Fast, multi-channel download accelerator that bypasses ISP throttling with 16 parallel streams, adaptive watchdog recovery, and cryptographic verification — on Windows and macOS.

`parallel-dl` (invokable as `paradl` or `parallel-dl`) is a **single native Rust binary**. It saturates your connection by splitting downloads across concurrent range channels, then automatically resets throttled flows with an adaptive p90 watchdog. Every file is verified with SHA-256 checksums before it lands on disk.

**Python is not required for normal downloads.** The only Python component is an optional media worker (`yt-dlp`-based audio/video extraction) that the binary talks to over the versioned `paradl-media/1` protocol; media routes need Python 3.11+ and are enabled separately via a private virtual environment.

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
| **REST API** | `paradl server` — built-in Rust API server (opt-in `api` feature) with task CRUD (mirrors Gopeed's MCP set). |
| **Terminal UI** | `paradl tui` — built-in Rust terminal download manager (opt-in `tui` feature) with progress bars and categories. |
| **Download History** | `paradl history` — searchable, filterable SQLite-backed download log. |

---

## Installation

`paradl` and `parallel-dl` are **the same native binary** — there is no Python
package, console script, or `pip`/`uv tool` install for the application itself.

### Option 1: Via Homebrew (macOS)

```bash
brew tap tawsifx/tap
brew install parallel-dl
```

This builds the Rust binary into `bin/` and installs an **optional** media venv
under `libexec/`. Add `--without-python@3.12` for a media-free install; every
non-media route still works.

### Option 2: Local install script (macOS / Linux / Windows)

```bash
# macOS / Linux — builds the binary, plus an optional media venv
./scripts/install-local.sh

# Media-free install: no Python or uv needed
PARADL_SKIP_MEDIA=1 ./scripts/install-local.sh

# Windows (PowerShell):
#   .\scripts\install-local.ps1
#   $env:PARADL_SKIP_MEDIA = "1"; .\scripts\install-local.ps1
```

Both scripts install `paradl` and the `parallel-dl` alias into
`PARADL_BIN_DIR` (default `~/.local/bin`, or `%USERPROFILE%\.local\bin` on
Windows).

### Option 3: Build from source

Requires the Rust toolchain (1.98+):

```bash
cd rust
cargo build --release --bin paradl
# binary: rust/target/release/paradl
```

### Prerequisites

HTTP(S) downloads use the native Rust engine built into the binary — no Python
and no external download daemon are needed. [`aria2`](https://aria2.github.io/)
is only required if you force `--engine aria2` or download torrents/magnets/
metalinks. [`ffmpeg`](https://ffmpeg.org/) is optional and only used by media
routes:

- **Windows:** `winget install Gyan.FFmpeg` (add `aria2.aria2` only for the legacy engine / torrents)
- **macOS:** `brew install ffmpeg` (add `aria2` only for the legacy engine / torrents)

Verify the install (works with **no Python installed or configured**):

```bash
paradl --version
paradl doctor
```

### Optional: enable media routes (YouTube, audio extraction)

Media extraction is the one remaining Python component. It is a one-shot worker
invoked as `python -m paradl_media`, installed into its own venv so it never
pollutes system Python. Run the install scripts above (which create this venv),
or set it up manually:

```bash
python3.12 -m venv ~/.local/share/paradl/media-venv
VENV=~/.local/share/paradl/media-venv
"$VENV/bin/python" -m pip install -U pip
"$VENV/bin/python" -m pip install "yt-dlp[default]" mutagen
# From a source checkout, also install the retained media package:
"$VENV/bin/python" -m pip install ./media
export PARADL_MEDIA_PY="$VENV/bin/python"   # persist this in your shell profile
```

`paradl doctor` reports the media package, protocol, and `yt-dlp` versions.
Without `PARADL_MEDIA_PY`, media routes report the dependency as missing while
all other routes keep working.

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

Start the built-in local API server for programmatic control. It is an opt-in
Rust feature — build with `cargo build --release --features api` (or use a
release that enables it):

```bash
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

A visual download manager inside your terminal (built-in Rust `tui` feature;
build with `cargo build --release --features tui`):

```bash
paradl tui
```

Features: task list with progress bars, pause/resume/delete, categories, search, and a persistent SQLite queue.

---

## Engine selection

`paradl` picks a transfer backend per target:

- **HTTP(S)** uses the native Rust `rapids` engine by default (`--engine rust`).
- `--engine aria2` selects the legacy JSON-RPC daemon backend.
- **Torrents, magnets, and metalinks** are always delegated to aria2, regardless of selection.

The native engine is compiled into the binary; `paradl doctor` reports engine
status. Python is never spawned on HTTP(S), GitHub, Hugging Face, IPSW, PyPI,
Brew, or other non-media routes.

---

## Configuration

| Environment Variable | Description | Default |
|----------------------|-------------|---------|
| `PARADL_HOME` | Config & data directory | `~/.config/paradl` (macOS/Linux) <br> `%LOCALAPPDATA%\paradl` (Windows) |
| `PARADL_GITHUB_ARCHIVE_MIRROR` | Range-capable mirror prefix for GitHub `/archive/` URLs (empty disables acceleration) | `https://gh-proxy.com/` |
| `PARADL_MEDIA_PY` | Interpreter for the **optional** media worker venv; only consulted on media routes | auto-discovered from the install's `libexec/` venv |

---

## Contributor setup

The framework is a Cargo workspace under `rust/` (public `paradl-*` crates plus
the legacy `rapids` core / `paradl-rs` PyO3 member). A Python 3.12 interpreter is
only needed so the legacy PyO3 member can resolve; the new framework crates do
not depend on it:

```bash
cd rust
PYO3_PYTHON="$(pwd)/../.venv/bin/python" cargo build --all
PYO3_PYTHON="$(pwd)/../.venv/bin/python" cargo test --all
PYO3_PYTHON="$(pwd)/../.venv/bin/python" cargo clippy --all -- -D warnings
```

Media worker tests live in the retained `media/` package:

```bash
PYTHONPATH=media .venv/bin/python -m pytest media/tests -q
```

The root `pyproject.toml` is a legacy tooling stub only; neither the Python
application (`src/paradl/`) nor the bulk Python test suite exists anymore.

---

## CLI Entry Points

Both commands are the **same native Rust binary** — there are no Python console
scripts and no `parallel-dl.py` shim:

- `paradl` — primary command
- `parallel-dl` — full-name alias (symlink to the same binary)

`paradl --version` prints `parallel-dl (paradl) v1.0.0` for either name.

---

## License

MIT License. See [LICENSE](LICENSE) for details.