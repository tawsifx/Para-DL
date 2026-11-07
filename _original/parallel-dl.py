#!/usr/bin/env python3
"""
pdl (Parallel Downloader) - Pro Edition (v3.0.0)
High-Performance Multi-Channel Acceleration Suite with aria2c, GitHub PowerSuite,
Hugging Face Hub, Cloud Resolvers, Archive Extractor, Binary Installer,
yt-dlp CoreAudio/FFmpeg, Cryptographic Hash Suite, and Clean Rich UI.
"""

import sys
import os
import re
import json
import time
import signal
import shutil
import tempfile
import warnings
import argparse
import subprocess
import hashlib
import zipfile
import tarfile
import urllib.parse
from datetime import datetime
from pathlib import Path
from dataclasses import dataclass, field
from typing import Optional, List, Dict, Tuple, Any, Set, Union
from concurrent.futures import ThreadPoolExecutor, as_completed

# Suppress harmless urllib3 LibreSSL warnings on macOS
warnings.filterwarnings("ignore", category=UserWarning)
warnings.filterwarnings("ignore", module="urllib3")
os.environ["PYTHONWARNINGS"] = "ignore"

# Rich UI Framework
try:
    from rich.console import Console
    from rich.panel import Panel
    from rich.table import Table
    from rich.text import Text
    from rich.theme import Theme
    from rich.status import Status
    from rich.prompt import Prompt
    from rich import box
    RICH_AVAILABLE = True
except ImportError:
    RICH_AVAILABLE = False

# HTTP Requests
try:
    import requests
    REQUESTS_AVAILABLE = True
except ImportError:
    REQUESTS_AVAILABLE = False

# Audio ID3 / MP4 Tagging
try:
    from mutagen.mp4 import MP4
    from mutagen.id3 import ID3, TIT2, TPE1, APIC
    MUTAGEN_AVAILABLE = True
except ImportError:
    MUTAGEN_AVAILABLE = False

VERSION = "3.0.0"

# Setup Refined Minimal Rich Theme & Console
if RICH_AVAILABLE:
    custom_theme = Theme({
        "info": "bright_cyan",
        "warning": "yellow",
        "error": "bold red",
        "success": "green",
        "accent": "cyan",
        "dimmed": "dim",
        "highlight": "bold white",
        "muted": "dim white",
        "path": "bright_blue",
        "size": "dim cyan",
    })
    console = Console(theme=custom_theme)
else:
    console = None

def print_plain(msg: str, **kwargs):
    if console:
        console.print(msg, **kwargs)
    else:
        clean = re.sub(r'\[/?[a-zA-Z0-9_ =#]+\]', '', msg)
        print(clean)

# Known direct download file extensions
DIRECT_EXTENSIONS = {
    '.zip', '.tar', '.gz', '.tgz', '.bz2', '.tbz2', '.xz', '.txz', '.7z', '.rar',
    '.iso', '.img', '.dmg', '.pkg', '.deb', '.rpm', '.apk', '.ipa', '.ipsw',
    '.bin', '.exe', '.msi', '.appimage', '.safetensors', '.gguf', '.pt', '.pth',
    '.onnx', '.ckpt', '.parquet', '.csv', '.jsonl',
    '.mp4', '.mkv', '.avi', '.mov', '.webm', '.flv',
    '.mp3', '.m4a', '.flac', '.wav', '.opus', '.aac', '.ogg',
    '.pdf', '.epub', '.doc', '.docx',
    '.png', '.jpg', '.jpeg', '.webp', '.gif', '.svg'
}

# Known streaming media domains
MEDIA_DOMAINS = [
    "youtube.com", "youtu.be", "soundcloud.com", "vimeo.com", "twitch.tv",
    "bandcamp.com", "bilibili.com", "dailymotion.com", "mixcloud.com",
    "facebook.com", "instagram.com", "tiktok.com", "twitter.com", "x.com"
]


# ==============================================================================
# Configuration Management
# ==============================================================================
CONFIG_DIR = Path.home() / ".config" / "pdl"
CONFIG_FILE = CONFIG_DIR / "config.json"
HISTORY_FILE = CONFIG_DIR / "history.json"

@dataclass
class PDLConfig:
    connections: int = 16
    output_dir: str = "."
    audio_codec: str = "lossless"  # 'lossless' (ALAC), 'mp3', 'flac', 'opus', 'wav', 'aac'
    video_quality: str = "best"    # 'best', '4k', '1440p', '1080p', '720p', '480p'
    auto_extract: bool = False
    auto_adb_push: bool = False
    bin_dir: str = str(Path.home() / ".local/bin")
    adb_music_dir: str = "/storage/emulated/0/Music"
    adb_download_dir: str = "/storage/emulated/0/Download"
    max_download_limit: Optional[str] = None
    history_limit: int = 50
    anti_throttle: Optional[str] = "4MBps"
    anti_throttle_grace: int = 5

    @classmethod
    def load(cls) -> "PDLConfig":
        if CONFIG_FILE.exists():
            try:
                with open(CONFIG_FILE, "r") as f:
                    data = json.load(f)
                    return cls(**{k: v for k, v in data.items() if hasattr(cls, k)})
            except Exception:
                pass
        return cls()

    def save(self):
        CONFIG_DIR.mkdir(parents=True, exist_ok=True)
        with open(CONFIG_FILE, "w") as f:
            json.dump(self.__dict__, f, indent=2)

    def set(self, key: str, value: Any) -> bool:
        if not hasattr(self, key):
            return False
        orig_val = getattr(self, key)
        if isinstance(orig_val, bool):
            val = str(value).lower() in ("true", "1", "yes", "on")
        elif isinstance(orig_val, int):
            val = int(value)
        else:
            val = value
        setattr(self, key, val)
        self.save()
        return True


# ==============================================================================
# Dependency & Binary Discovery
# ==============================================================================
class Toolchain:
    def __init__(self):
        self.aria2c = self._find_bin("aria2c", [
            "/opt/homebrew/bin/aria2c",
            "/usr/local/bin/aria2c",
            "/usr/bin/aria2c"
        ])
        self.ytdlp = self._find_bin("yt-dlp", [
            str(Path.home() / "Library/Python/3.9/bin/yt-dlp"),
            str(Path.home() / ".local/bin/yt-dlp"),
            "/opt/homebrew/bin/yt-dlp",
            "/usr/local/bin/yt-dlp"
        ])
        self.afconvert = self._find_bin("afconvert", ["/usr/bin/afconvert"])
        self.ffmpeg = self._find_bin("ffmpeg", [
            "/opt/homebrew/bin/ffmpeg",
            "/usr/local/bin/ffmpeg",
            "/usr/bin/ffmpeg"
        ])
        self.adb = self._find_bin("adb", [
            "/opt/homebrew/bin/adb",
            "/usr/local/bin/adb",
            str(Path.home() / "Library/Android/sdk/platform-tools/adb")
        ])
        self.hdiutil = self._find_bin("hdiutil", ["/usr/bin/hdiutil"])
        self.p7zip = self._find_bin("7z", ["/opt/homebrew/bin/7z", "/usr/local/bin/7z"]) or self._find_bin("7za", ["/opt/homebrew/bin/7za"])

        # JavaScript runtimes for yt-dlp EJS challenge solving
        self.deno = self._find_bin("deno", [
            "/opt/homebrew/bin/deno",
            "/usr/local/bin/deno",
            str(Path.home() / ".deno/bin/deno")
        ])
        nvm_node = None
        nvm_dir = Path.home() / ".nvm/versions/node"
        if nvm_dir.exists():
            for vdir in sorted(nvm_dir.glob("v*"), reverse=True):
                candidate = vdir / "bin/node"
                if candidate.exists() and os.access(candidate, os.X_OK):
                    nvm_node = str(candidate)
                    break
        self.node = self._find_bin("node", [
            "/opt/homebrew/bin/node",
            "/usr/local/bin/node",
            "/usr/bin/node"
        ] + ([nvm_node] if nvm_node else []))
        self.bun = self._find_bin("bun", [
            "/opt/homebrew/bin/bun",
            "/usr/local/bin/bun",
            str(Path.home() / ".bun/bin/bun")
        ])
        self.quickjs = self._find_bin("qjs", [
            "/opt/homebrew/bin/qjs",
            "/usr/local/bin/qjs"
        ])

    @property
    def js_runtime(self) -> Optional[str]:
        if self.deno:
            return f"deno ({self.deno})"
        if self.node:
            return f"node ({self.node})"
        if self.bun:
            return f"bun ({self.bun})"
        if self.quickjs:
            return f"quickjs ({self.quickjs})"
        return None

    def get_ytdlp_js_args(self) -> List[str]:
        args: List[str] = []
        runtimes: List[str] = []
        if self.deno:
            runtimes.append(f"deno:{self.deno}")
        if self.node:
            runtimes.append(f"node:{self.node}")
        if self.bun:
            runtimes.append(f"bun:{self.bun}")
        if self.quickjs:
            runtimes.append(f"quickjs:{self.quickjs}")

        for rt in runtimes:
            args.extend(["--js-runtimes", rt])
        if not runtimes:
            args.extend(["--js-runtimes", "node"])

        args.extend(["--remote-components", "ejs:github"])
        return args

    def _find_bin(self, name: str, fallback_paths: List[str]) -> Optional[str]:
        found = shutil.which(name)
        if found:
            return found
        for p in fallback_paths:
            if os.path.exists(p) and os.access(p, os.X_OK):
                return p
        return None

TOOLCHAIN = Toolchain()

# Temp files registry for signal cleanup
ACTIVE_TEMP_FILES = set()

def cleanup_temp_files():
    for f in list(ACTIVE_TEMP_FILES):
        try:
            if os.path.isdir(f):
                shutil.rmtree(f, ignore_errors=True)
            elif os.path.exists(f):
                os.remove(f)
        except Exception:
            pass
        aria2_file = f"{f}.aria2"
        if os.path.exists(aria2_file):
            try: os.remove(aria2_file)
            except Exception: pass
    ACTIVE_TEMP_FILES.clear()

def sigint_handler(sig, frame):
    print_plain("\n[warning]  Cancelled by user. Download paused. Resume session preserved in .aria2 file.[/warning]")
    cleanup_temp_files()
    sys.exit(130)

signal.signal(signal.SIGINT, sigint_handler)
signal.signal(signal.SIGTERM, sigint_handler)


# ==============================================================================
# Helper Utilities
# ==============================================================================
def sanitize_filename(name: str) -> str:
    for ch in ['/', '\\', ':', '*', '?', '"', '<', '>', '|']:
        name = name.replace(ch, '_')
    return name.strip()[:200]

def format_size(bytes_sz: Optional[int]) -> str:
    if not bytes_sz:
        return "Unknown size"
    for unit in ['B', 'KB', 'MB', 'GB', 'TB']:
        if bytes_sz < 1024.0:
            return f"{bytes_sz:.1f} {unit}"
        bytes_sz /= 1024.0
    return f"{bytes_sz:.1f} PB"


# ==============================================================================
# History Manager
# ==============================================================================
class HistoryManager:
    @staticmethod
    def record(target: str, target_type: str, file_path: Optional[str], size: Optional[int], duration: float, success: bool, adb: bool = False):
        CONFIG_DIR.mkdir(parents=True, exist_ok=True)
        entries = []
        if HISTORY_FILE.exists():
            try:
                with open(HISTORY_FILE, "r") as f:
                    entries = json.load(f)
            except Exception:
                entries = []

        entry = {
            "timestamp": datetime.now().strftime("%Y-%m-%d %H:%M:%S"),
            "target": target,
            "type": target_type,
            "filename": os.path.basename(file_path) if file_path else "-",
            "path": str(file_path) if file_path else "-",
            "size": size or (os.path.getsize(file_path) if file_path and os.path.isfile(file_path) else 0),
            "duration": round(duration, 2),
            "success": success,
            "adb": adb
        }

        entries.insert(0, entry)
        entries = entries[:100]

        try:
            with open(HISTORY_FILE, "w") as f:
                json.dump(entries, f, indent=2)
        except Exception:
            pass

    @staticmethod
    def display(limit: int = 15):
        if not HISTORY_FILE.exists():
            print_plain("  [dim]No download history recorded yet.[/dim]")
            return

        try:
            with open(HISTORY_FILE, "r") as f:
                entries = json.load(f)
        except Exception:
            print_plain("  [error]Failed to read history log.[/error]")
            return

        if not entries:
            print_plain("  [dim]Download history is empty.[/dim]")
            return

        table = Table(title=f"Recent Downloads (Last {min(limit, len(entries))})", box=box.HORIZONTALS, pad_edge=False, expand=True)
        table.add_column("Date", style="dim", ratio=2)
        table.add_column("Status", justify="center", ratio=1)
        table.add_column("Type", style="cyan", ratio=1)
        table.add_column("Filename", style="bold white", ratio=4)
        table.add_column("Size", justify="right", style="dim cyan", ratio=1)
        table.add_column("Time", justify="right", style="dim", ratio=1)

        for e in entries[:limit]:
            st = "[green]✓[/green]" if e.get("success") else "[red]✗[/red]"
            sz_str = format_size(e.get("size")) if e.get("size") else "-"
            dur_str = f"{e.get('duration')}s" if e.get('duration') else "-"
            table.add_row(
                e.get("timestamp", "-")[:16],
                st,
                e.get("type", "-")[:8],
                e.get("filename", "-")[:40],
                sz_str,
                dur_str
            )

        if console:
            console.print()
            console.print(table)
            console.print()

    @staticmethod
    def clear():
        if HISTORY_FILE.exists():
            try:
                os.remove(HISTORY_FILE)
                print_plain("  [green]✓ History log cleared.[/green]")
            except Exception as e:
                print_plain(f"  [error]Error clearing history: {e}[/error]")
        else:
            print_plain("  [dim]History log is already empty.[/dim]")


