"""Safe cURL command parser and Netscape cookie jar generator for pdl (Phase E).

Parses 'Copy as cURL' commands from Chrome, Firefox, Safari, and Edge.
Never uses shell=True and never uses eval.
Strictly rejects POST/form data and non-GET requests.
Generates isolated, domain-scoped Netscape cookie jars with 0600 permissions.
"""

from __future__ import annotations

import os
import re
import shlex
import shutil
import tempfile
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from urllib.parse import urlparse

_DATA_FLAGS = {
    "-d",
    "--data",
    "--data-raw",
    "--data-binary",
    "--data-urlencode",
    "--data-ascii",
    "-F",
    "--form",
    "--form-string",
}

_UNSAFE_FILENAME_CHARS = re.compile(r'[/\\:\x00-\x1f\x7f*?"<>|]')


@dataclass
class ParsedCurl:
    """Structured representation of a parsed cURL command."""

    url: str
    headers: dict[str, str] = field(default_factory=dict)
    cookies: dict[str, str] = field(default_factory=dict)
    user_agent: str | None = None
    referer: str | None = None
    compressed: bool = False
    output_filename: str | None = None


def sanitize_filename(name: str) -> str:
    """Strip path traversal, directory separators, and unsafe control characters."""
    base = Path(name).name
    cleaned = _UNSAFE_FILENAME_CHARS.sub("_", base).strip(" .")
    return cleaned or "download.bin"


def parse_curl_command(cmd_string: str) -> ParsedCurl:
    """Parse a 'Copy as cURL' command string using safe lexical analysis (shlex).

    Raises ValueError for unsafe, non-GET, or malformed commands.
    """
    raw_str = cmd_string.strip()
    if not raw_str:
        raise ValueError("Empty cURL command provided.")

    try:
        tokens = shlex.split(raw_str)
    except Exception as exc:
        raise ValueError(f"Failed to parse cURL command syntax: {exc}") from exc

    if not tokens:
        raise ValueError("Empty cURL command after tokenization.")

    # Remove leading 'curl' if present
    if tokens[0].lower() == "curl":
        tokens = tokens[1:]

    url: str | None = None
    headers: dict[str, str] = {}
    cookies: dict[str, str] = {}
    user_agent: str | None = None
    referer: str | None = None
    compressed = False
    output_filename: str | None = None

    idx = 0
    num_tokens = len(tokens)
    while idx < num_tokens:
        tok = tokens[idx]

        # Check for rejected POST/form data flags
        if tok in _DATA_FLAGS or any(tok.startswith(f"{df}=") for df in _DATA_FLAGS):
            raise ValueError(f"POST/payload flag '{tok}' is rejected. pdl only downloads via HTTP GET.")

        # Check method
        if tok in ("-X", "--request"):
            idx += 1
            if idx >= num_tokens:
                raise ValueError(f"Flag '{tok}' missing HTTP method argument.")
            method = tokens[idx].upper()
            if method != "GET":
                raise ValueError(f"HTTP method '{method}' is not supported. pdl only performs GET downloads.")
            idx += 1
            continue

        # Headers
        if tok in ("-H", "--header"):
            idx += 1
            if idx >= num_tokens:
                raise ValueError(f"Flag '{tok}' missing header argument.")
            hdr_val = tokens[idx]
            if ":" in hdr_val:
                h_name, h_val = hdr_val.split(":", 1)
                h_name_clean = h_name.strip()
                h_val_clean = h_val.strip()
                h_lower = h_name_clean.lower()
                if h_lower == "cookie":
                    # Parse semicolon-separated cookies
                    for part in h_val_clean.split(";"):
                        if "=" in part:
                            k, v = part.split("=", 1)
                            if k.strip():
                                cookies[k.strip()] = v.strip()
                elif h_lower == "user-agent":
                    user_agent = h_val_clean
                elif h_lower == "referer":
                    referer = h_val_clean
                else:
                    headers[h_name_clean] = h_val_clean
            idx += 1
            continue

        # Cookies via -b / --cookie
        if tok in ("-b", "--cookie"):
            idx += 1
            if idx >= num_tokens:
                raise ValueError(f"Flag '{tok}' missing cookie argument.")
            c_val = tokens[idx]
            if "=" in c_val:
                for part in c_val.split(";"):
                    if "=" in part:
                        k, v = part.split("=", 1)
                        if k.strip():
                            cookies[k.strip()] = v.strip()
            elif os.path.exists(c_val):
                # Read from cookie file
                try:
                    with open(c_val, encoding="utf-8", errors="ignore") as cf:
                        for line in cf:
                            line_s = line.strip()
                            if line_s and not line_s.startswith("#"):
                                fields = line_s.split("\t")
                                if len(fields) >= 7:
                                    cookies[fields[5]] = fields[6]
                except OSError:
                    pass
            idx += 1
            continue

        # User-Agent
        if tok in ("-A", "--user-agent"):
            idx += 1
            if idx >= num_tokens:
                raise ValueError(f"Flag '{tok}' missing user-agent argument.")
            user_agent = tokens[idx].strip()
            idx += 1
            continue

        # Referer
        if tok in ("-e", "--referer"):
            idx += 1
            if idx >= num_tokens:
                raise ValueError(f"Flag '{tok}' missing referer argument.")
            referer = tokens[idx].strip()
            idx += 1
            continue

        # Compressed
        if tok == "--compressed":
            compressed = True
            idx += 1
            continue

        # Output filename
        if tok in ("-o", "--output"):
            idx += 1
            if idx >= num_tokens:
                raise ValueError(f"Flag '{tok}' missing output argument.")
            output_filename = sanitize_filename(tokens[idx])
            idx += 1
            continue

        # Target URL
        if not tok.startswith("-"):
            if url is None:
                url = tok
            idx += 1
            continue

        # Unrecognized flags without parameters (e.g. -s, -S, -k, -L)
        idx += 1

    if not url:
        raise ValueError("No URL found in cURL command.")

    # Validate URL scheme
    parsed = urlparse(url)
    if parsed.scheme.lower() not in ("http", "https"):
        raise ValueError(f"Unsupported URL scheme '{parsed.scheme}'. Only http and https URLs are allowed.")

    if not parsed.netloc:
        raise ValueError(f"Invalid URL '{url}': missing hostname.")

    return ParsedCurl(
        url=url,
        headers=headers,
        cookies=cookies,
        user_agent=user_agent,
        referer=referer,
        compressed=compressed,
        output_filename=output_filename,
    )


