"""Argument parser construction for the pdl CLI.

Builds the argparse.ArgumentParser with all pdl flags, recipes, and epilog examples.
"""

from __future__ import annotations

import argparse
import logging
import sys

from paradl import VERSION
from paradl.core.config import PDLConfig
from paradl.ui import SecretRedactingFilter

SUBCOMMANDS = {
    "auth",
    "doctor",
    "install",
    "info",
    "hash",
    "extract",
    "history",
    "clean",
    "config",
    "diagnose",
    "netprobe",
    "bench",
    "profiles",
    "queue",
    "resume",
    "server",
    "tui",
    "throttle",
    "pip",
    "pypi",
    "brew",
}


def configure_logging(
    *, verbose: bool = False, debug: bool = False, json_mode: bool = False
) -> None:
    """Configure root logger with secret redaction filter."""
    level = logging.WARNING
    if debug:
        level = logging.DEBUG
    elif verbose:
        level = logging.INFO

    root = logging.getLogger("pdl")
    root.setLevel(level)
    root.handlers.clear()

    handler = logging.StreamHandler(sys.stderr)
    handler.setLevel(level)
    handler.addFilter(SecretRedactingFilter())
    fmt = logging.Formatter("[%(levelname)s] %(name)s: %(message)s")
    handler.setFormatter(fmt)
    root.addHandler(handler)
    root.propagate = False


