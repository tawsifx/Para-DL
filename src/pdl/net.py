"""Pooled httpx network layer, RFC 6266 parser, Range/HEAD prober, and target classifier."""

from __future__ import annotations

import importlib.util
import logging
import os
import random
import re
import socket
import time
import urllib.parse
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from typing import Any

from pdl import VERSION
from pdl.history import HistoryManager
from pdl.models import (
    DIRECT_EXTENSIONS,
    TargetInfo,
    TargetType,
    is_media_domain,
    sanitize_filename,
)

logger = logging.getLogger(__name__)

_SHARED_CLIENT: Any = None

_SHA256_HEX_RE = re.compile(r"\b([0-9a-fA-F]{64})\b")
_FILENAME_STAR_RE = re.compile(
    r"filename\*\s*=\s*([A-Za-z0-9_\-]+)?'[^']*'(?:\"([^\"]+)\"|([^;\s]+))",
    re.IGNORECASE,
)
_FILENAME_PLAIN_RE = re.compile(
    r'filename\s*=\s*(?:"([^"]+)"|\'([^\']+)\'|([^;\s]+))',
    re.IGNORECASE,
)
_CONTENT_RANGE_TOTAL_RE = re.compile(r"bytes\s+(?:[0-9]+-[0-9]+|\*)/([0-9]+)", re.IGNORECASE)


def get_http_client() -> Any:
    """Return a lazily initialized pooled httpx.Client (with HTTP/2 if h2 is installed)."""
    global _SHARED_CLIENT
    if _SHARED_CLIENT is None:
        import httpx

        has_h2 = importlib.util.find_spec("h2") is not None
        _SHARED_CLIENT = httpx.Client(
            http2=has_h2,
            follow_redirects=True,
            timeout=httpx.Timeout(10.0, connect=4.0),
            limits=httpx.Limits(max_keepalive_connections=20, max_connections=40),
            headers={"User-Agent": f"pdl-accelerator/{VERSION}"},
        )
    return _SHARED_CLIENT


def close_http_client() -> None:
    """Close shared httpx client if open."""
    global _SHARED_CLIENT
    if _SHARED_CLIENT is not None:
        try:
            _SHARED_CLIENT.close()
        except Exception as exc:
            logger.debug("Error closing httpx client: %s", exc)
        _SHARED_CLIENT = None


def re_resolve_dns(host: str) -> list[str]:
    """Reset pooled HTTP connections and re-resolve DNS A/AAAA addresses for `host`."""
    close_http_client()
    if not host:
        return []
    resolved: list[str] = []
    try:
        infos = socket.getaddrinfo(host, None, socket.AF_UNSPEC, socket.SOCK_STREAM)
        for info in infos:
            addr = str(info[4][0])
            if addr and addr not in resolved:
                resolved.append(addr)
    except OSError as exc:
        logger.debug("DNS re-resolution for %s failed: %s", host, exc)
    return resolved


def request_with_retry(
    method: str,
    url: str,
    *,
    headers: dict[str, str] | None = None,
    retries: int = 3,
    timeout: float = 4.5,
    follow_redirects: bool = True,
) -> Any:
    """Perform an HTTP request with jittered exponential backoff on transient failures."""
    import httpx

    client = get_http_client()
    last_exc: Exception | None = None
    for attempt in range(max(retries, 1)):
        try:
            resp = client.request(
                method,
                url,
                headers=headers,
                timeout=timeout,
                follow_redirects=follow_redirects,
            )
            if resp.status_code in (502, 503, 504) and attempt < retries - 1:
                delay = (0.15 * (2**attempt)) + random.uniform(0.0, 0.15)
                time.sleep(delay)
                continue
            return resp
        except httpx.HTTPError as exc:
            last_exc = exc
            if attempt < retries - 1:
                delay = (0.15 * (2**attempt)) + random.uniform(0.0, 0.15)
                time.sleep(delay)
    if last_exc is not None:
        raise last_exc
    raise RuntimeError(f"Request failed for {url}")