def create_netscape_cookie_jar(
    cookies: dict[str, str],
    host: str,
    is_secure: bool = True,
) -> tuple[Path, Callable[[], None]]:
    """Generate a temporary Netscape-format cookie file with 0600 permissions.

    Strictly scoped to `host` so cookies cannot be sent to other domains on redirect.
    Returns (jar_path, cleanup_function).
    """
    temp_dir = Path(tempfile.mkdtemp(prefix="pdl_curl_cookies_"))
    jar_path = temp_dir / "cookies.txt"

    # Open with explicit 0600 permissions (read/write only by current user)
    fd = os.open(jar_path, os.O_CREAT | os.O_WRONLY | os.O_TRUNC, 0o600)
    try:
        with open(fd, "w", encoding="utf-8") as f:
            f.write("# Netscape HTTP Cookie File\n")
            f.write("# Generated securely by pdl for scoped transfer\n\n")

            domain = host.split(":")[0]  # strip port if present
            # Netscape format: domain, flag, path, secure, expiration, name, value
            flag = "FALSE"
            path = "/"
            secure_str = "TRUE" if is_secure else "FALSE"
            expiration = "2147483647"  # Year 2038 POSIX max

            for name, val in cookies.items():
                if name:
                    f.write(f"{domain}\t{flag}\t{path}\t{secure_str}\t{expiration}\t{name}\t{val}\n")
    except Exception:
        shutil.rmtree(temp_dir, ignore_errors=True)
        raise

    def cleanup() -> None:
        try:
            if jar_path.exists():
                jar_path.unlink(missing_ok=True)
            if temp_dir.exists():
                shutil.rmtree(temp_dir, ignore_errors=True)
        except OSError:
            pass

    return jar_path, cleanup