def build_parser(config: PDLConfig) -> argparse.ArgumentParser:
    """Build the argparse.ArgumentParser with all pdl flags and recipes."""
    default_anti_throttle = config.anti_throttle
    default_grace = config.anti_throttle_grace

    parser = argparse.ArgumentParser(
        prog="parallel-dl",
        description="High-Performance Parallel Multichannel Downloader Suite [paradl / parallel-dl].",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Recipes:
  paradl "Song Title Artist Name" -o ~/Music
  paradl --mp3 "Artist Name Track" -o ~/Music
  paradl --audio-codec native "Artist Name Track" -o ~/Music
  paradl --quality 4k "https://www.youtube.com/watch?v=..."
  paradl gh:BurntSushi/ripgrep
  paradl install astral-sh/uv
  paradl pip install requests
  paradl brew install wget
  paradl hf:TheBloke/Llama-2-7B-GGUF --asset "*Q4_K_M.gguf"
  paradl ipsw:iPhone14,5 -c 16 -o ~/Downloads
  paradl "https://releases.ubuntu.com/24.04/ubuntu-24.04.1-desktop-amd64.iso" -c 16
  paradl netprobe "https://example.com/huge-file.iso"
  paradl bench "https://example.com/huge-file.iso" --watchdog v1,v2,off --duration 60
  paradl info "https://example.com/huge-file.iso"
  paradl hash ./downloaded.tar.gz
  paradl history
        """,
    )

    parser.add_argument(
        "targets",
        nargs="*",
        help="Subcommand (install/info/hash/extract/history/clean/config/diagnose/netprobe/bench) or download targets.",
    )
    parser.add_argument(
        "-a",
        "--audio",
        action="store_true",
        default=True,
        help="Extract audio track for media streams (default: True).",
    )
    parser.add_argument(
        "-v",
        "--video",
        action="store_true",
        help="Download complete high-res video (1080p/4K) instead of audio.",
    )
    parser.add_argument(
        "-l",
        "--lossless",
        action="store_true",
        default=True,
        help="CoreAudio M4A/ALAC stream packaging (default: True).",
    )
    parser.add_argument(
        "--no-lossless",
        action="store_true",
        help="Disable ALAC mastering, keep standard AAC.",
    )
    parser.add_argument(
        "--mp3",
        action="store_true",
        help="Shortcut for --audio-codec mp3.",
    )
    parser.add_argument(
        "--audio-codec",
        choices=["lossless", "native", "mp3", "flac", "opus", "wav", "aac"],
        help="Audio codec/format to export ('native' preserves original stream without re-encoding).",
    )
    parser.add_argument(
        "-Q",
        "--quality",
        choices=["best", "4k", "1440p", "1080p", "720p", "480p"],
        help="Video quality resolution preset.",
    )
    parser.add_argument(
        "--playlist",
        action="store_true",
        help="Download entire playlist instead of a single media item.",
    )
    parser.add_argument(
        "--subs",
        action="store_true",
        help="Download and embed subtitles into media.",
    )
    parser.add_argument(
        "--no-thumbnail",
        dest="thumbnail",
        action="store_false",
        default=True,
        help="Do not embed or write thumbnail image.",
    )
    parser.add_argument(
        "--cookies-from-browser",
        help="Extract cookies from browser for yt-dlp (e.g. safari, chrome, firefox).",
    )
    parser.add_argument(
        "--sponsorblock",
        action="store_true",
        help="Remove SponsorBlock segments via yt-dlp.",
    )
    parser.add_argument(
        "--ejs-remote",
        action="store_true",
        help="Opt-in to fetching remote yt-dlp EJS challenge solver scripts from GitHub (--remote-components ejs:github).",
    )

    parser.add_argument(
        "-c",
        "--connections",
        type=int,
        default=config.connections,
        help=f"Parallel connection streams for aria2c (1-16, maximum allowed: 16, default: {min(config.connections, 16)}).",
    )
    parser.add_argument(
        "-o",
        "--output",
        default=None,
        help=f"Output destination directory (default: '{config.output_dir}').",
    )
    parser.add_argument(
        "-i",
        "--input-file",
        help="File containing list of URLs/queries (one per line).",
    )
    parser.add_argument(
        "-x",
        "--extract",
        action="store_true",
        help="Automatically unpack downloaded archives (.zip, .tar.gz, .dmg, etc.).",
    )
    parser.add_argument(
        "--checksum",
        help="Expected cryptographic checksum in format [algo:]<hash> to verify upon completion.",
    )
    parser.add_argument(
        "--verify",
        action="store_true",
        help="Automatically discover and verify companion .sha256/.md5 checksum files.",
    )
    parser.add_argument(
        "-f",
        "--format",
        help="Custom yt-dlp format specification.",
    )
    parser.add_argument(
        "-q",
        "--quiet",
        action="store_true",
        help="Quiet mode, suppress banner and progress display.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit structured JSON-lines (JSONL) events to stdout for script and LLM callers.",
    )
    parser.add_argument(
        "--non-interactive",
        action="store_true",
        help="Forbid all interactive prompts regardless of TTY state.",
    )
    parser.add_argument(
        "-y",
        "--yes",
        action="store_true",
        help="Automatic yes to confirmation prompts (e.g. installing unverified release binaries).",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="Enable verbose informational logging on stderr.",
    )
    parser.add_argument(
        "--debug",
        action="store_true",
        help="Enable debug logging on stderr.",
    )
    parser.add_argument(
        "--limit-rate",
        help="Maximum download speed limit (e.g. 10MB/s, 80Mbps, 500K).",
    )
    parser.add_argument(
        "--anti-throttle",
        nargs="?",
        const="4MBps",
        default=default_anti_throttle,
        help=f"Auto-recycle connections when download speed drops below threshold (default: {default_anti_throttle}).",
    )
    parser.add_argument(
        "--no-anti-throttle",
        dest="anti_throttle",
        action="store_const",
        const=None,
        help="Disable anti-throttle connection recycling.",
    )
    parser.add_argument(
        "--anti-throttle-grace",
        type=int,
        default=default_grace,
        help=f"Grace period in seconds before checking speed threshold (default: {default_grace}).",
    )
    parser.add_argument(
        "--watchdog",
        default=None,
        help=f"Anti-throttle watchdog version: 'v1', 'v2', or 'off' (or comma-separated list for 'pdl bench', default: {config.watchdog}).",
    )
    parser.add_argument(
        "--no-caffeinate",
        dest="caffeinate",
        action="store_false",
        default=config.caffeinate,
        help="Disable macOS 'caffeinate -i -w <pid>' idle-sleep assertion during downloads.",
    )
    parser.add_argument(
        "--no-net-watch",
        dest="net_watch",
        action="store_false",
        default=config.net_watch,
        help="Disable macOS network-change watcher (scutil / route / scutil --nwi).",
    )
    parser.add_argument(
        "--stall-timeout",
        type=float,
        default=config.stall_timeout,
        help=f"Seconds without byte progress before triggering a stall connection recycle (0 to disable, default: {config.stall_timeout:g}).",
    )
    parser.add_argument(
        "--require-ac",
        dest="require_ac",
        action="store_true",
        default=None,
        help="Fail early unless the system is connected to AC power (via 'pmset -g batt').",
    )
    parser.add_argument(
        "--no-require-ac",
        dest="require_ac",
        action="store_false",
        default=None,
        help="Disable AC power requirement (e.g. for queued jobs which default to --require-ac).",
    )
    parser.add_argument(
        "--pause-on-low-power",
        action="store_true",
        default=config.pause_on_low_power,
        help="Pause or defer downloads while macOS Low Power Mode is active.",
    )
    parser.add_argument(
        "--mirror",
        dest="mirrors",
        action="append",
        default=[],
        help="Alternate mirror URL for the download target (repeatable).",
    )
    parser.add_argument(
        "--when-fast",
        dest="when_fast",
        nargs="?",
        const="DEFAULT",
        default=None,
        help="Start or continue large jobs only when a short speed test exceeds threshold (e.g. 5MB/s).",
    )
    parser.add_argument(
        "--at",
        dest="start_at",
        default=None,
        help="Schedule start time in 24-hour HH:MM format (e.g. 02:30).",
    )
    parser.add_argument(
        "--priority",
        type=int,
        default=0,
        help="Priority for queued jobs (higher integer runs first, default: 0).",
    )
    parser.add_argument(
        "--all",
        dest="all_flag",
        action="store_true",
        help="Apply queue/resume action to all jobs (e.g. 'pdl resume --all').",
    )
    parser.add_argument(
        "--interval",
        type=int,
        default=900,
        help="StartInterval in seconds for 'pdl queue launchd-plist' when --at is not set (default: 900).",
    )
    parser.add_argument(
        "--duration",
        type=float,
        default=60.0,
        help="Duration in seconds for 'pdl netprobe' or 'pdl bench' (default: 60).",
    )
    parser.add_argument(
        "--http3-probe",
        dest="http3_probe",
        action="store_true",
        help="Probe optional HTTP/3 support via curl during 'pdl netprobe'.",
    )
    parser.add_argument(
        "--http3",
        dest="http3",
        action="store_true",
        default=True,
        help="Enable HTTP/3 (QUIC) for aria2c transfers when supported by the server (default: on).",
    )
    parser.add_argument(
        "--no-http3",
        dest="http3",
        action="store_false",
        help="Disable HTTP/3 (QUIC) for aria2c transfers.",
    )
    parser.add_argument(
        "--proxy",
        "--via-proxy",
        dest="proxy",
        metavar="URL",
        help="Proxy URL for aria2c transfers (--all-proxy), e.g. socks5://127.0.0.1:1080 or http://user:pass@host:port.",
    )
    parser.add_argument(
        "--push",
        "--adb",
        action="store_true",
        dest="push_adb",
        default=config.auto_adb_push,
        help="Automatically push completed files to connected Android phone via ADB.",
    )

    # GitHub & Hugging Face Specific Flags
    parser.add_argument(
        "--asset",
        help="Filter release/HF asset by pattern/extension (e.g. dmg, tar.gz, arm64, *.gguf).",
    )
    parser.add_argument(
        "--all-assets",
        action="store_true",
        help="Download all assets in the release concurrently.",
    )
    parser.add_argument(
        "--select",
        action="store_true",
        help="Interactive table to choose which asset or model file to download (TTY only).",
    )
    parser.add_argument(
        "--source",
        action="store_true",
        help="Force download repo source archive instead of release binary.",
    )
    parser.add_argument(
        "--branch",
        help="Specify branch when downloading GitHub directory or repository archive.",
    )

    # Tool Flags
    parser.add_argument(
        "--install",
        dest="install_target",
        help="Download, verify, extract, and install GitHub release binary into local bin path.",
    )
    parser.add_argument(
        "--info",
        dest="info_target",
        help="Inspect target metadata without downloading.",
    )
    parser.add_argument(
        "--hash",
        dest="hash_file",
        help="Compute cryptographic hashes (MD5, SHA1, SHA256, SHA512, BLAKE2b) for a file.",
    )
    parser.add_argument(
        "--history",
        action="store_true",
        help="Display recent download history.",
    )
    parser.add_argument(
        "--history-limit",
        type=int,
        default=15,
        help="Number of history entries to show (default: 15).",
    )
    parser.add_argument(
        "--search",
        help="Filter history entries by substring match on filename or target.",
    )
    parser.add_argument(
        "--type",
        dest="history_type",
        choices=[
            "direct",
            "stream",
            "search",
            "github",
            "huggingface",
            "ipsw",
            "pypi",
            "brew",
            "torrent",
        ],
        help="Filter history entries by download type.",
    )
    parser.add_argument(
        "--success",
        dest="history_success",
        action="store_true",
        default=None,
        help="Show only successful history entries.",
    )
    parser.add_argument(
        "--failed",
        dest="history_success",
        action="store_false",
        help="Show only failed history entries.",
    )
    parser.add_argument(
        "--clear-history",
        action="store_true",
        help="Clear the download history log.",
    )
    parser.add_argument(
        "--from-history",
        type=int,
        metavar="ID",
        help="Re-download a previous history entry by its ID (used with 'pdl resume').",
    )
    parser.add_argument(
        "--clean",
        action="store_true",
        help="Clean orphaned temporary download files (.aria2, .part).",
    )
    parser.add_argument(
        "--diagnose",
        action="store_true",
        help="Print system toolchain diagnostics and installed engines.",
    )
    parser.add_argument(
        "--doctor",
        action="store_true",
        help="Run comprehensive system doctor, toolchain, and macOS security diagnostics.",
    )
    parser.add_argument(
        "--sign",
        action="store_true",
        help="Apply ad-hoc code signature ('codesign --force --sign -') to installed binary during 'pdl install'.",
    )
    parser.add_argument(
        "--no-xattr",
        action="store_true",
        help="Disable writing source URL to com.apple.metadata:kMDItemWhereFroms extended attribute.",
    )
    parser.add_argument(
        "--notify",
        action="store_true",
        help="Display native macOS notifications on download completion, failure, watchdog give-up, and queue summary.",
    )
    parser.add_argument(
        "--from-curl",
        dest="from_curl",
        nargs="?",
        const="-",
        default=None,
        help="Download from a 'Copy as cURL' command string, file path, or stdin ('-').",
    )
    parser.add_argument(
        "--clipboard",
        action="store_true",
        help="Read 'Copy as cURL' input directly from the macOS clipboard via pbpaste.",
    )
    parser.add_argument(
        "--version",
        action="version",
        version=f"parallel-dl (paradl) v{VERSION}",
    )

    # REST API server options (used by `pdl server`)
    parser.add_argument(
        "--host",
        default="127.0.0.1",
        help="API server bind host (default: 127.0.0.1). Use 0.0.0.0 to listen on all interfaces.",
    )
    parser.add_argument(
        "--port",
        type=int,
        default=8000,
        help="API server bind port (default: 8000).",
    )
    parser.add_argument(
        "--reload",
        action="store_true",
        help="Enable auto-reload for API development (requires watchfiles).",
    )

    return parser