def parse_content_disposition_filename(header_val: str | None) -> str | None:
    """Extract filename from Content-Disposition header with RFC 6266 filename* support."""
    if not header_val:
        return None

    # RFC 6266: filename* takes priority over filename when both are present
    m_star = _FILENAME_STAR_RE.search(header_val)
    if m_star:
        charset = (m_star.group(1) or "utf-8").strip()
        raw_encoded = (m_star.group(2) or m_star.group(3) or "").strip()
        if raw_encoded:
            try:
                decoded = urllib.parse.unquote(raw_encoded, encoding=charset, errors="replace")
            except LookupError:
                decoded = urllib.parse.unquote(raw_encoded, encoding="utf-8", errors="replace")
            base = os.path.basename(decoded.strip())
            if base:
                return sanitize_filename(base)

    m_plain = _FILENAME_PLAIN_RE.search(header_val)
    if m_plain:
        raw_name = (m_plain.group(1) or m_plain.group(2) or m_plain.group(3) or "").strip()
        if raw_name:
            unquoted = urllib.parse.unquote(raw_name)
            base = os.path.basename(unquoted.strip())
            if base:
                return sanitize_filename(base)

    return None


def extract_etag_sha256(headers: Any) -> str | None:
    """Extract a 64-hex SHA-256 digest from Hugging Face X-Linked-ETag or ETag headers."""
    if not headers:
        return None
    for h_name in ("x-linked-etag", "etag", "x-checksum-sha256"):
        val = headers.get(h_name)
        if val:
            m = _SHA256_HEX_RE.search(str(val))
            if m:
                return m.group(1).lower()
    return None


@dataclass
class HttpProbeResult:
    ok: bool = False
    final_url: str = ""
    status_code: int = 0
    content_type: str = ""
    content_length: int | None = None
    filename: str | None = None
    accept_ranges: bool = False
    etag_sha256: str | None = None
    headers: dict[str, str] = field(default_factory=dict)


def probe_range_support(
    url: str,
    headers: dict[str, str] | None = None,
    timeout: float = 4.0,
) -> HttpProbeResult:
    """Send a streaming GET with Range: bytes=0-0 to verify 206 Partial Content support
    without downloading the full response body on 200-only servers.
    """
    client = get_http_client()
    req_headers = {
        "User-Agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
        "Range": "bytes=0-0",
    }
    if headers:
        req_headers.update(headers)

    try:
        with client.stream("GET", url, headers=req_headers, timeout=timeout, follow_redirects=True) as resp:
            status = resp.status_code
            ctype = resp.headers.get("content-type", "").lower()
            cdisp = resp.headers.get("content-disposition", "")
            crange = resp.headers.get("content-range", "")
            clen_hdr = resp.headers.get("content-length")

            total_len: int | None = None
            m_range = _CONTENT_RANGE_TOTAL_RE.search(crange)
            if m_range:
                total_len = int(m_range.group(1))
            elif status == 200 and clen_hdr and clen_hdr.isdigit():
                total_len = int(clen_hdr)

            supports_206 = status == 206
            fname = parse_content_disposition_filename(cdisp)
            etag_hash = extract_etag_sha256(resp.headers)

            return HttpProbeResult(
                ok=status in (200, 206),
                final_url=str(resp.url),
                status_code=status,
                content_type=ctype,
                content_length=total_len,
                filename=fname,
                accept_ranges=supports_206,
                etag_sha256=etag_hash,
                headers={k.lower(): v for k, v in resp.headers.items()},
            )
    except Exception as exc:
        logger.debug("Range probe failed for %s: %s", url, exc)
        return HttpProbeResult(ok=False, final_url=url)