# ==============================================================================
# Cryptographic Checksum & Hash Engine
# ==============================================================================
class ChecksumEngine:
    @staticmethod
    def compute_hashes(file_path: Path, algos: List[str] = ["sha256", "md5"]) -> Dict[str, str]:
        hashers = {}
        for a in algos:
            a_lower = a.lower()
            if a_lower == "sha256":
                hashers["sha256"] = hashlib.sha256()
            elif a_lower == "sha512":
                hashers["sha512"] = hashlib.sha512()
            elif a_lower == "sha1":
                hashers["sha1"] = hashlib.sha1()
            elif a_lower == "md5":
                hashers["md5"] = hashlib.md5()
            elif a_lower == "blake2b":
                hashers["blake2b"] = hashlib.blake2b()

        buf_size = 1024 * 1024  # 1MB buffer
        with open(file_path, "rb") as f:
            while chunk := f.read(buf_size):
                for h in hashers.values():
                    h.update(chunk)

        return {name: h.hexdigest() for name, h in hashers.items()}

    @classmethod
    def verify(cls, file_path: Path, expected: str) -> Tuple[bool, str, str]:
        expected = expected.strip()
        algo = "sha256"
        target_hash = expected

        if ":" in expected:
            parts = expected.split(":", 1)
            algo = parts[0].lower()
            target_hash = parts[1].strip()
        else:
            lh = len(expected)
            if lh == 32: algo = "md5"
            elif lh == 40: algo = "sha1"
            elif lh == 64: algo = "sha256"
            elif lh == 128: algo = "sha512"

        computed = cls.compute_hashes(file_path, [algo]).get(algo, "")
        is_match = computed.lower() == target_hash.lower()
        return is_match, algo, computed

    @classmethod
    def find_companion_checksum(cls, file_path: Path) -> Optional[Tuple[str, str]]:
        parent = file_path.parent
        name = file_path.name

        candidates = [
            parent / f"{name}.sha256",
            parent / f"{name}.sha512",
            parent / f"{name}.md5",
            parent / "sha256sums.txt",
            parent / "SHA256SUMS",
            parent / "checksums.txt"
        ]

        for cand in candidates:
            if cand.exists() and cand.is_file():
                try:
                    with open(cand, "r") as f:
                        for line in f:
                            line = line.strip()
                            if not line or line.startswith("#"):
                                continue
                            if name in line:
                                parts = line.split()
                                if parts:
                                    h = parts[0]
                                    algo = "sha256" if len(h) == 64 else ("sha512" if len(h) == 128 else "md5")
                                    return algo, h
                except Exception:
                    pass
        return None

    @classmethod
    def display_hash_table(cls, file_path: Path):
        p = Path(file_path).expanduser().resolve()
        if not p.exists() or not p.is_file():
            print_plain(f"  [error]File not found: {file_path}[/error]")
            return

        size = p.stat().st_size
        t0 = time.time()
        with Status("[dim]  Computing cryptographic hashes...[/dim]", console=console) if console else open(os.devnull) as _:
            hashes = cls.compute_hashes(p, ["md5", "sha1", "sha256", "sha512", "blake2b"])
        dur = round(time.time() - t0, 3)

        table = Table(title=f"Cryptographic Hashes: {p.name} ({format_size(size)})", box=box.HORIZONTALS, pad_edge=False)
        table.add_column("Algorithm", style="bold cyan")
        table.add_column("Hex Digest", style="white")

        for algo in ["md5", "sha1", "sha256", "sha512", "blake2b"]:
            if algo in hashes:
                table.add_row(algo.upper(), hashes[algo])

        if console:
            console.print()
            console.print(table)
            console.print(f"  [dim]Computed in {dur}s ({format_size(int(size / max(dur, 0.001)))}/s)[/dim]\n")


