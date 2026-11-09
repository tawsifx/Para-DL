"""Cryptographic checksum computation, companion file discovery, and integrity verification."""

from __future__ import annotations

import hashlib
import logging
import mmap
import re
import sys
import time
from pathlib import Path
from typing import Any

from pdl.models import format_size
from pdl.ui import UI, print_plain

logger = logging.getLogger(__name__)

SUPPORTED_ALGOS = ("md5", "sha1", "sha256", "sha512", "blake2b")

_BSD_CHECKSUM_RE = re.compile(
    r"^(MD5|SHA1|SHA256|SHA-256|SHA512|SHA-512|BLAKE2b)\s*\(([^)]+)\)\s*=\s*([0-9a-fA-F]+)$",
    re.IGNORECASE,
)

# Minimum file size (bytes) at which mmap is preferred for checksum verification.
MMAP_CHECKSUM_THRESHOLD = 100 * 1024 * 1024  # 100 MiB


class ChecksumEngine:
    """Streaming cryptographic hash calculator and verifier."""

    @staticmethod
    def compute_hashes(file_path: Path, algos: list[str] | tuple[str, ...] = ("sha256", "md5")) -> dict[str, str]:
        hashers: dict[str, Any] = {}
        for a in algos:
            a_lower = a.lower().replace("-", "")
            if a_lower == "sha256":
                hashers["sha256"] = hashlib.sha256()
            elif a_lower == "sha512":
                hashers["sha512"] = hashlib.sha512()
            elif a_lower == "sha1":
                hashers["sha1"] = hashlib.sha1(usedforsecurity=False)
            elif a_lower == "md5":
                hashers["md5"] = hashlib.md5(usedforsecurity=False)
            elif a_lower == "blake2b":
                hashers["blake2b"] = hashlib.blake2b()
            else:
                raise ValueError(f"Unsupported hash algorithm: {a}")

        file_size = file_path.stat().st_size
        use_mmap = file_size >= MMAP_CHECKSUM_THRESHOLD

        if use_mmap:
            # mmap avoids userspace copies for large files; kernel page cache handles I/O.
            with open(file_path, "rb") as f:
                with mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ) as mm:
                    for h in hashers.values():
                        h.update(mm)
        else:
            buf_size = 1024 * 1024  # 1 MiB streaming buffer
            with open(file_path, "rb") as f:
                while chunk := f.read(buf_size):
                    for h in hashers.values():
                        h.update(chunk)

        return {name: h.hexdigest() for name, h in hashers.items()}

    @classmethod
    def verify(cls, file_path: Path, expected: str) -> tuple[bool, str, str]:
        """Verify file_path against expected '[algo:]<hex_digest>'.

        Returns (matched, algorithm, computed_hex).
        """
        expected_clean = expected.strip()
        algo = "sha256"
        target_hash = expected_clean

        if ":" in expected_clean:
            parts = expected_clean.split(":", 1)
            algo = parts[0].lower().replace("-", "")
            target_hash = parts[1].strip()
        else:
            lh = len(expected_clean)
            if lh == 32:
                algo = "md5"
            elif lh == 40:
                algo = "sha1"
            elif lh == 64:
                algo = "sha256"
            elif lh == 128:
                algo = "sha512"

        if algo not in SUPPORTED_ALGOS:
            raise ValueError(f"Unsupported checksum algorithm: {algo!r}")

        computed = cls.compute_hashes(file_path, [algo]).get(algo, "")
        is_match = computed.lower() == target_hash.lower()
        return is_match, algo, computed

    @staticmethod
    def parse_checksum_text(text: str, target_filename: str | None = None) -> tuple[str, str] | None:
        """Parse GNU or BSD formatted checksum file content and return (algo, hex_digest)."""
        lines = [ln.strip() for ln in text.splitlines() if ln.strip() and not ln.strip().startswith("#")]
        if not lines:
            return None

        def _algo_from_len(hex_str: str) -> str | None:
            lh = len(hex_str)
            if lh == 32:
                return "md5"
            if lh == 40:
                return "sha1"
            if lh == 64:
                return "sha256"
            if lh == 128:
                return "sha512"
            return None

        # Pass 1: match target_filename if provided
        for line in lines:
            bsd_m = _BSD_CHECKSUM_RE.match(line)
            if bsd_m:
                algo_raw, fname, digest = bsd_m.groups()
                if target_filename is None or Path(fname).name == target_filename:
                    return algo_raw.lower().replace("-", ""), digest.lower()

            parts = line.split()
            if len(parts) >= 2 and re.fullmatch(r"[0-9a-fA-F]{32,128}", parts[0]):
                digest = parts[0].lower()
                fname = parts[1].lstrip("*")
                algo = _algo_from_len(digest)
                if algo and (target_filename is None or Path(fname).name == target_filename):
                    return algo, digest

        # Pass 2: single bare hash in a dedicated <filename>.sha256 file
        if len(lines) == 1:
            parts = lines[0].split()
            if parts and re.fullmatch(r"[0-9a-fA-F]{32,128}", parts[0]):
                if len(parts) == 1 or target_filename is None or Path(parts[1].lstrip("*")).name == target_filename:
                    algo = _algo_from_len(parts[0])
                    if algo:
                        return algo, parts[0].lower()

        return None

    @classmethod
    def find_companion_checksum(cls, file_path: Path) -> tuple[str, str] | None:
        parent = file_path.parent
        name = file_path.name

        direct_candidates = [
            parent / f"{name}.sha256",
            parent / f"{name}.sha256sum",
            parent / f"{name}.sha512",
            parent / f"{name}.md5",
        ]
        for cand in direct_candidates:
            if cand.exists() and cand.is_file():
                try:
                    content = cand.read_text(encoding="utf-8", errors="replace")
                    parsed = cls.parse_checksum_text(content, target_filename=name)
                    if not parsed:
                        parsed = cls.parse_checksum_text(content, target_filename=None)
                    if parsed:
                        return parsed
                except OSError as exc:
                    logger.debug("Error reading companion checksum %s: %s", cand, exc)

        multi_candidates = [
            parent / "sha256sums.txt",
            parent / "SHA256SUMS",
            parent / "checksums.txt",
            parent / "SHASUMS256.txt",
        ]
        for cand in multi_candidates:
            if cand.exists() and cand.is_file():
                try:
                    content = cand.read_text(encoding="utf-8", errors="replace")
                    parsed = cls.parse_checksum_text(content, target_filename=name)
                    if parsed:
                        return parsed
                except OSError as exc:
                    logger.debug("Error reading multi checksum %s: %s", cand, exc)

        return None

    @classmethod
    def display_hash_table(cls, file_path: Path) -> bool:
        p = Path(file_path).expanduser().resolve()
        if not p.exists() or not p.is_file():
            print_plain(f"  [error]File not found: {file_path}[/error]", force=True)
            UI.emit_event("error", message=f"File not found: {file_path}")
            return False

        size = p.stat().st_size
        t0 = time.time()
        hashes = cls.compute_hashes(p, ["md5", "sha1", "sha256", "sha512", "blake2b"])
        dur = round(time.time() - t0, 4)

        if UI.json_mode:
            UI.emit_event(
                "hashes",
                file=str(p),
                filename=p.name,
                size=size,
                duration=dur,
                hashes=hashes,
            )
            return True

        if UI.quiet:
            for algo in SUPPORTED_ALGOS:
                if algo in hashes:
                    sys.stdout.write(f"{algo.upper():<10} {hashes[algo]}\n")
            return True

        # Render horizontal table without importing Rich so `pdl hash` starts in <40ms
        title = f"Cryptographic Hashes: {p.name} ({format_size(size)})"
        rows = [(algo.upper(), hashes[algo]) for algo in SUPPORTED_ALGOS if algo in hashes]
        col1_w = max(len("Algorithm"), *(len(r[0]) for r in rows))
        col2_w = max(len("Hex Digest"), *(len(r[1]) for r in rows))
        width = max(col1_w + col2_w + 4, len(title))
        sep = "─" * width

        lines = [
            "",
            f"  {title}",
            f"  {sep}",
            f"  {'Algorithm':<{col1_w}}    {'Hex Digest':<{col2_w}}",
            f"  {sep}",
        ]
        for k, v in rows:
            lines.append(f"  {k:<{col1_w}}    {v}")
        lines.append(f"  {sep}")
        rate_bps = int(size / max(dur, 0.0001))
        lines.append(f"  Computed in {dur:.3f}s ({format_size(rate_bps)}/s)\n")
        sys.stdout.write("\n".join(lines) + "\n")
        sys.stdout.flush()
        return True