def probe_http_url(
    url: str,
    headers: dict[str, str] | None = None,
    timeout: float = 3.5,
) -> HttpProbeResult:
    """Probe a URL via HEAD, falling back to Range: bytes=0-0 GET on 403/405 or missing metadata."""
    req_headers = {
        "User-Agent": "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
    }
    if headers:
        req_headers.update(headers)

    head_res: HttpProbeResult | None = None
    try:
        resp = request_with_retry("HEAD", url, headers=req_headers, retries=2, timeout=timeout)
        if resp.status_code not in (400, 403, 405, 501):
            ctype = resp.headers.get("content-type", "").lower()
            cdisp = resp.headers.get("content-disposition", "")
            clen = resp.headers.get("content-length")
            c_len_val = int(clen) if (clen and clen.isdigit()) else None
            acc_ranges = resp.headers.get("accept-ranges", "").lower() == "bytes"
            fname = parse_content_disposition_filename(cdisp)
            etag_hash = extract_etag_sha256(resp.headers)
            head_res = HttpProbeResult(
                ok=200 <= resp.status_code < 400,
                final_url=str(resp.url),
                status_code=resp.status_code,
                content_type=ctype,
                content_length=c_len_val,
                filename=fname,
                accept_ranges=acc_ranges,
                etag_sha256=etag_hash,
                headers={k.lower(): v for k, v in resp.headers.items()},
            )
    except Exception as exc:
        logger.debug("HEAD probe failed for %s: %s", url, exc)

    # Fall back to Range: bytes=0-0 GET if HEAD failed, returned 403/405, or didn't confirm range support
    if head_res is None or not head_res.ok or not head_res.accept_ranges:
        range_res = probe_range_support(url, headers=headers, timeout=timeout)
        if range_res.ok:
            if head_res and head_res.ok:
                if not range_res.filename and head_res.filename:
                    range_res.filename = head_res.filename
                if range_res.content_length is None and head_res.content_length is not None:
                    range_res.content_length = head_res.content_length
                if not range_res.etag_sha256 and head_res.etag_sha256:
                    range_res.etag_sha256 = head_res.etag_sha256
            return range_res

    return head_res if head_res is not None else HttpProbeResult(ok=False, final_url=url)


def ytdlp_extractor_claims_url(url: str) -> bool:
    """Return True if a non-generic yt-dlp extractor explicitly matches the URL."""
    if is_media_domain(url):
        return True
    try:
        from yt_dlp.extractor import gen_extractor_classes

        for ie_cls in gen_extractor_classes():
            ie_name = getattr(ie_cls, "IE_NAME", "")
            if ie_name == "generic":
                continue
            if ie_cls.suitable(url):
                return True
    except Exception as exc:
        logger.debug("yt-dlp extractor check skipped: %s", exc)
    return False


def fetch_json_with_etag(
    url: str,
    headers: dict[str, str] | None = None,
    timeout: float = 6.0,
) -> tuple[Any | None, dict[str, str], int]:
    """Fetch JSON from url with SQLite ETag/If-None-Match caching.

    Returns (parsed_json_or_None, response_headers_lower, status_code).
    """
    req_headers = dict(headers or {})
    cached = HistoryManager.get_http_cache(url)
    if cached is not None:
        cached_etag, _cached_body = cached
        if cached_etag:
            req_headers["If-None-Match"] = cached_etag

    try:
        resp = request_with_retry("GET", url, headers=req_headers, retries=2, timeout=timeout)
        hdrs = {k.lower(): v for k, v in resp.headers.items()}
        if resp.status_code == 304 and cached is not None:
            return cached[1], hdrs, 200
        if resp.status_code == 200:
            data = resp.json()
            etag = hdrs.get("etag")
            if etag:
                HistoryManager.set_http_cache(url, etag, data)
            return data, hdrs, 200
        return None, hdrs, resp.status_code
    except Exception as exc:
        logger.debug("fetch_json_with_etag failed for %s: %s", url, exc)
        return None, {}, 0