# ==============================================================================
# Archive Auto-Extractor & DMG Mounter
# ==============================================================================
class ArchiveExtractor:
    @staticmethod
    def is_archive(path: Path) -> bool:
        ext = path.suffix.lower()
        double_ext = "".join(path.suffixes[-2:]).lower()
        archive_exts = {
            ".zip", ".tar", ".tgz", ".tar.gz", ".txz", ".tar.xz", ".tbz2", ".tar.bz2",
            ".7z", ".dmg"
        }
        return ext in archive_exts or double_ext in archive_exts

    @classmethod
    def extract(cls, archive_path: Path, dest_dir: Path, remove_archive: bool = False) -> Tuple[bool, Optional[Path], List[Path]]:
        archive_path = Path(archive_path).expanduser().resolve()
        if not archive_path.exists():
            return False, None, []

        dest_dir = Path(dest_dir).expanduser().resolve()
        dest_dir.mkdir(parents=True, exist_ok=True)

        name = archive_path.name
        lower_name = name.lower()
        extracted_files: List[Path] = []
        target_out = dest_dir

        print_plain(f"  [dim]•[/dim] [white]Extracting archive:[/white] [cyan]{name}[/cyan] -> [dim]{dest_dir}[/dim]")

        try:
            # ZIP Archive
            if lower_name.endswith(".zip"):
                with zipfile.ZipFile(archive_path, 'r') as zf:
                    for member in zf.infolist():
                        zf.extract(member, dest_dir)
                        out_f = dest_dir / member.filename
                        # Preserve POSIX file permissions (e.g. executable bit)
                        perm = member.external_attr >> 16
                        if perm and out_f.exists():
                            try: os.chmod(out_f, perm)
                            except: pass
                        if out_f.is_file():
                            extracted_files.append(out_f)

            # Tarball / Gzip / Xz / Bz2
            elif any(lower_name.endswith(x) for x in [".tar", ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2"]):
                mode = "r:*"
                with tarfile.open(archive_path, mode) as tf:
                    tf.extractall(dest_dir)
                    for member in tf.getmembers():
                        out_f = dest_dir / member.name
                        if out_f.is_file():
                            extracted_files.append(out_f)

            # macOS DMG Disk Image
            elif lower_name.endswith(".dmg") and TOOLCHAIN.hdiutil:
                with Status("[dim]  Mounting macOS DMG image...[/dim]", console=console) if console else open(os.devnull) as _:
                    mount_proc = subprocess.run(
                        [TOOLCHAIN.hdiutil, "attach", "-nobrowse", "-readonly", str(archive_path)],
                        capture_output=True, text=True
                    )
                if mount_proc.returncode == 0:
                    mount_point = None
                    for line in mount_proc.stdout.splitlines():
                        if "/Volumes/" in line:
                            mount_point = line.split("/Volumes/")[1].strip()
                            mount_point = f"/Volumes/{mount_point}"
                            break

                    if mount_point and os.path.exists(mount_point):
                        try:
                            # Look for .app bundles or binaries in DMG
                            for item in os.listdir(mount_point):
                                src_item = os.path.join(mount_point, item)
                                dst_item = dest_dir / item
                                if item.endswith(".app"):
                                    if dst_item.exists():
                                        shutil.rmtree(dst_item, ignore_errors=True)
                                    shutil.copytree(src_item, dst_item)
                                    extracted_files.append(dst_item)
                                    print_plain(f"  [green]✓[/green] [white]Copied application bundle:[/white] [cyan]{item}[/cyan]")
                                elif not item.startswith("."):
                                    if os.path.isfile(src_item):
                                        shutil.copy2(src_item, dst_item)
                                        extracted_files.append(dst_item)
                        finally:
                            subprocess.run([TOOLCHAIN.hdiutil, "detach", mount_point], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                else:
                    print_plain("  [error]Failed to attach DMG disk image.[/error]")
                    return False, None, []

            # 7z Archive
            elif lower_name.endswith(".7z") and TOOLCHAIN.p7zip:
                cmd = [TOOLCHAIN.p7zip, "x", f"-o{dest_dir}", "-y", str(archive_path)]
                res = subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                if res.returncode != 0:
                    print_plain("  [error]7z extraction failed.[/error]")
                    return False, None, []
            else:
                print_plain(f"  [warning]Unsupported archive format for automatic extraction: {name}[/warning]")
                return False, None, []

            print_plain(f"  [green]✓[/green] [white]Extracted {len(extracted_files)} files[/white]")

            if remove_archive:
                try: archive_path.unlink()
                except: pass

            return True, dest_dir, extracted_files

        except Exception as e:
            print_plain(f"  [error]Extraction error: {e}[/error]")
            return False, None, []


# ==============================================================================
# Cloud Sharing & URL Resolvers (Google Drive, Dropbox, Gist, Pastebin)
# ==============================================================================
class CloudResolver:
    @staticmethod
    def resolve_url(url: str) -> Tuple[str, Optional[str]]:
        u = url.strip()

        # Google Drive: convert preview link to direct export stream
        if "drive.google.com" in u:
            file_id = None
            m = re.search(r"drive\.google\.com/file/d/([a-zA-Z0-9_-]+)", u)
            if m: file_id = m.group(1)
            else:
                m = re.search(r"[?&]id=([a-zA-Z0-9_-]+)", u)
                if m: file_id = m.group(1)

            if file_id:
                direct = f"https://drive.usercontent.google.com/download?id={file_id}&export=download&confirm=t"
                return direct, None

        # Dropbox: change dl=0 to dl=1
        if "dropbox.com" in u:
            if "dl=0" in u:
                return u.replace("dl=0", "dl=1"), None
            elif "?" in u:
                return f"{u}&dl=1", None
            else:
                return f"{u}?dl=1", None

        # Pastebin: direct raw URL
        if "pastebin.com" in u and "/raw/" not in u:
            m = re.search(r"pastebin\.com/([a-zA-Z0-9]+)$", u)
            if m:
                return f"https://pastebin.com/raw/{m.group(1)}", None

        return u, None


# ==============================================================================
# Hugging Face Hub Acceleration Engine
# ==============================================================================
class HuggingFaceEngine:
    @staticmethod
    def get_headers() -> Dict[str, str]:
        token = os.environ.get("HF_TOKEN") or os.environ.get("HUGGING_FACE_HUB_TOKEN")
        if not token:
            token_file = Path.home() / ".cache/huggingface/token"
            if token_file.exists():
                try: token = token_file.read_text().strip()
                except: pass

        headers = {"User-Agent": f"pdl-hf-accelerator/{VERSION}"}
        if token:
            headers["Authorization"] = f"Bearer {token}"
        return headers

    @staticmethod
    def is_hf(target: str) -> bool:
        t = target.strip().lower()
        return t.startswith("hf:") or "huggingface.co" in t

    @staticmethod
    def parse_hf_target(target: str) -> Dict[str, Any]:
        t = target.strip()
        repo_type = "model"
        revision = "main"

        # Prefix hf:
        if t.startswith("hf:"):
            raw_path = t[3:].strip()
            if raw_path.startswith("datasets/"):
                repo_type = "dataset"
                raw_path = raw_path[len("datasets/"):]

            parts = raw_path.split("/")
            if len(parts) >= 2:
                repo_id = f"{parts[0]}/{parts[1]}"
                subpath = "/".join(parts[2:]) if len(parts) > 2 else None
                return {
                    "repo_id": repo_id,
                    "repo_type": repo_type,
                    "subpath": subpath,
                    "revision": revision
                }
            elif len(parts) == 1:
                return {
                    "repo_id": parts[0],
                    "repo_type": repo_type,
                    "subpath": None,
                    "revision": revision
                }

        # Full URL parsing
        # e.g. https://huggingface.co/TheBloke/Llama-2-7B-GGUF/resolve/main/llama-2-7b.Q4_K_M.gguf
        m_file = re.search(r"huggingface\.co/(?:datasets/)?([^/]+/[^/]+)/(?:resolve|blob)/([^/]+)/(.+)", t)
        if m_file:
            repo_id, revision, filepath = m_file.groups()
            return {
                "repo_id": repo_id,
                "repo_type": "dataset" if "/datasets/" in t else "model",
                "subpath": filepath,
                "revision": revision,
                "is_direct_file": True
            }

        m_repo = re.search(r"huggingface\.co/(?:datasets/)?([^/]+/[^/]+)(?:/)?$", t)
        if m_repo:
            repo_id = m_repo.group(1)
            return {
                "repo_id": repo_id,
                "repo_type": "dataset" if "/datasets/" in t else "model",
                "subpath": None,
                "revision": revision
            }

        return {"repo_id": t, "repo_type": "model", "subpath": None, "revision": revision}

    @classmethod
    def fetch_hf_info(cls, repo_id: str, repo_type: str = "model") -> Optional[Dict[str, Any]]:
        endpoint = "datasets" if repo_type == "dataset" else "models"
        api_url = f"https://huggingface.co/api/{endpoint}/{repo_id}"
        try:
            resp = requests.get(api_url, headers=cls.get_headers(), timeout=7)
            if resp.status_code == 200:
                return resp.json()
        except Exception:
            pass
        return None

    @classmethod
    def download(
        cls,
        meta: Dict[str, Any],
        out_dir: Path,
        connections: int = 16,
        pattern: Optional[str] = None,
        interactive: bool = False
    ) -> Tuple[bool, Optional[Path]]:
        repo_id = meta["repo_id"]
        revision = meta.get("revision", "main")
        subpath = meta.get("subpath")
        repo_type = meta.get("repo_type", "model")

        out_dir.mkdir(parents=True, exist_ok=True)

        # Single direct file download
        if meta.get("is_direct_file") and subpath:
            direct_url = f"https://huggingface.co/{repo_id}/resolve/{revision}/{subpath}"
            fname = os.path.basename(subpath)
            print_plain(f"  [dim]•[/dim] [white]Hugging Face File:[/white] [cyan]{fname}[/cyan] [dim]({repo_id})[/dim]")
            t_info = TargetInfo(raw=direct_url, target_type="direct", clean_url=direct_url, filename_hint=fname)
            return Aria2Engine.download(t_info, out_dir, connections)

        # Fetch model tree / siblings
        print_plain(f"  [dim]•[/dim] [white]Querying Hugging Face Hub:[/white] [cyan]{repo_id}[/cyan]")
        info = cls.fetch_hf_info(repo_id, repo_type)
        if not info:
            print_plain(f"  [error]✗ Repository not found or private on Hugging Face: {repo_id}[/error]")
            return False, None

        siblings = info.get("siblings", [])
        if not siblings:
            print_plain(f"  [error]✗ No files found in repository {repo_id}[/error]")
            return False, None

        # Filter out hidden and root git files
        valid_files = [s for s in siblings if not s.get("rfilename", "").startswith(".")]

        if subpath:
            valid_files = [s for s in valid_files if s.get("rfilename") == subpath or s.get("rfilename", "").endswith(subpath)]

        if pattern:
            valid_files = [s for s in valid_files if re.search(pattern, s.get("rfilename", ""), re.I)]

        if not valid_files:
            print_plain(f"  [warning]No files matched filter. Showing repository files:[/warning]")
            valid_files = [s for s in siblings if not s.get("rfilename", "").startswith(".")]
            interactive = True

        # Interactive or multi-file selection
        if interactive or len(valid_files) > 1:
            table = Table(title=f"Hugging Face: {repo_id} ({len(valid_files)} files)", box=box.HORIZONTALS, pad_edge=False)
            table.add_column("#", justify="right", style="cyan")
            table.add_column("Filename", style="bold white")
            table.add_column("Type", style="dim")

            for idx, f in enumerate(valid_files[:30], 1):
                fname = f.get("rfilename", "")
                ext = os.path.splitext(fname)[1] or "file"
                table.add_row(str(idx), fname, ext)

            if console:
                console.print()
                console.print(table)
                if len(valid_files) > 30:
                    console.print(f"  [dim]... and {len(valid_files) - 30} more files[/dim]")
                console.print()

            if interactive and console:
                choices = [str(i) for i in range(1, min(len(valid_files), 30) + 1)]
                choices.append("all")
                ans = Prompt.ask("  Select file number to download (or 'all')", choices=choices, default="1")
                if ans != "all":
                    chosen = valid_files[int(ans) - 1]
                    valid_files = [chosen]

        if len(valid_files) == 1:
            rfilename = valid_files[0]["rfilename"]
            dl_url = f"https://huggingface.co/{repo_id}/resolve/{revision}/{rfilename}"
            fname = os.path.basename(rfilename)
            print_plain(f"  [green]✓[/green] [white]Selected:[/white] [bold cyan]{fname}[/bold cyan]")
            t_info = TargetInfo(raw=dl_url, target_type="direct", clean_url=dl_url, filename_hint=fname)
            return Aria2Engine.download(t_info, out_dir, connections)

        # Batch download multiple files
        print_plain(f"  [dim]• Batch downloading {len(valid_files)} files from {repo_id}...[/dim]")
        batch_lines = []
        token = os.environ.get("HF_TOKEN") or os.environ.get("HUGGING_FACE_HUB_TOKEN")

        for f in valid_files:
            rfilename = f["rfilename"]
            dl_url = f"https://huggingface.co/{repo_id}/resolve/{revision}/{rfilename}"
            sub_dest = out_dir / os.path.dirname(rfilename)
            sub_dest.mkdir(parents=True, exist_ok=True)
            batch_lines.append(dl_url)
            batch_lines.append(f"  dir={sub_dest}")
            batch_lines.append(f"  out={os.path.basename(rfilename)}")
            batch_lines.append(f"  split={connections}")
            batch_lines.append(f"  max-connection-per-server={connections}")
            if token:
                batch_lines.append(f"  header=Authorization: Bearer {token}")

        with tempfile.NamedTemporaryFile("w", delete=False, prefix="pdl_hf_") as tf:
            tf.write("\n".join(batch_lines) + "\n")
            tmp_file = tf.name

        try:
            cmd = [
                TOOLCHAIN.aria2c,
                "-i", tmp_file,
                f"-j{min(len(valid_files), 4)}",
                f"-x{min(connections, 16)}",
                f"-s{connections}",
                "-k1M",
                "--continue=true",
                "--file-allocation=none",
                "--console-log-level=warn"
            ]
            res = subprocess.run(cmd)
            return res.returncode == 0, out_dir
        finally:
            if os.path.exists(tmp_file):
                try: os.remove(tmp_file)
                except: pass


# ==============================================================================
# GitHub Engine - Releases, Assets, Blobs, Subtrees, and Archives
# ==============================================================================
class GitHubEngine:
    @staticmethod
    def get_headers() -> Dict[str, str]:
        token = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
        headers = {
            "User-Agent": f"pdl-accelerator/{VERSION}",
            "Accept": "application/vnd.github.v3+json"
        }
        if token:
            headers["Authorization"] = f"Bearer {token}"
        return headers

    @staticmethod
    def is_github(target: str) -> bool:
        t = target.strip().lower()
        return (
            t.startswith("gh:") or
            t.startswith("github:") or
            "github.com" in t or
            "raw.githubusercontent.com" in t or
            "gist.github.com" in t
        )

    @staticmethod
    def parse_github_target(target: str) -> Dict[str, Any]:
        """Classify GitHub target into: asset, release, blob, tree, repo, gist."""
        t = target.strip()

        # Shortcut gh:owner/repo
        if t.startswith("gh:") or t.startswith("github:"):
            parts = t.split(":", 1)[1].strip().split("/")
            if len(parts) >= 2:
                owner, repo = parts[0], parts[1]
                return {"type": "repo", "owner": owner, "repo": repo, "url": f"https://github.com/{owner}/{repo}"}

        # Gist
        if "gist.github.com" in t:
            m = re.search(r"gist\.github\.com/([^/]+)/([a-f0-9]+)", t)
            if m:
                return {"type": "gist", "user": m.group(1), "gist_id": m.group(2), "url": t}

        # Direct Release Asset: /releases/download/{tag}/{file}
        m = re.search(r"github\.com/([^/]+)/([^/]+)/releases/download/([^/]+)/(.+)", t)
        if m:
            owner, repo, tag, fname = m.groups()
            return {"type": "asset", "owner": owner, "repo": repo, "tag": tag, "filename": fname, "url": t}

        # Release Page or Tag: /releases(/tag/{tag}|/latest)?
        m = re.search(r"github\.com/([^/]+)/([^/]+)/releases(?:/tag/([^/]+)|/latest)?(?:/)?$", t)
        if m:
            owner, repo, tag = m.groups()
            return {"type": "release", "owner": owner, "repo": repo, "tag": tag or "latest", "url": t}

        # Blob / Single File: /blob/{branch}/{path} -> convert to raw
        m = re.search(r"github\.com/([^/]+)/([^/]+)/blob/([^/]+)/(.+)", t)
        if m:
            owner, repo, branch, path = m.groups()
            raw_url = f"https://raw.githubusercontent.com/{owner}/{repo}/{branch}/{path}"
            return {"type": "blob", "owner": owner, "repo": repo, "branch": branch, "path": path, "url": raw_url, "filename": os.path.basename(path)}

        # Already Raw URL: raw.githubusercontent.com/{owner}/{repo}/{branch}/{path}
        m = re.search(r"raw\.githubusercontent\.com/([^/]+)/([^/]+)/([^/]+)/(.+)", t)
        if m:
            owner, repo, branch, path = m.groups()
            return {"type": "blob", "owner": owner, "repo": repo, "branch": branch, "path": path, "url": t, "filename": os.path.basename(path)}

        # Tree / Subdirectory: /tree/{branch}/{path}
        m = re.search(r"github\.com/([^/]+)/([^/]+)/tree/([^/]+)/(.+)", t)
        if m:
            owner, repo, branch, path = m.groups()
            return {"type": "tree", "owner": owner, "repo": repo, "branch": branch, "path": path, "url": t}

        # Repository Root: github.com/{owner}/{repo}(.git)?
        m = re.search(r"github\.com/([^/]+)/([^/]+?)(?:\.git)?(?:/)?$", t)
        if m:
            owner, repo = m.groups()
            return {"type": "repo", "owner": owner, "repo": repo, "url": f"https://github.com/{owner}/{repo}"}

        return {"type": "generic", "url": t}

    @staticmethod
    def score_asset_for_host(filename: str) -> int:
        """Score release asset filename for macOS host (ARM64 priority)."""
        fn = filename.lower()
        score = 0
        is_arm = os.uname().machine in ("arm64", "aarch64")

        is_mac = any(x in fn for x in ["darwin", "macos", "apple", "osx", ".dmg", ".pkg"])
        if is_mac:
            score += 40
            if fn.endswith(".dmg"):
                score += 25
            elif fn.endswith(".pkg"):
                score += 20
            elif fn.endswith(".tar.gz") or fn.endswith(".zip"):
                score += 15

            if is_arm:
                if any(x in fn for x in ["arm64", "aarch64", "apple-darwin"]):
                    score += 35
                elif "universal" in fn:
                    score += 30
                elif any(x in fn for x in ["x86_64", "x64", "amd64"]):
                    score += 5
            else:
                if any(x in fn for x in ["x86_64", "x64", "amd64"]):
                    score += 35
                elif "universal" in fn:
                    score += 30
        else:
            if any(x in fn for x in ["linux", "windows", ".exe", ".msi", ".deb", ".rpm", ".apk"]):
                score = 0
            elif fn.endswith(".zip") or fn.endswith(".tar.gz"):
                score += 5

        if any(fn.endswith(ext) for ext in [".sha256", ".sha512", ".md5", ".sig", ".asc", ".sbom"]):
            score = 0

        return score

    @classmethod
    def fetch_release_info(cls, owner: str, repo: str, tag: str = "latest") -> Optional[Dict[str, Any]]:
        url = f"https://api.github.com/repos/{owner}/{repo}/releases/latest" if tag in ("latest", None) else f"https://api.github.com/repos/{owner}/{repo}/releases/tags/{tag}"
        try:
            resp = requests.get(url, headers=cls.get_headers(), timeout=6)
            if resp.status_code == 200:
                return resp.json()
        except Exception:
            pass
        return None

    @classmethod
    def download_release_asset(
        cls,
        owner: str,
        repo: str,
        tag: str,
        out_dir: Path,
        connections: int,
        asset_pattern: Optional[str] = None,
        download_all: bool = False,
        interactive: bool = False
    ) -> Tuple[bool, Optional[Path]]:
        print_plain(f"  [dim]•[/dim] [white]GitHub Release:[/white] [cyan]{owner}/{repo}[/cyan] [dim]({tag})[/dim]")
        info = cls.fetch_release_info(owner, repo, tag)
        if not info:
            print_plain(f"  [error]✗ Release not found for {owner}/{repo}[/error]")
            return False, None

        assets = info.get("assets", [])
        tag_name = info.get("tag_name", tag)

        if not assets:
            tarball_url = info.get("tarball_url") or f"https://github.com/{owner}/{repo}/archive/refs/tags/{tag_name}.tar.gz"
            fname = f"{repo}-{tag_name}.tar.gz"
            print_plain(f"  [dim]• Source archive: {fname}[/dim]")
            t_info = TargetInfo(raw=tarball_url, target_type="direct", clean_url=tarball_url, filename_hint=fname)
            return Aria2Engine.download(t_info, out_dir, connections)

        if download_all:
            print_plain(f"  [dim]• Downloading {len(assets)} release assets concurrently...[/dim]")
            batch_lines = []
            for a in assets:
                batch_lines.append(a["browser_download_url"])
                batch_lines.append(f"  dir={out_dir}")
                batch_lines.append(f"  out={a['name']}")
                batch_lines.append(f"  split={connections}")
                batch_lines.append(f"  max-connection-per-server={connections}")

            with tempfile.NamedTemporaryFile("w", delete=False, prefix="pdl_gh_") as tf:
                tf.write("\n".join(batch_lines) + "\n")
                tmp_file = tf.name

            try:
                cmd = [
                    TOOLCHAIN.aria2c,
                    "-i", tmp_file,
                    f"-j{min(len(assets), 4)}",
                    f"-x{connections}",
                    f"-s{connections}",
                    "-k1M",
                    "--continue=true",
                    "--file-allocation=none",
                    "--console-log-level=warn"
                ]
                res = subprocess.run(cmd)
                return res.returncode == 0, out_dir
            finally:
                if os.path.exists(tmp_file):
                    try: os.remove(tmp_file)
                    except: pass

        candidate_assets = assets
        if asset_pattern:
            candidate_assets = [a for a in assets if re.search(asset_pattern, a["name"], re.I)]

        if not candidate_assets:
            print_plain(f"  [warning]No assets matched '{asset_pattern}'. Showing all:[/warning]")
            candidate_assets = assets
            interactive = True

        scored = sorted(
            [(cls.score_asset_for_host(a["name"]), a) for a in candidate_assets],
            key=lambda x: x[0],
            reverse=True
        )

        chosen_asset = None

        if not interactive and scored and scored[0][0] >= 40:
            chosen_asset = scored[0][1]
            print_plain(f"  [green]✓[/green] [white]Auto-selected:[/white] [bold cyan]{chosen_asset['name']}[/bold cyan] [dim]({format_size(chosen_asset['size'])})[/dim]")
        else:
            table = Table(title=f"Release Assets: {owner}/{repo} ({tag_name})", box=box.HORIZONTALS, pad_edge=False)
            table.add_column("#", justify="right", style="cyan")
            table.add_column("Asset", style="bold white")
            table.add_column("Size", justify="right", style="dim")
            table.add_column("Match", justify="center")

            for idx, (sc, a) in enumerate(scored, 1):
                badge = "[green]Recommended[/green]" if sc >= 60 else ("[cyan]Compatible[/cyan]" if sc >= 30 else "[dim]-[/dim]")
                table.add_row(str(idx), a["name"], format_size(a["size"]), badge)

            if console:
                console.print()
                console.print(table)
                console.print()

            if interactive and console:
                choice = Prompt.ask("  Select asset number", choices=[str(i) for i in range(1, len(scored) + 1)], default="1")
                chosen_asset = scored[int(choice) - 1][1]
            else:
                chosen_asset = scored[0][1]

        target_url = chosen_asset["browser_download_url"]
        fname = chosen_asset["name"]
        t_info = TargetInfo(raw=target_url, target_type="direct", clean_url=target_url, filename_hint=fname, content_length=chosen_asset["size"])
        return Aria2Engine.download(t_info, out_dir, connections)

    @classmethod
    def download_tree_directory(cls, owner: str, repo: str, branch: str, path: str, out_dir: Path, connections: int) -> Tuple[bool, Optional[Path]]:
        print_plain(f"  [dim]•[/dim] [white]GitHub Directory:[/white] [cyan]{path}[/cyan] [dim]({owner}/{repo} @ {branch})[/dim]")
        target_dir = out_dir / os.path.basename(path.rstrip("/"))
        target_dir.mkdir(parents=True, exist_ok=True)

        api_url = f"https://api.github.com/repos/{owner}/{repo}/contents/{path}?ref={branch}"
        headers = cls.get_headers()

        files_to_download = []

        def crawl(url: str, current_local: Path):
            try:
                r = requests.get(url, headers=headers, timeout=6)
                if r.status_code != 200:
                    return
                items = r.json()
                if not isinstance(items, list):
                    return
                for it in items:
                    if it["type"] == "file":
                        files_to_download.append((it["download_url"], current_local, it["name"]))
                    elif it["type"] == "dir":
                        sub_dir = current_local / it["name"]
                        sub_dir.mkdir(parents=True, exist_ok=True)
                        crawl(it["url"], sub_dir)
            except Exception:
                pass

        crawl(api_url, target_dir)

        if not files_to_download:
            print_plain(f"  [error]✗ No files found in directory {path}[/error]")
            return False, None

        print_plain(f"  [dim]• Downloading {len(files_to_download)} files via aria2c ({connections} streams)...[/dim]")
        batch_lines = []
        for dl_url, dest_dir, fname in files_to_download:
            batch_lines.append(dl_url)
            batch_lines.append(f"  dir={dest_dir}")
            batch_lines.append(f"  out={fname}")
            batch_lines.append(f"  split={connections}")
            batch_lines.append(f"  max-connection-per-server={connections}")

        with tempfile.NamedTemporaryFile("w", delete=False, prefix="pdl_gh_tree_") as tf:
            tf.write("\n".join(batch_lines) + "\n")
            tmp_file = tf.name

        try:
            cmd = [
                TOOLCHAIN.aria2c,
                "-i", tmp_file,
                f"-j{min(len(files_to_download), 8)}",
                f"-x{min(connections, 16)}",
                f"-s{connections}",
                "-k1M",
                "--continue=true",
                "--file-allocation=none",
                "--console-log-level=warn"
            ]
            res = subprocess.run(cmd)
            return res.returncode == 0, target_dir
        finally:
            if os.path.exists(tmp_file):
                try: os.remove(tmp_file)
                except: pass

    @classmethod
    def download_repo_archive(cls, owner: str, repo: str, out_dir: Path, connections: int, branch: Optional[str] = None) -> Tuple[bool, Optional[Path]]:
        print_plain(f"  [dim]•[/dim] [white]GitHub Archive:[/white] [cyan]{owner}/{repo}[/cyan]")
        chosen_branch = branch
        if not chosen_branch:
            try:
                r = requests.get(f"https://api.github.com/repos/{owner}/{repo}", headers=cls.get_headers(), timeout=5)
                if r.status_code == 200:
                    chosen_branch = r.json().get("default_branch", "main")
            except Exception:
                chosen_branch = "main"

        archive_url = f"https://github.com/{owner}/{repo}/archive/refs/heads/{chosen_branch}.zip"
        fname = f"{repo}-{chosen_branch}.zip"
        t_info = TargetInfo(raw=archive_url, target_type="direct", clean_url=archive_url, filename_hint=fname)
        return Aria2Engine.download(t_info, out_dir, connections)


# ==============================================================================
# Binary Installer Tool (Direct GitHub release binary setup)
# ==============================================================================
class BinaryInstaller:
    @classmethod
    def install(cls, target: str, bin_dir: Path) -> bool:
        bin_dir = Path(bin_dir).expanduser().resolve()
        bin_dir.mkdir(parents=True, exist_ok=True)

        meta = GitHubEngine.parse_github_target(target)
        owner = meta.get("owner")
        repo = meta.get("repo")

        if not owner or not repo:
            if "/" in target and not target.startswith("http"):
                parts = target.split("/", 1)
                owner, repo = parts[0], parts[1]
            else:
                print_plain(f"  [error]Invalid GitHub repository format for install: {target}[/error]")
                return False

        print_plain(f"  [dim]•[/dim] [white]Installing binary release:[/white] [cyan]{owner}/{repo}[/cyan] -> [dim]{bin_dir}[/dim]")

        with tempfile.TemporaryDirectory(prefix="pdl_install_") as tmp_dir:
            tmp_path = Path(tmp_dir)
            ok, downloaded_asset = GitHubEngine.download_release_asset(
                owner=owner,
                repo=repo,
                tag="latest",
                out_dir=tmp_path,
                connections=16,
                interactive=False
            )

            if not ok or not downloaded_asset or not downloaded_asset.exists():
                print_plain("  [error]Failed to download release asset.[/error]")
                return False

            extract_dir = tmp_path / "extracted"
            extract_dir.mkdir()

            # Extract archive
            if ArchiveExtractor.is_archive(downloaded_asset):
                succ, _, extracted_files = ArchiveExtractor.extract(downloaded_asset, extract_dir)
            else:
                extracted_files = [downloaded_asset]

            # Scan for candidate executable binaries
            installed_any = False
            candidate_binaries = []

            for root, _, files in os.walk(extract_dir if extract_dir.exists() else tmp_path):
                for f in files:
                    fp = Path(root) / f
                    if f.startswith(".") or f.lower().endswith((".md", ".txt", ".json", ".yaml", ".yml", ".sha256")):
                        continue

                    # If already executable or matches repo name
                    is_exec = os.access(fp, os.X_OK)
                    matches_name = f.lower() == repo.lower() or f.lower() == repo.lower().replace("-cli", "")
                    if is_exec or matches_name or not "." in f:
                        candidate_binaries.append(fp)

            if not candidate_binaries:
                print_plain("  [error]No executable binary found in package.[/error]")
                return False

            # Select primary binary (prioritizing repo name match)
            candidate_binaries.sort(key=lambda p: 0 if p.name.lower() == repo.lower() else 1)
            target_bin = candidate_binaries[0]

            dest_bin = bin_dir / target_bin.name
            shutil.copy2(target_bin, dest_bin)
            dest_bin.chmod(0o755)

            # Verification
            ver_text = ""
            try:
                proc = subprocess.run([str(dest_bin), "--version"], capture_output=True, text=True, timeout=2)
                if proc.stdout:
                    ver_text = f" [dim]({proc.stdout.strip().splitlines()[0]})[/dim]"
            except Exception:
                pass

            print_plain(f"  [green]✓[/green] [bold white]Successfully installed [cyan]{dest_bin.name}[/cyan]![/bold white]{ver_text}")
            print_plain(f"    [dim]Binary Location:[/dim] [cyan]{dest_bin}[/cyan]")
            return True


# ==============================================================================
# Apple IPSW Firmware Acceleration Engine
# ==============================================================================
class IpswEngine:
    DEVICE_ALIASES = {
        "iphone 16": "iPhone17,3",
        "iphone 16 plus": "iPhone17,4",
        "iphone 16 pro": "iPhone17,1",
        "iphone 16 pro max": "iPhone17,2",
        "iphone 15": "iPhone15,4",
        "iphone 15 plus": "iPhone15,5",
        "iphone 15 pro": "iPhone16,1",
        "iphone 15 pro max": "iPhone16,2",
        "iphone 14": "iPhone14,7",
        "iphone 14 plus": "iPhone14,8",
        "iphone 14 pro": "iPhone15,2",
        "iphone 14 pro max": "iPhone15,3",
        "iphone 13": "iPhone14,5",
        "iphone 13 mini": "iPhone14,4",
        "iphone 13 pro": "iPhone14,2",
        "iphone 13 pro max": "iPhone14,3",
        "iphone 12": "iPhone13,2",
        "iphone 12 mini": "iPhone13,1",
        "iphone 12 pro": "iPhone13,3",
        "iphone 12 pro max": "iPhone13,4",
        "iphone 11": "iPhone12,1",
        "iphone 11 pro": "iPhone12,3",
        "iphone 11 pro max": "iPhone12,5",
        "iphone se 2": "iPhone12,8",
        "iphone se 3": "iPhone14,6",
    }

    @classmethod
    def is_ipsw(cls, target: str) -> bool:
        t = target.strip().lower()
        if t.startswith("ipsw:") or t.startswith("ipsw/"):
            return True
        if "ipsw.me" in t:
            return True
        if ".ipsw" in t and ("updates.cdn-apple.com" in t or t.startswith("http")):
            return True
        return False

    @classmethod
    def resolve_device_identifier(cls, dev_str: str) -> str:
        s = dev_str.strip()
        if s.lower().startswith("ipsw:"):
            s = s[5:].strip()
        elif s.lower().startswith("ipsw/"):
            s = s[5:].strip()

        clean_lower = s.lower().replace("-", " ").replace("_", " ").strip()
        if clean_lower in cls.DEVICE_ALIASES:
            return cls.DEVICE_ALIASES[clean_lower]

        for alias, ident in cls.DEVICE_ALIASES.items():
            if clean_lower == alias or clean_lower == ident.lower():
                return ident

        return s

    @classmethod
    def resolve(cls, target: str) -> Optional[Dict[str, Any]]:
        target = target.strip()
        if target.startswith("http://") or target.startswith("https://"):
            url_clean = target.split("?")[0].split("#")[0]
            fname = os.path.basename(url_clean)
            m = re.match(r'([A-Za-z0-9,]+)_([0-9.]+)_([A-Za-z0-9]+)_Restore\.ipsw', fname)
            ident = m.group(1) if m else None
            build = m.group(3) if m else None
            meta = {
                "device_name": ident or "Apple Device",
                "identifier": ident,
                "version": m.group(2) if m else "",
                "buildid": build or "",
                "url": target,
                "filename": fname,
                "filesize": None,
                "sha256": None
            }
            if ident and REQUESTS_AVAILABLE:
                try:
                    r = requests.get(f"https://api.ipsw.me/v4/device/{ident}?type=ipsw", timeout=4)
                    if r.status_code == 200:
                        data = r.json()
                        meta["device_name"] = data.get("name", ident)
                        for fw in data.get("firmwares", []):
                            if (build and fw.get("buildid") == build) or fw.get("url") == target:
                                meta["version"] = fw.get("version", meta["version"])
                                meta["buildid"] = fw.get("buildid", meta["buildid"])
                                meta["sha256"] = fw.get("sha256sum")
                                meta["filesize"] = fw.get("filesize")
                                break
                except Exception:
                    pass
            return meta

        ident = cls.resolve_device_identifier(target)
        if REQUESTS_AVAILABLE:
            try:
                r = requests.get(f"https://api.ipsw.me/v4/device/{ident}?type=ipsw", timeout=6)
                if r.status_code == 200:
                    data = r.json()
                    signed = [f for f in data.get("firmwares", []) if f.get("signed")]
                    if signed:
                        latest = signed[0]
                        url = latest["url"]
                        return {
                            "device_name": data.get("name", ident),
                            "identifier": ident,
                            "version": latest.get("version"),
                            "buildid": latest.get("buildid"),
                            "url": url,
                            "filename": os.path.basename(url.split("?")[0]),
                            "filesize": latest.get("filesize"),
                            "sha256": latest.get("sha256sum")
                        }
            except Exception:
                pass
        return None


# ==============================================================================
# Target Classification & Probing
# ==============================================================================
class TargetType:
    TORRENT = "torrent"
    DIRECT = "direct"
    STREAM = "stream"
    SEARCH = "search"
    GITHUB = "github"
    HF = "huggingface"
    IPSW = "ipsw"

@dataclass
class TargetInfo:
    raw: str
    target_type: str
    clean_url: str
    filename_hint: Optional[str] = None
    content_length: Optional[int] = None
    mime_type: Optional[str] = None
    github_meta: Optional[Dict[str, Any]] = None
    hf_meta: Optional[Dict[str, Any]] = None
    ipsw_meta: Optional[Dict[str, Any]] = None

def probe_target(target: str) -> TargetInfo:
    target = target.strip()

    # Apple IPSW Firmware URLs & Shortcuts
    if IpswEngine.is_ipsw(target):
        ipsw_data = IpswEngine.resolve(target)
        if ipsw_data and ipsw_data.get("url"):
            return TargetInfo(
                raw=target,
                target_type=TargetType.IPSW,
                clean_url=ipsw_data["url"],
                filename_hint=ipsw_data.get("filename"),
                content_length=ipsw_data.get("filesize"),
                ipsw_meta=ipsw_data
            )

    # Magnet / Torrent
    if target.startswith("magnet:?") or target.endswith(".torrent"):
        return TargetInfo(raw=target, target_type=TargetType.TORRENT, clean_url=target)

    # Cloud sharing URLs (Google Drive, Dropbox, Pastebin)
    resolved_url, hint = CloudResolver.resolve_url(target)
    if resolved_url != target:
        return TargetInfo(raw=target, target_type=TargetType.DIRECT, clean_url=resolved_url, filename_hint=hint)

    # Hugging Face Hub
    if HuggingFaceEngine.is_hf(target):
        hf_meta = HuggingFaceEngine.parse_hf_target(target)
        if hf_meta.get("is_direct_file"):
            direct_url = f"https://huggingface.co/{hf_meta['repo_id']}/resolve/{hf_meta['revision']}/{hf_meta['subpath']}"
            return TargetInfo(raw=target, target_type=TargetType.DIRECT, clean_url=direct_url, filename_hint=os.path.basename(hf_meta["subpath"]), hf_meta=hf_meta)
        return TargetInfo(raw=target, target_type=TargetType.HF, clean_url=target, hf_meta=hf_meta)

    # GitHub URLs & Shortcuts
    if GitHubEngine.is_github(target):
        gh_meta = GitHubEngine.parse_github_target(target)
        if gh_meta["type"] == "blob":
            return TargetInfo(raw=target, target_type=TargetType.DIRECT, clean_url=gh_meta["url"], filename_hint=gh_meta["filename"], github_meta=gh_meta)
        elif gh_meta["type"] == "asset":
            return TargetInfo(raw=target, target_type=TargetType.DIRECT, clean_url=gh_meta["url"], filename_hint=gh_meta["filename"], github_meta=gh_meta)
        else:
            return TargetInfo(raw=target, target_type=TargetType.GITHUB, clean_url=gh_meta.get("url", target), github_meta=gh_meta)

    # Check generic URL
    if target.startswith("http://") or target.startswith("https://") or target.startswith("ftp://"):
        url_without_params = target.split("?")[0].split("#")[0]
        ext = os.path.splitext(url_without_params)[1].lower()

        # Check known media domains
        domain_match = False
        lower_url = target.lower()
        for domain in MEDIA_DOMAINS:
            if domain in lower_url:
                domain_match = True
                break

        if domain_match:
            return TargetInfo(raw=target, target_type=TargetType.STREAM, clean_url=target)

        # Check file extension
        if ext in DIRECT_EXTENSIONS:
            filename = os.path.basename(url_without_params)
            return TargetInfo(raw=target, target_type=TargetType.DIRECT, clean_url=target, filename_hint=filename)

        # HTTP HEAD Probe for generic URLs
        if REQUESTS_AVAILABLE:
            try:
                headers = {"User-Agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36"}
                resp = requests.head(target, headers=headers, allow_redirects=True, timeout=3.5)
                content_type = resp.headers.get("Content-Type", "").lower()
                content_disp = resp.headers.get("Content-Disposition", "")
                content_len = int(resp.headers.get("Content-Length", 0)) if resp.headers.get("Content-Length") else None

                extracted_name = None
                if content_disp and "filename=" in content_disp:
                    m = re.search(r'filename\*?=(?:UTF-8\'\')?["\']?([^"\';]+)["\']?', content_disp)
                    if m:
                        extracted_name = m.group(1).strip()

                is_direct = False
                if any(x in content_type for x in ["application/", "video/", "audio/", "image/", "octet-stream"]):
                    is_direct = True
                elif "text/html" not in content_type and content_len and content_len > 1024:
                    is_direct = True
                elif resp.headers.get("Accept-Ranges") == "bytes" and "text/html" not in content_type:
                    is_direct = True

                if is_direct:
                    fname = extracted_name or os.path.basename(resp.url.split("?")[0]) or "downloaded_file"
                    return TargetInfo(
                        raw=target,
                        target_type=TargetType.DIRECT,
                        clean_url=resp.url,
                        filename_hint=fname,
                        content_length=content_len,
                        mime_type=content_type
                    )
            except Exception:
                pass

        return TargetInfo(raw=target, target_type=TargetType.STREAM, clean_url=target)

    # Search Query
    return TargetInfo(raw=target, target_type=TargetType.SEARCH, clean_url=f"ytsearch1:{target}")


# ==============================================================================
# Target Inspector Tool (Inspect target metadata without downloading)
# ==============================================================================
class TargetInspector:
    @classmethod
    def inspect(cls, target: str):
        t_info = probe_target(target)
        table = Table(title=f"Target Inspector: {target}", box=box.HORIZONTALS, pad_edge=False)
        table.add_column("Property", style="bold cyan")
        table.add_column("Value", style="white")

        table.add_row("Classification", t_info.target_type.upper())
        table.add_row("Resolved URL", t_info.clean_url)

        if t_info.target_type == TargetType.GITHUB and t_info.github_meta:
            meta = t_info.github_meta
            owner = meta.get("owner", "")
            repo = meta.get("repo", "")
            table.add_row("GitHub Repo", f"{owner}/{repo}")
            table.add_row("Target Kind", meta.get("type", "repo"))
            rel = GitHubEngine.fetch_release_info(owner, repo)
            if rel:
                table.add_row("Latest Tag", rel.get("tag_name", "-"))
                table.add_row("Published At", rel.get("published_at", "-")[:10])
                table.add_row("Asset Count", str(len(rel.get("assets", []))))

        elif t_info.target_type == TargetType.HF and t_info.hf_meta:
            meta = t_info.hf_meta
            repo_id = meta.get("repo_id", "")
            table.add_row("HF Model ID", repo_id)
            info = HuggingFaceEngine.fetch_hf_info(repo_id, meta.get("repo_type", "model"))
            if info:
                table.add_row("Pipeline Type", info.get("pipeline_tag", "-"))
                table.add_row("Downloads", str(info.get("downloads", "-")))
                table.add_row("Files Count", str(len(info.get("siblings", []))))

        elif t_info.target_type == TargetType.STREAM or t_info.target_type == TargetType.SEARCH:
            if TOOLCHAIN.ytdlp:
                cmd = [TOOLCHAIN.ytdlp, *TOOLCHAIN.get_ytdlp_js_args(), "--skip-download", "--dump-json", t_info.clean_url]
                res = subprocess.run(cmd, capture_output=True, text=True)
                if res.returncode == 0 and res.stdout.strip():
                    try:
                        data = json.loads(res.stdout.splitlines()[0])
                        table.add_row("Media Title", data.get("title", "-"))
                        table.add_row("Uploader", data.get("uploader", "-"))
                        table.add_row("Duration", f"{data.get('duration', 0)}s")
                        table.add_row("View Count", str(data.get("view_count", "-")))
                    except Exception:
                        pass

        elif t_info.target_type == TargetType.IPSW and t_info.ipsw_meta:
            meta = t_info.ipsw_meta
            table.add_row("Apple Device", meta.get("device_name", "-"))
            table.add_row("Model Identifier", meta.get("identifier", "-"))
            table.add_row("iOS Version", meta.get("version", "-"))
            table.add_row("Build ID", meta.get("buildid", "-"))
            if meta.get("filesize"):
                table.add_row("File Size", format_size(meta["filesize"]))
            if meta.get("sha256"):
                table.add_row("SHA-256 Hash", meta["sha256"])

        elif t_info.target_type == TargetType.DIRECT:
            if t_info.filename_hint:
                table.add_row("Filename Hint", t_info.filename_hint)
            if t_info.content_length:
                table.add_row("Content Length", format_size(t_info.content_length))
            if t_info.mime_type:
                table.add_row("MIME Type", t_info.mime_type)

        if console:
            console.print()
            console.print(table)
            console.print()


# ==============================================================================
# Aria2 Multi-Channel Direct Engine
# ==============================================================================
class Aria2Engine:
    @staticmethod
    def parse_speed_str(speed_str: Optional[str]) -> float:
        if not speed_str:
            return 0.0
        s = speed_str.strip().upper().replace(' ', '')
        m = re.match(r'^([0-9.]+)\s*([KMGT]?)(?:I?B(?:PS|/S(?:EC)?)?)?$', s)
        if not m:
            return 0.0
        val = float(m.group(1))
        unit = m.group(2)
        mult = {'': 1, 'K': 1024, 'M': 1024**2, 'G': 1024**3, 'T': 1024**4}.get(unit, 1)
        return val * mult

    @classmethod
    def download(
        cls,
        target_info: TargetInfo,
        out_dir: Path,
        connections: int = 16,
        limit_rate: Optional[str] = None,
        anti_throttle: Optional[str] = "4MBps",
        grace_period: int = 15
    ) -> Tuple[bool, Optional[Path]]:
        if not TOOLCHAIN.aria2c:
            print_plain("  [error]Error: aria2c binary not found. Run 'brew install aria2'.[/error]")
            return False, None

        out_dir.mkdir(parents=True, exist_ok=True)

        # aria2c hard limit for connections per server is 16
        safe_conn = min(max(connections, 1), 16)

        cmd = [
            TOOLCHAIN.aria2c,
            f"-s{safe_conn}",
            f"-x{safe_conn}",
            f"-j{safe_conn}",
            "-k1M",
            "--continue=true",
            "--file-allocation=none",
            "--summary-interval=1",
            "--console-log-level=warn",
            "--auto-file-renaming=false",
            "--max-tries=10",
            "--retry-wait=2",
            "--connect-timeout=30",
            "--timeout=60",
            "--remote-time=true",
            "-d", str(out_dir),
        ]

        if limit_rate:
            cmd.append(f"--max-download-limit={limit_rate}")

        if target_info.filename_hint:
            cmd.extend(["-o", target_info.filename_hint])

        cmd.append(target_info.clean_url)

        expected_file = out_dir / (target_info.filename_hint or "download")
        threshold_bps = cls.parse_speed_str(anti_throttle)

        # Direct execution without watchdog if anti_throttle is disabled
        if threshold_bps <= 0:
            try:
                res = subprocess.run(cmd)
                if res.returncode == 0:
                    if expected_file.exists():
                        return True, expected_file
                    candidates = sorted(out_dir.glob("*"), key=os.path.getmtime, reverse=True)
                    if candidates:
                        return True, candidates[0]
                    return True, out_dir
                return False, None
            except Exception as e:
                print_plain(f"  [error]aria2c error: {e}[/error]")
                return False, None

        # Anti-Throttle Supervised Execution Loop (Calibrated via Live Network Trials)
        trigger_str = f"{threshold_bps/(1024*1024):.1f} MB/s" if threshold_bps >= 1024*1024 else f"{threshold_bps/1024:.0f} KB/s"
        print_plain(f"  [dim]• Anti-Throttle Watchdog:[/dim] [yellow]Active (< {trigger_str} trigger)[/yellow]")
        cycle = 1
        effective_grace = max(min(grace_period, 5), 3)

        while True:
            proc = None
            try:
                proc = subprocess.Popen(
                    cmd,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    text=True,
                    bufsize=1
                )
                start_time = time.time()
                low_speed_consecutive = 0
                reconnect_needed = False
                last_speed_reported = 0.0
                warm_up_complete = False

                for line in iter(proc.stdout.readline, ''):
                    if not line:
                        break
                    sys.stdout.write(line)
                    sys.stdout.flush()

                    m = re.search(r'DL:([0-9.]+)\s*([KMGT]?i?B)', line)
                    if m:
                        val = float(m.group(1))
                        unit = m.group(2).upper()
                        mult = {'B': 1, 'KIB': 1024, 'KB': 1024, 'MIB': 1024**2, 'MB': 1024**2, 'GIB': 1024**3, 'GB': 1024**3}.get(unit, 1)
                        speed_bps = val * mult
                        last_speed_reported = speed_bps
                        elapsed = time.time() - start_time

                        # Dynamic warm-up: armed once connection has surpassed the target threshold, or after grace period
                        if speed_bps >= threshold_bps:
                            warm_up_complete = True

                        can_check = warm_up_complete or (elapsed > effective_grace)

                        if can_check:
                            if speed_bps < threshold_bps:
                                low_speed_consecutive += 1
                                # 2 consecutive low samples triggers immediate socket recycling
                                if low_speed_consecutive >= 2:
                                    reconnect_needed = True
                                    break
                            else:
                                low_speed_consecutive = 0

                if reconnect_needed and proc.poll() is None:
                    try:
                        proc.send_signal(signal.SIGINT)
                        proc.wait(timeout=5)
                    except (subprocess.TimeoutExpired, ProcessLookupError):
                        try: proc.kill()
                        except: pass

                    cycle += 1
                    def _fmt_spd(bps: float) -> str:
                        if bps >= 1024 * 1024:
                            return f"{bps / (1024 * 1024):.1f} MB/s"
                        return f"{bps / 1024.0:.1f} KB/s"
                    print_plain("")
                    print_plain(f"  [bold yellow]⚡ [Anti-Throttle Cycle #{cycle}][/bold yellow] [white]Speed throttled to[/white] [bold red]{_fmt_spd(last_speed_reported)}[/bold red] [white](< {_fmt_spd(threshold_bps)} threshold).[/white]")
                    print_plain(f"  [dim]• Recycling {safe_conn} parallel connections to reset ISP burst window...[/dim]")
                    time.sleep(0.5)
                    continue
                else:
                    proc.wait()
                    if proc.returncode == 0:
                        if expected_file.exists():
                            return True, expected_file
                        candidates = sorted(out_dir.glob("*"), key=os.path.getmtime, reverse=True)
                        if candidates:
                            return True, candidates[0]
                        return True, out_dir
                    return False, None
            except KeyboardInterrupt:
                if proc and proc.poll() is None:
                    try:
                        proc.send_signal(signal.SIGINT)
                        proc.wait(timeout=3)
                    except:
                        pass
                return False, None
            except Exception as e:
                print_plain(f"  [error]aria2c error: {e}[/error]")
                return False, None


# ==============================================================================
# Audio & Video Media Engine (yt-dlp + CoreAudio + FFmpeg + Mutagen)
# ==============================================================================
class MediaEngine:
    @staticmethod
    def download(
        target_info: TargetInfo,
        out_dir: Path,
        is_audio: bool = True,
        audio_codec: str = "lossless",
        video_quality: str = "best",
        connections: int = 16,
        custom_format: Optional[str] = None,
        embed_subs: bool = False,
        embed_thumbnail: bool = True
    ) -> Tuple[bool, Optional[Path], Dict[str, Any]]:
        metadata = {}
        if not TOOLCHAIN.ytdlp:
            print_plain("  [error]Error: yt-dlp binary was not found.[/error]")
            return False, None, metadata

        out_dir.mkdir(parents=True, exist_ok=True)
        query = target_info.clean_url

        with Status("[dim]  Resolving media metadata...[/dim]", console=console) if console else open(os.devnull) as _:
            info_cmd = [
                TOOLCHAIN.ytdlp,
                *TOOLCHAIN.get_ytdlp_js_args(),
                "--extractor-args", "youtube:player-client=android",
                "--skip-download",
                "--dump-json",
                query
            ]
            info_res = subprocess.run(info_cmd, capture_output=True, text=True)

        if info_res.returncode == 0 and info_res.stdout.strip():
            try:
                json_lines = [l for l in info_res.stdout.splitlines() if l.strip()]
                if json_lines:
                    data = json.loads(json_lines[0])
                    metadata["title"] = data.get("title", "Unknown_Track")
                    metadata["id"] = data.get("id", "track")
                    metadata["uploader"] = data.get("uploader", "Unknown Artist")
                    metadata["duration"] = data.get("duration", 0)
                    metadata["thumbnail"] = data.get("thumbnail")
                    metadata["filesize"] = data.get("filesize") or data.get("filesize_approx")
            except Exception:
                pass

        title = metadata.get("title") or "Track"
        video_id = metadata.get("id") or f"id_{int(time.time())}"
        safe_title = sanitize_filename(title)

        sz_str = f" [dim]({format_size(metadata.get('filesize'))})[/dim]" if metadata.get('filesize') else ""
        print_plain(f"  [green]✓[/green] [bold white]{title}[/bold white]{sz_str}")

        # Format string resolution
        if custom_format:
            fmt = custom_format
        elif is_audio:
            fmt = "ba[ext=m4a]/ba[ext=aac]/ba/b"
        else:
            if video_quality == "4k":
                fmt = "bv*[height<=2160]+ba/b[height<=2160]/best"
            elif video_quality == "1440p":
                fmt = "bv*[height<=1440]+ba/b[height<=1440]/best"
            elif video_quality == "1080p":
                fmt = "bv*[height<=1080]+ba/b[height<=1080]/best"
            elif video_quality == "720p":
                fmt = "bv*[height<=720]+ba/b[height<=720]/best"
            elif video_quality == "480p":
                fmt = "bv*[height<=480]+ba/b[height<=480]/best"
            else:
                if TOOLCHAIN.ffmpeg:
                    fmt = "bv*[ext=mp4]+ba[ext=m4a]/b[ext=mp4]/bv*+ba/b"
                else:
                    fmt = "b[ext=mp4]/18/best"

        raw_tmp = out_dir / f".pdl_raw_{video_id}_{os.getpid()}"
        ACTIVE_TEMP_FILES.add(str(raw_tmp))

        dl_cmd = [
            TOOLCHAIN.ytdlp,
            *TOOLCHAIN.get_ytdlp_js_args(),
            "--extractor-args", "youtube:player-client=android",
            "-f", fmt,
            "-o", f"{raw_tmp}.%(ext)s",
            "--no-playlist",
        ]

        if embed_subs:
            dl_cmd.extend(["--write-subs", "--write-auto-subs", "--sub-langs", "en.*,all"])

        if embed_thumbnail:
            dl_cmd.append("--write-thumbnail")

        if TOOLCHAIN.aria2c:
            dl_cmd.extend([
                "--external-downloader", TOOLCHAIN.aria2c,
                "--external-downloader-args",
                f"aria2c:-s {connections} -x {min(connections, 16)} -k 1M -j {connections} --continue=true --file-allocation=none --console-log-level=warn"
            ])

        dl_cmd.append(query)

        print_plain(f"  [dim]• Downloading via aria2c ({connections} streams)...[/dim]")
        dl_res = subprocess.run(dl_cmd)

        matched_files = [f for f in out_dir.glob(f".pdl_raw_{video_id}_{os.getpid()}.*") if not f.name.endswith(('.jpg', '.png', '.webp', '.vtt', '.srt'))]
        if dl_res.returncode != 0 or not matched_files:
            cleanup_temp_files()
            return False, None, metadata

        downloaded_raw = matched_files[0]
        ext = downloaded_raw.suffix.lower()

        final_path = None

        if is_audio:
            # Codec-based audio handling
            if audio_codec == "mp3" and TOOLCHAIN.ffmpeg:
                final_path = out_dir / f"{safe_title}.mp3"
                cmd = [TOOLCHAIN.ffmpeg, "-y", "-i", str(downloaded_raw), "-vn", "-c:a", "libmp3lame", "-b:a", "320k", str(final_path)]
                subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            elif audio_codec == "flac" and TOOLCHAIN.ffmpeg:
                final_path = out_dir / f"{safe_title}.flac"
                cmd = [TOOLCHAIN.ffmpeg, "-y", "-i", str(downloaded_raw), "-vn", "-c:a", "flac", str(final_path)]
                subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            elif audio_codec == "opus" and TOOLCHAIN.ffmpeg:
                final_path = out_dir / f"{safe_title}.opus"
                cmd = [TOOLCHAIN.ffmpeg, "-y", "-i", str(downloaded_raw), "-vn", "-c:a", "libopus", "-b:a", "160k", str(final_path)]
                subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            elif audio_codec == "wav" and TOOLCHAIN.ffmpeg:
                final_path = out_dir / f"{safe_title}.wav"
                cmd = [TOOLCHAIN.ffmpeg, "-y", "-i", str(downloaded_raw), "-vn", "-c:a", "pcm_s16le", str(final_path)]
                subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            else:
                # Default lossless CoreAudio ALAC / bit-perfect AAC
                final_path = out_dir / f"{safe_title}.m4a"
                extracted = False
                if TOOLCHAIN.afconvert:
                    # Lossless direct packet copy (-d 0)
                    conv_cmd = [TOOLCHAIN.afconvert, "-f", "m4af", "-d", "0", str(downloaded_raw), str(final_path)]
                    conv_res = subprocess.run(conv_cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    if conv_res.returncode == 0 and final_path.exists() and final_path.stat().st_size > 50000:
                        extracted = True

                    if not extracted:
                        conv_cmd = [TOOLCHAIN.afconvert, "-f", "m4af", "-d", "alac", str(downloaded_raw), str(final_path)]
                        conv_res = subprocess.run(conv_cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                        if conv_res.returncode == 0 and final_path.exists() and final_path.stat().st_size > 50000:
                            extracted = True

                if not extracted:
                    final_path = out_dir / f"{safe_title}{ext}"
                    shutil.move(str(downloaded_raw), str(final_path))
                else:
                    if downloaded_raw.exists():
                        try: downloaded_raw.unlink()
                        except: pass

            # ID3 Tagging for M4A / MP3
            if MUTAGEN_AVAILABLE and final_path and final_path.exists():
                try:
                    artist = metadata.get("uploader", "")
                    track_name = title
                    if " - " in title:
                        parts = title.split(" - ", 1)
                        artist = parts[0].strip()
                        track_name = parts[1].strip()

                    if final_path.suffix.lower() == ".m4a":
                        audio = MP4(str(final_path))
                        audio["\xa9nam"] = [track_name]
                        if artist: audio["\xa9ART"] = [artist]
                        audio.save()
                    print_plain("  [dim]• Tagged audio metadata[/dim]")
                except Exception:
                    pass
        else:
            final_path = out_dir / f"{safe_title}{ext if ext != '' else '.mp4'}"
            shutil.move(str(downloaded_raw), str(final_path))

        cleanup_temp_files()
        return True, final_path, metadata


# ==============================================================================
# Android ADB Device Synchronization
# ==============================================================================
class ADBDeviceSync:
    @staticmethod
    def get_connected_devices() -> List[Dict[str, str]]:
        if not TOOLCHAIN.adb:
            return []
        try:
            res = subprocess.run([TOOLCHAIN.adb, "devices", "-l"], capture_output=True, text=True)
            devices = []
            for line in res.stdout.splitlines()[1:]:
                line = line.strip()
                if not line or "offline" in line:
                    continue
                parts = line.split()
                serial = parts[0]
                status = parts[1]
                model = "Android Device"
                for p in parts[2:]:
                    if p.startswith("model:"):
                        model = p.split("model:")[1].replace("_", " ")
                devices.append({"serial": serial, "status": status, "model": model})
            return devices
        except Exception:
            return []

    @staticmethod
    def push_file(file_path: Path, target_dir: str = "/storage/emulated/0/Music") -> bool:
        if not TOOLCHAIN.adb:
            print_plain("  [warning]ADB is not installed or not in PATH.[/warning]")
            return False

        devices = ADBDeviceSync.get_connected_devices()
        if not devices:
            print_plain("  [warning]No active Android devices detected via ADB.[/warning]")
            return False

        target_dev = devices[0]
        if target_dev["status"] != "device":
            print_plain(f"  [warning]Device {target_dev['serial']} is {target_dev['status']}. Confirm USB debugging prompt on phone.[/warning]")
            return False

        print_plain(f"  [dim]• Syncing to {target_dev['model']}...[/dim]")
        push_cmd = [TOOLCHAIN.adb, "-s", target_dev["serial"], "push", str(file_path), f"{target_dir}/"]
        p_res = subprocess.run(push_cmd)

        if p_res.returncode == 0:
            remote_path = f"{target_dir}/{file_path.name}"
            subprocess.run([
                TOOLCHAIN.adb, "-s", target_dev["serial"], "shell", "am", "broadcast",
                "-a", "android.intent.action.MEDIA_SCANNER_SCAN_FILE",
                "-d", f"file://{remote_path}"
            ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            print_plain(f"  [green]✓[/green] [white]Synced to[/white] {target_dev['model']}")
            return True
        else:
            print_plain("  [error]✗ ADB push failed. Check phone connection.[/error]")
            return False


# ==============================================================================
# Main Application Orchestrator
# ==============================================================================
class PDLApp:
    def __init__(self, config: PDLConfig):
        self.config = config

    def render_banner(self):
        if not console:
            return

        header = Text()
        header.append("pdl ", style="bold bright_cyan")
        header.append(f"v{VERSION}", style="dim")
        header.append("  •  parallel multi-channel accelerator", style="dim white")
        console.print()
        console.print(header)
        console.print(Text("─" * 48, style="dim"))

    def print_diagnostics(self):
        table = Table(title="System Toolchain Diagnostics", box=box.HORIZONTALS, pad_edge=False)
        table.add_column("Tool / Engine", style="bold white")
        table.add_column("Status", style="bold")
        table.add_column("Path / Details", style="dim")

        def status_row(name, path, req=True):
            if path:
                table.add_row(name, "[green]ready[/green]", path)
            else:
                st = "[red]missing[/red]" if req else "[dim]optional[/dim]"
                table.add_row(name, st, "not found")

        status_row("aria2c (Multi-Channel Engine)", TOOLCHAIN.aria2c, True)
        status_row("GitHub PowerSuite", "ready" if REQUESTS_AVAILABLE else "degraded", True)
        status_row("Hugging Face Hub Engine", "ready" if REQUESTS_AVAILABLE else "degraded", True)
        status_row("yt-dlp (Media Resolver)", TOOLCHAIN.ytdlp, True)
        status_row("JS Runtime (yt-dlp EJS)", TOOLCHAIN.js_runtime, False)
        status_row("afconvert (CoreAudio Lossless)", TOOLCHAIN.afconvert, False)
        status_row("ffmpeg (AV Transcoder)", TOOLCHAIN.ffmpeg, False)
        status_row("hdiutil (macOS DMG Engine)", TOOLCHAIN.hdiutil, False)
        status_row("adb (Android Sync)", TOOLCHAIN.adb, False)

        console.print()
        console.print(table)
        console.print()

    def clean_orphans(self, target_dir: Path):
        target_dir = Path(target_dir).expanduser().resolve()
        patterns = ["*.aria2", ".pdl_raw_*", "*.part", "*.ytdl"]
        found = []
        for pat in patterns:
            found.extend(list(target_dir.glob(pat)))

        if not found:
            print_plain(f"  [green]✓ Clean![/green] [dim]No temporary or orphaned download files in {target_dir}[/dim]")
            return

        removed = 0
        for f in found:
            try:
                if f.is_dir(): shutil.rmtree(f, ignore_errors=True)
                else: f.unlink()
                removed += 1
            except Exception:
                pass
        print_plain(f"  [green]✓ Cleaned {removed} orphaned file(s) in {target_dir}[/green]")

    def handle_config_command(self, action: Optional[str], key: Optional[str], value: Optional[str]):
        if not action or action == "show":
            table = Table(title="pdl Configuration", box=box.HORIZONTALS, pad_edge=False)
            table.add_column("Setting", style="bold cyan")
            table.add_column("Value", style="white")
            for k, v in self.config.__dict__.items():
                table.add_row(k, str(v))
            if console:
                console.print()
                console.print(table)
                console.print()
        elif action == "set" and key and value is not None:
            if self.config.set(key, value):
                print_plain(f"  [green]✓ Updated [bold]{key}[/bold] -> {value}[/green]")
            else:
                print_plain(f"  [error]Unknown config key: {key}[/error]")
        elif action == "reset":
            self.config = PDLConfig()
            self.config.save()
            print_plain("  [green]✓ Configuration reset to defaults.[/green]")

    def execute(self, args: argparse.Namespace):
        if not getattr(args, "quiet", False):
            self.render_banner()

        if getattr(args, "diagnose", False):
            self.print_diagnostics()
            return

        if getattr(args, "history", False):
            HistoryManager.display(limit=args.history_limit or 15)
            return

        if getattr(args, "clear_history", False):
            HistoryManager.clear()
            return

        if getattr(args, "clean", False):
            self.clean_orphans(Path(args.output or self.config.output_dir))
            return

        if getattr(args, "info_target", None):
            TargetInspector.inspect(args.info_target)
            return

        if getattr(args, "hash_file", None):
            ChecksumEngine.display_hash_table(Path(args.hash_file))
            return

        if getattr(args, "install_target", None):
            BinaryInstaller.install(args.install_target, Path(self.config.bin_dir))
            return

        targets = list(args.targets)
        if args.input_file:
            inp_path = Path(args.input_file).expanduser().resolve()
            if not inp_path.exists():
                print_plain(f"  [error]Error: Input file '{args.input_file}' not found.[/error]")
                sys.exit(1)
            with open(inp_path, "r") as f:
                for line in f:
                    line = line.strip()
                    if line and not line.startswith("#"):
                        targets.append(line)

        if not targets:
            print_plain("  [yellow]No targets specified. Use 'pdl --help' for recipes & options.[/yellow]\n")
            return

        out_dir = Path(args.output or self.config.output_dir).expanduser().resolve()
        raw_conn = args.connections or self.config.connections
        connections = min(max(raw_conn, 1), 16)
        if raw_conn > 16:
            print_plain(f"  [dim]• Note: {raw_conn} streams requested; automatically clamped to maximum {connections} allowed by aria2.[/dim]")
        is_audio = not args.video
        audio_codec = args.audio_codec or ("lossless" if not args.no_lossless else "aac")
        video_quality = args.quality or self.config.video_quality
        auto_extract = args.extract or self.config.auto_extract
        push_adb = args.push_adb or self.config.auto_adb_push

        results = []
        total_items = len(targets)

        for idx, target in enumerate(targets, 1):
            if total_items > 1:
                print_plain(f"\n  [accent]── [{idx}/{total_items}][/accent] [bold white]{target}[/bold white]")

            t0 = time.time()
            t_info = probe_target(target)
            success = False
            out_file = None

            # Hugging Face Model / Dataset
            if t_info.target_type == TargetType.HF:
                success, out_file = HuggingFaceEngine.download(
                    meta=t_info.hf_meta,
                    out_dir=out_dir,
                    connections=connections,
                    pattern=args.asset,
                    interactive=args.select
                )

            # GitHub Release / Tree / Repo Archive
            elif t_info.target_type == TargetType.GITHUB:
                meta = t_info.github_meta or {}
                mtype = meta.get("type")
                if mtype == "release":
                    success, out_file = GitHubEngine.download_release_asset(
                        owner=meta["owner"],
                        repo=meta["repo"],
                        tag=meta["tag"],
                        out_dir=out_dir,
                        connections=connections,
                        asset_pattern=args.asset,
                        download_all=args.all_assets,
                        interactive=args.select
                    )
                elif mtype == "tree":
                    success, out_file = GitHubEngine.download_tree_directory(
                        owner=meta["owner"],
                        repo=meta["repo"],
                        branch=meta["branch"],
                        path=meta["path"],
                        out_dir=out_dir,
                        connections=connections
                    )
                elif mtype == "repo":
                    rel = GitHubEngine.fetch_release_info(meta["owner"], meta["repo"])
                    if rel and rel.get("assets") and not args.source:
                        success, out_file = GitHubEngine.download_release_asset(
                            owner=meta["owner"],
                            repo=meta["repo"],
                            tag=rel.get("tag_name", "latest"),
                            out_dir=out_dir,
                            connections=connections,
                            asset_pattern=args.asset,
                            download_all=args.all_assets,
                            interactive=args.select
                        )
                    else:
                        success, out_file = GitHubEngine.download_repo_archive(
                            owner=meta["owner"],
                            repo=meta["repo"],
                            out_dir=out_dir,
                            connections=connections,
                            branch=args.branch
                        )
                else:
                    success, out_file = Aria2Engine.download(t_info, out_dir, connections, args.limit_rate, anti_throttle=args.anti_throttle, grace_period=args.anti_throttle_grace)

            # Apple IPSW Firmware
            elif t_info.target_type == TargetType.IPSW:
                meta = t_info.ipsw_meta or {}
                dev_name = meta.get("device_name", "Apple Device")
                ios_ver = meta.get("version", "")
                build_id = meta.get("buildid", "")
                print_plain(f"  [dim]•[/dim] [white]Apple Device:[/white] [bold cyan]{dev_name}[/bold cyan] [dim]({meta.get('identifier', '')})[/dim]")
                if ios_ver:
                    print_plain(f"  [dim]•[/dim] [white]Firmware:[/white]     [green]iOS {ios_ver}[/green] [dim](Build {build_id}) • Apple Signed[/dim]")
                if meta.get("filesize"):
                    print_plain(f"  [dim]•[/dim] [white]File Size:[/white]    [white]{format_size(meta['filesize'])}[/white]")
                print_plain(f"  [dim]•[/dim] [white]Streams:[/white]      [bold cyan]{connections}[/bold cyan] parallel connections (maximum 16 allowed)")
                print_plain(f"  [dim]•[/dim] [white]Output:[/white]       [dim]{out_dir}[/dim]")
                success, out_file = Aria2Engine.download(
                    target_info=t_info,
                    out_dir=out_dir,
                    connections=connections,
                    limit_rate=args.limit_rate,
                    anti_throttle=args.anti_throttle,
                    grace_period=args.anti_throttle_grace
                )
                if success and out_file and os.path.isfile(out_file) and not args.checksum and meta.get("sha256"):
                    print_plain(f"  [dim]•[/dim] [white]Verifying official Apple SHA-256 checksum...[/white]")
                    matched, algo, digest = ChecksumEngine.verify(out_file, f"sha256:{meta['sha256']}")
                    if matched:
                        print_plain(f"  [green]✓ Firmware integrity verified against Apple TSS catalog (SHA-256)![/green]")
                    else:
                        print_plain(f"  [error]✗ Checksum mismatch! Expected {meta['sha256']}, got {digest}[/error]")

            # Direct HTTP/HTTPS or Magnet/Torrent
            elif t_info.target_type == TargetType.DIRECT or t_info.target_type == TargetType.TORRENT:
                print_plain(f"  [dim]•[/dim] [white]Source:[/white]   [cyan]{target}[/cyan]")
                print_plain(f"  [dim]•[/dim] [white]Streams:[/white]  [bold cyan]{connections}[/bold cyan] parallel connections (maximum 16 allowed)")
                print_plain(f"  [dim]•[/dim] [white]Output:[/white]   [dim]{out_dir}[/dim]")
                success, out_file = Aria2Engine.download(
                    target_info=t_info,
                    out_dir=out_dir,
                    connections=connections,
                    limit_rate=args.limit_rate,
                    anti_throttle=args.anti_throttle,
                    grace_period=args.anti_throttle_grace
                )

            # Media Stream / Search Query
            else:
                mode_str = f"Audio ({audio_codec.upper()})" if is_audio else f"Video ({video_quality.upper()})"
                print_plain(f"  [dim]•[/dim] [white]Target:[/white]   [bold white]{target}[/bold white]")
                print_plain(f"  [dim]•[/dim] [white]Mode:[/white]     [dim]{mode_str} • {connections} streams[/dim]")
                success, out_file, _ = MediaEngine.download(
                    target_info=t_info,
                    out_dir=out_dir,
                    is_audio=is_audio,
                    audio_codec=audio_codec,
                    video_quality=video_quality,
                    connections=connections,
                    custom_format=args.format,
                    embed_subs=args.subs,
                    embed_thumbnail=args.thumbnail
                )

            # Checksum Verification
            if success and out_file and os.path.isfile(out_file):
                if args.checksum:
                    matched, algo, digest = ChecksumEngine.verify(out_file, args.checksum)
                    if matched:
                        print_plain(f"  [green]✓ Checksum verified ({algo})[/green]")
                    else:
                        print_plain(f"  [error]✗ Checksum mismatch! Expected {args.checksum}, got {digest}[/error]")
                elif args.verify:
                    companion = ChecksumEngine.find_companion_checksum(out_file)
                    if companion:
                        algo, exp = companion
                        matched, _, digest = ChecksumEngine.verify(out_file, f"{algo}:{exp}")
                        if matched:
                            print_plain(f"  [green]✓ Companion checksum verified ({algo})[/green]")
                        else:
                            print_plain(f"  [error]✗ Companion checksum mismatch![/error]")

            # Auto-Archive Extraction
            if success and out_file and auto_extract and ArchiveExtractor.is_archive(out_file):
                ext_dir = out_dir / out_file.stem
                succ, _, files = ArchiveExtractor.extract(out_file, ext_dir)
                if succ:
                    out_file = ext_dir

            # Mobile ADB Sync if requested
            adb_pushed = False
            if success and out_file and push_adb:
                dest_phone_dir = self.config.adb_music_dir if is_audio else self.config.adb_download_dir
                if os.path.isfile(out_file):
                    adb_pushed = ADBDeviceSync.push_file(out_file, target_dir=dest_phone_dir)

            elapsed = time.time() - t0
            file_sz = os.path.getsize(out_file) if out_file and os.path.isfile(out_file) else None
            HistoryManager.record(
                target=target,
                target_type=t_info.target_type,
                file_path=str(out_file) if out_file else None,
                size=file_sz,
                duration=elapsed,
                success=success,
                adb=adb_pushed
            )

            results.append({
                "target": target,
                "success": success,
                "file": out_file,
                "adb": adb_pushed
            })

        self.render_summary(results)

    def render_summary(self, results: List[Dict[str, Any]]):
        if not console:
            return

        if len(results) == 1:
            res = results[0]
            console.print()
            if res["success"]:
                out_f = res["file"]
                size_str = ""
                fname = ""
                if out_f and os.path.exists(out_f):
                    fname = os.path.basename(out_f)
                    if os.path.isfile(out_f):
                        size_str = f" [dim]({format_size(os.path.getsize(out_f))})[/dim]"
                
                console.print(f"  [bold green]✓ Complete[/bold green]")
                if fname:
                    console.print(f"    [dim]File:[/dim]  [bold white]{fname}[/bold white]{size_str}")
                    console.print(f"    [dim]Path:[/dim]  [cyan]{out_f}[/cyan]")
                if res.get("adb"):
                    console.print(f"    [dim]Sync:[/dim]  [green]Copied to Android device[/green]")
            else:
                console.print(f"  [bold red]✗ Download failed[/bold red]: [dim]{res['target']}[/dim]")
            console.print()
            return

        table = Table(title="Batch Summary", box=box.HORIZONTALS, pad_edge=False, expand=True)
        table.add_column("Target", style="dim", no_wrap=True, ratio=3)
        table.add_column("Status", justify="center", ratio=1)
        table.add_column("Filename", style="bold white", ratio=4)
        table.add_column("Size", justify="right", style="dim cyan", ratio=1)
        table.add_column("Device", justify="center", style="dim", ratio=1)

        for res in results:
            st = "[green]✓ ok[/green]" if res["success"] else "[red]✗ error[/red]"
            out_f = res["file"]
            size_str = "-"
            file_name_str = "-"
            if out_f and os.path.exists(out_f):
                file_name_str = os.path.basename(out_f)
                if os.path.isfile(out_f):
                    size_str = format_size(os.path.getsize(out_f))
                else:
                    size_str = "dir"

            adb_status = "[green]synced[/green]" if res["adb"] else "[dim]-[/dim]"
            table.add_row(res["target"][:35], st, file_name_str[:38], size_str, adb_status)

        console.print()
        console.print(table)
        console.print()


# ==============================================================================
# CLI Entrypoint Definition
# ==============================================================================
def main():
    config = PDLConfig.load()

    # Pre-parse subcommands for ergonomic invocation:
    # e.g. pdl install <repo>, pdl info <target>, pdl hash <file>, pdl extract <file>, pdl clean, pdl history
    if len(sys.argv) > 1:
        sub = sys.argv[1].lower()
        if sub == "install" and len(sys.argv) > 2:
            BinaryInstaller.install(sys.argv[2], Path(config.bin_dir))
            sys.exit(0)
        elif sub == "info" and len(sys.argv) > 2:
            TargetInspector.inspect(sys.argv[2])
            sys.exit(0)
        elif sub == "hash" and len(sys.argv) > 2:
            ChecksumEngine.display_hash_table(Path(sys.argv[2]))
            sys.exit(0)
        elif sub == "extract" and len(sys.argv) > 2:
            target_out = Path(sys.argv[3]) if len(sys.argv) > 3 else Path(config.output_dir)
            ArchiveExtractor.extract(Path(sys.argv[2]), target_out)
            sys.exit(0)
        elif sub == "history":
            HistoryManager.display(limit=int(sys.argv[2]) if len(sys.argv) > 2 else 15)
            sys.exit(0)
        elif sub == "clean":
            target_out = Path(sys.argv[2]) if len(sys.argv) > 2 else Path(config.output_dir)
            app = PDLApp(config=config)
            app.clean_orphans(target_out)
            sys.exit(0)
        elif sub == "config":
            app = PDLApp(config=config)
            action = sys.argv[2] if len(sys.argv) > 2 else "show"
            k = sys.argv[3] if len(sys.argv) > 3 else None
            v = sys.argv[4] if len(sys.argv) > 4 else None
            app.handle_config_command(action, k, v)
            sys.exit(0)
        elif sub == "diagnose":
            app = PDLApp(config=config)
            app.print_diagnostics()
            sys.exit(0)

    parser = argparse.ArgumentParser(
        prog="parallel-dl",
        description="High-Performance Parallel Multichannel Downloader Suite [pdl].",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Recipes:
  pdl "Song Title Artist Name" -o ~/Music
  pdl --mp3 "Artist Name Track" -o ~/Music
  pdl --quality 4k "https://www.youtube.com/watch?v=..."
  pdl gh:BurntSushi/ripgrep
  pdl install astral-sh/uv
  pdl hf:TheBloke/Llama-2-7B-GGUF --asset "*Q4_K_M.gguf"
  pdl ipsw:iPhone14,5 -c 32 -o ~/Downloads
  pdl "https://releases.ubuntu.com/24.04/ubuntu-24.04.1-desktop-amd64.iso" -c 32
  pdl info "https://example.com/huge-file.iso"
  pdl hash ./downloaded.tar.gz
  pdl history
        """
    )

    parser.add_argument("targets", nargs="*", help="GitHub URLs/shortcuts, HuggingFace IDs (hf:), direct links, magnet URIs, or media queries.")
    parser.add_argument("-a", "--audio", action="store_true", default=True, help="Extract lossless audio track for media (default: True).")
    parser.add_argument("-v", "--video", action="store_true", help="Download complete high-res video (1080p/4K) instead of audio.")
    parser.add_argument("-l", "--lossless", action="store_true", default=True, help="Lossless ALAC stream extraction via CoreAudio (default: True).")
    parser.add_argument("--no-lossless", action="store_true", help="Disable lossless ALAC mastering, keep standard AAC.")
    parser.add_argument("--audio-codec", choices=["lossless", "mp3", "flac", "opus", "wav", "aac"], help="Audio codec/format to export.")
    parser.add_argument("-Q", "--quality", choices=["best", "4k", "1440p", "1080p", "720p", "480p"], help="Video quality resolution preset.")
    parser.add_argument("--subs", action="store_true", help="Download and embed subtitles into media.")
    parser.add_argument("--no-thumbnail", dest="thumbnail", action="store_false", default=True, help="Do not write thumbnail image.")

    parser.add_argument("-c", "--connections", type=int, default=config.connections, help=f"Parallel connection streams for aria2c (1-16, maximum allowed: 16, default: {min(config.connections, 16)}).")
    parser.add_argument("-o", "--output", default=config.output_dir, help=f"Output destination directory (default: '{config.output_dir}').")
    parser.add_argument("-i", "--input-file", help="File containing list of URLs/queries (one per line).")
    parser.add_argument("-x", "--extract", action="store_true", help="Automatically unpack downloaded archives (.zip, .tar.gz, .dmg, etc.).")
    parser.add_argument("--checksum", help="Expected cryptographic checksum in format [algo:]<hash> to verify upon completion.")
    parser.add_argument("--verify", action="store_true", help="Automatically discover and verify companion .sha256/.md5 checksum files.")
    parser.add_argument("-f", "--format", help="Custom yt-dlp format specification.")
    parser.add_argument("-q", "--quiet", action="store_true", help="Quiet mode, suppress banner.")
    parser.add_argument("--limit-rate", help="Maximum download speed limit (e.g. 5M, 500K).")
    default_anti_throttle = getattr(config, "anti_throttle", "4MBps") or "4MBps"
    default_grace = getattr(config, "anti_throttle_grace", 5) or 5
    parser.add_argument("--anti-throttle", nargs="?", const="4MBps", default=default_anti_throttle, help=f"Auto-recycle connections when download speed drops below threshold (default: {default_anti_throttle}).")
    parser.add_argument("--no-anti-throttle", dest="anti_throttle", action="store_const", const=None, help="Disable anti-throttle connection recycling.")
    parser.add_argument("--anti-throttle-grace", type=int, default=default_grace, help=f"Grace period in seconds before checking speed threshold (default: {default_grace}).")
    parser.add_argument("--push", "--adb", action="store_true", dest="push_adb", default=config.auto_adb_push, help="Automatically push completed files to connected Android phone via ADB.")
    
    # GitHub & HuggingFace Specific Flags
    parser.add_argument("--asset", help="Filter release/HF asset by pattern/extension (e.g. dmg, tar.gz, arm64, *.gguf).")
    parser.add_argument("--all-assets", action="store_true", help="Download all assets in the release concurrently.")
    parser.add_argument("--select", action="store_true", help="Interactive table to choose which asset or model file to download.")
    parser.add_argument("--source", action="store_true", help="Force download repo source archive instead of release binary.")
    parser.add_argument("--branch", help="Specify branch when downloading GitHub directory or repository archive.")

    # Tool Flags
    parser.add_argument("--install", dest="install_target", help="Download, extract, and install GitHub release binary into local bin path.")
    parser.add_argument("--info", dest="info_target", help="Inspect target metadata without downloading.")
    parser.add_argument("--hash", dest="hash_file", help="Compute cryptographic hashes (MD5, SHA1, SHA256, SHA512, BLAKE2b) for a file.")
    parser.add_argument("--history", action="store_true", help="Display recent download history.")
    parser.add_argument("--history-limit", type=int, default=15, help="Number of history entries to show (default: 15).")
    parser.add_argument("--clear-history", action="store_true", help="Clear the download history log.")
    parser.add_argument("--clean", action="store_true", help="Clean orphaned temporary download files (.aria2, .part).")
    parser.add_argument("--diagnose", action="store_true", help="Print system toolchain diagnostics and installed engines.")
    parser.add_argument("--version", action="version", version=f"parallel-dl (pdl) v{VERSION}")

    args = parser.parse_args()

    app = PDLApp(config=config)
    app.execute(args)

if __name__ == "__main__":
    main()