def probe_target(target: str, *, allow_network_probe: bool = True) -> TargetInfo:
    """Classify any raw target string into a TargetInfo with identical order to v3.1.0-pro."""
    from pdl.engines.brew import BrewEngine
    from pdl.engines.cloud import CloudResolver
    from pdl.engines.github import GitHubEngine
    from pdl.engines.hf import HuggingFaceEngine
    from pdl.engines.ipsw import IpswEngine
    from pdl.engines.pypi import PyPIEngine

    target = target.strip()

    # 1. Apple IPSW Firmware URLs & Shortcuts
    if IpswEngine.is_ipsw(target):
        ipsw_data = IpswEngine.resolve(target, allow_network=allow_network_probe)
        if ipsw_data and ipsw_data.get("url"):
            sha = ipsw_data.get("sha256")
            return TargetInfo(
                raw=target,
                target_type=TargetType.IPSW,
                clean_url=ipsw_data["url"],
                filename_hint=ipsw_data.get("filename"),
                content_length=ipsw_data.get("filesize"),
                expected_digest=f"sha256:{sha}" if sha else None,
                ipsw_meta=ipsw_data,
            )

    # 2. Magnet / Torrent / Local Metalink (.meta4 / .metalink)
    if target.startswith("magnet:?") or target.lower().endswith(".torrent"):
        return TargetInfo(raw=target, target_type=TargetType.TORRENT, clean_url=target)

    # 3. Python / PyPI packages (pip:<pkg>, pypi:<pkg>, pypi.org/project/...)
    if PyPIEngine.is_pypi(target):
        pypi_data = PyPIEngine.resolve(target, allow_network=allow_network_probe)
        if pypi_data and pypi_data.get("url"):
            sha = pypi_data.get("sha256")
            return TargetInfo(
                raw=target,
                target_type=TargetType.PYPI,
                clean_url=pypi_data["url"],
                filename_hint=pypi_data.get("filename"),
                content_length=pypi_data.get("filesize"),
                expected_digest=f"sha256:{sha}" if sha else None,
                pypi_meta=pypi_data,
            )
        return TargetInfo(
            raw=target,
            target_type=TargetType.PYPI,
            clean_url=target,
            pypi_meta=pypi_data or {},
        )

    # 4. Homebrew bottles (brew:<formula>, homebrew:<formula>, formulae.brew.sh/formula/...)
    if BrewEngine.is_brew(target):
        brew_data = BrewEngine.resolve(target, allow_network=allow_network_probe)
        if brew_data and brew_data.get("url"):
            sha = brew_data.get("sha256")
            return TargetInfo(
                raw=target,
                target_type=TargetType.BREW,
                clean_url=brew_data["url"],
                filename_hint=brew_data.get("filename"),
                content_length=brew_data.get("filesize"),
                headers=brew_data.get("headers", {}),
                expected_digest=f"sha256:{sha}" if sha else None,
                brew_meta=brew_data,
            )
        return TargetInfo(
            raw=target,
            target_type=TargetType.BREW,
            clean_url=target,
            brew_meta=brew_data or {},
        )

    if target.lower().endswith((".meta4", ".metalink")) and not target.startswith(("http://", "https://")):
        local_meta = os.path.abspath(os.path.expanduser(target))
        if os.path.isfile(local_meta):
            from pathlib import Path as _Path

            return TargetInfo(
                raw=target,
                target_type=TargetType.DIRECT,
                clean_url=local_meta,
                metalink_path=_Path(local_meta),
            )

    # 3. Cloud sharing URLs (Google Drive, Dropbox, Pastebin)
    resolved_url, hint = CloudResolver.resolve_url(target)
    if resolved_url != target:
        return TargetInfo(
            raw=target,
            target_type=TargetType.DIRECT,
            clean_url=resolved_url,
            filename_hint=hint,
        )

    # 4. Hugging Face Hub
    if HuggingFaceEngine.is_hf(target):
        hf_meta = HuggingFaceEngine.parse_hf_target(target)
        if hf_meta.get("is_direct_file"):
            prefix = "datasets/" if hf_meta.get("repo_type") == "dataset" else ""
            direct_url = (
                f"https://huggingface.co/{prefix}{hf_meta['repo_id']}/resolve/"
                f"{hf_meta['revision']}/{hf_meta['subpath']}"
            )
            hf_headers = HuggingFaceEngine.get_headers()
            etag_sha: str | None = None
            content_len: int | None = None
            acc_ranges: bool | None = None
            if allow_network_probe:
                p_res = probe_http_url(direct_url, headers=hf_headers)
                if p_res.ok:
                    etag_sha = p_res.etag_sha256
                    content_len = p_res.content_length
                    acc_ranges = p_res.accept_ranges
            return TargetInfo(
                raw=target,
                target_type=TargetType.DIRECT,
                clean_url=direct_url,
                filename_hint=os.path.basename(hf_meta["subpath"]),
                content_length=content_len,
                accept_ranges=acc_ranges,
                etag_sha256=etag_sha,
                expected_digest=f"sha256:{etag_sha}" if etag_sha else None,
                headers=hf_headers,
                hf_meta=hf_meta,
            )
        return TargetInfo(raw=target, target_type=TargetType.HF, clean_url=target, hf_meta=hf_meta)

    # 5. GitHub URLs & Shortcuts
    if GitHubEngine.is_github(target):
        gh_meta = GitHubEngine.parse_github_target(target)
        if gh_meta["type"] in ("blob", "asset"):
            return TargetInfo(
                raw=target,
                target_type=TargetType.DIRECT,
                clean_url=gh_meta["url"],
                filename_hint=gh_meta["filename"],
                headers=GitHubEngine.get_download_headers(),
                github_meta=gh_meta,
            )
        return TargetInfo(
            raw=target,
            target_type=TargetType.GITHUB,
            clean_url=gh_meta.get("url", target),
            github_meta=gh_meta,
        )

    # 6. Generic URLs (http, https, ftp)
    if target.startswith(("http://", "https://", "ftp://")):
        url_without_params = target.split("?", 1)[0].split("#", 1)[0]
        ext = os.path.splitext(url_without_params)[1].lower()

        # 6a. Hostname suffix media domain match (Phase 0 #5)
        if is_media_domain(target):
            return TargetInfo(raw=target, target_type=TargetType.STREAM, clean_url=target)

        # 6b. Known direct file extensions
        if ext in DIRECT_EXTENSIONS:
            filename = os.path.basename(url_without_params)
            return TargetInfo(
                raw=target,
                target_type=TargetType.DIRECT,
                clean_url=target,
                filename_hint=filename,
            )

        # 6c. HTTP HEAD / Range Probe for unknown generic URLs
        if allow_network_probe and target.startswith(("http://", "https://")):
            p_res = probe_http_url(target)
            if p_res.ok:
                content_type = p_res.content_type
                content_len = p_res.content_length
                is_direct = False
                if any(x in content_type for x in ("application/", "video/", "audio/", "image/", "octet-stream")):
                    is_direct = True
                elif "text/html" not in content_type and content_len and content_len > 1024:
                    is_direct = True
                elif p_res.accept_ranges and "text/html" not in content_type:
                    is_direct = True

                if is_direct:
                    fname = p_res.filename or os.path.basename(p_res.final_url.split("?", 1)[0]) or "downloaded_file"
                    return TargetInfo(
                        raw=target,
                        target_type=TargetType.DIRECT,
                        clean_url=p_res.final_url,
                        filename_hint=fname,
                        content_length=content_len,
                        mime_type=content_type,
                        accept_ranges=p_res.accept_ranges,
                        etag_sha256=p_res.etag_sha256,
                        expected_digest=f"sha256:{p_res.etag_sha256}" if p_res.etag_sha256 else None,
                    )

                if "text/html" in content_type or ytdlp_extractor_claims_url(target):
                    return TargetInfo(raw=target, target_type=TargetType.STREAM, clean_url=target)

        return TargetInfo(raw=target, target_type=TargetType.STREAM, clean_url=target)

    # 7. Free-text Search Query
    return TargetInfo(raw=target, target_type=TargetType.SEARCH, clean_url=f"ytsearch1:{target}")


def probe_targets_parallel(targets: list[str], *, allow_network_probe: bool = True) -> list[TargetInfo]:
    """Probe multiple targets concurrently while preserving input order."""
    if not targets:
        return []
    if len(targets) == 1:
        return [probe_target(targets[0], allow_network_probe=allow_network_probe)]
    with ThreadPoolExecutor(max_workers=min(len(targets), 8)) as pool:
        futures = [pool.submit(probe_target, t, allow_network_probe=allow_network_probe) for t in targets]
        return [f.result() for f in futures]
