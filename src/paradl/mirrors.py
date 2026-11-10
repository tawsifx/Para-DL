"""Mirrors configuration ($PARADL_HOME/mirrors.json), pre-flight size validation, failure tracking, and Metalink (.meta4/.metalink) companion support."""

from __future__ import annotations

import fnmatch
import json
import logging
import os
import re
import xml.etree.ElementTree as ET
from dataclasses import dataclass, field
from pathlib import Path
from typing import ClassVar
from urllib.parse import urlparse

from paradl.models import sanitize_filename
from paradl.net import HttpProbeResult, probe_http_url, request_with_retry
from paradl.paths import get_mirrors_file
from paradl.ui import UI, print_plain

logger = logging.getLogger(__name__)

DEFAULT_MAX_MIRROR_FAILURES = 2


@dataclass
class MetalinkCompanion:
    """Represents a discovered .meta4 or .metalink companion document."""

    source: str
    raw_bytes: bytes
    filename: str | None = None
    expected_length: int | None = None
    expected_digest: str | None = None
    urls: list[str] = field(default_factory=list)


@dataclass
class MirrorResolution:
    """Result of resolving and validating primary + mirror URIs before handing to aria2."""

    active_uris: list[str] = field(default_factory=list)
    rejected_size_mirrors: list[str] = field(default_factory=list)
    dropped_failed_mirrors: list[str] = field(default_factory=list)
    expected_length: int | None = None
    expected_digest: str | None = None
    accept_ranges: bool = True
    filename: str | None = None
    metalink: MetalinkCompanion | None = None


def _expand_mirror_template(primary_url: str, pattern: str, mirror_entry: str) -> str:
    """Expand a mirror entry against primary_url and the matched pattern."""
    parsed = urlparse(primary_url)
    url_path = parsed.path or ""
    filename = os.path.basename(url_path.rstrip("/"))
    expanded = mirror_entry.strip()

    if "{filename}" in expanded or "{path}" in expanded or "{url}" in expanded:
        return (
            expanded.replace("{filename}", filename)
            .replace("{path}", url_path.lstrip("/"))
            .replace("{url}", primary_url)
        )

    # Prefix replacement when pattern is a URL prefix ending with '/' and mirror also ends with '/'
    pat_clean = pattern.rstrip("*")
    if pat_clean.endswith("/") and primary_url.startswith(pat_clean) and expanded.endswith("/"):
        suffix = primary_url[len(pat_clean) :]
        return expanded + suffix.lstrip("/")

    # If mirror entry ends with '/' and primary URL has a filename, append filename
    if expanded.endswith("/") and filename:
        return expanded + filename

    return expanded


def _pattern_matches_url(url: str, pattern: str) -> bool:
    """Return True if `pattern` matches `url` via exact, prefix, glob, or regex."""
    u = url.strip()
    p = pattern.strip()
    if not p:
        return False
    if u == p:
        return True
    if fnmatch.fnmatchcase(u, p) or fnmatch.fnmatchcase(u.lower(), p.lower()):
        return True
    if p.endswith("/") and u.startswith(p):
        return True
    if p in u:
        return True
    try:
        return bool(re.search(p, u))
    except re.error:
        return False


class MirrorManager:
    """Manages $PARADL_HOME/mirrors.json lookup, pre-flight size checks, and dead-mirror dropping."""

    _failure_counts: ClassVar[dict[str, int]] = {}

    @classmethod
    def reset_failures(cls) -> None:
        """Clear in-memory mirror failure counters."""
        cls._failure_counts.clear()

    @classmethod
    def get_failure_count(cls, url: str) -> int:
        return cls._failure_counts.get(url.strip(), 0)

    @classmethod
    def record_failure(cls, url: str, *, max_failures: int = DEFAULT_MAX_MIRROR_FAILURES) -> bool:
        """Record a failure for `url`. Returns True if `url` has reached `max_failures` and should be dropped."""
        key = url.strip()
        cls._failure_counts[key] = cls._failure_counts.get(key, 0) + 1
        dropped = cls._failure_counts[key] >= max(1, max_failures)
        if dropped:
            UI.emit_event(
                "mirror_dropped",
                mirror_url=key,
                failures=cls._failure_counts[key],
                reason="repeated_failures",
            )
        return dropped

    @classmethod
    def is_dropped(cls, url: str, *, max_failures: int = DEFAULT_MAX_MIRROR_FAILURES) -> bool:
        return cls.get_failure_count(url) >= max(1, max_failures)

    @classmethod
    def load_mirrors_config(cls, path: Path | None = None) -> dict[str, list[str]]:
        """Load pattern -> [mirror_urls] mapping from $PARADL_HOME/mirrors.json."""
        m_file = path or get_mirrors_file()
        if not m_file.is_file():
            return {}
        try:
            raw = json.loads(m_file.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            logger.warning("Failed to load mirrors.json at %s: %s", m_file, exc)
            return {}

        mapping: dict[str, list[str]] = {}
        if isinstance(raw, dict):
            # Support {"mirrors": {...}} or {"rules": [{"pattern": ..., "mirrors": [...]}]} or flat dict
            if "rules" in raw and isinstance(raw["rules"], list):
                for rule in raw["rules"]:
                    if isinstance(rule, dict) and isinstance(rule.get("pattern"), str):
                        m_list = rule.get("mirrors", [])
                        if isinstance(m_list, str):
                            m_list = [m_list]
                        if isinstance(m_list, list):
                            mapping[rule["pattern"]] = [
                                str(x).strip() for x in m_list if str(x).strip()
                            ]
                return mapping

            source_dict = raw.get("mirrors") if isinstance(raw.get("mirrors"), dict) else raw
            if isinstance(source_dict, dict):
                for pat, val in source_dict.items():
                    if isinstance(val, str) and val.strip():
                        mapping[str(pat)] = [val.strip()]
                    elif isinstance(val, list):
                        mapping[str(pat)] = [str(x).strip() for x in val if str(x).strip()]
        return mapping

    @classmethod
    def match_configured_mirrors(
        cls,
        primary_url: str,
        *,
        mirrors_file: Path | None = None,
    ) -> list[str]:
        """Return expanded alternate URLs from $PARADL_HOME/mirrors.json matching `primary_url`."""
        cfg = cls.load_mirrors_config(path=mirrors_file)
        matched: list[str] = []
        for pattern, mirror_templates in cfg.items():
            if _pattern_matches_url(primary_url, pattern):
                for tmpl in mirror_templates:
                    expanded = _expand_mirror_template(primary_url, pattern, tmpl)
                    if expanded and expanded != primary_url and expanded not in matched:
                        matched.append(expanded)
        return matched

    @classmethod
    def _probe_uri_with_failure_tracking(
        cls,
        url: str,
        *,
        headers: dict[str, str] | None = None,
        max_failures: int = DEFAULT_MAX_MIRROR_FAILURES,
        timeout: float = 3.0,
    ) -> tuple[HttpProbeResult, bool]:
        """Probe a URI up to `max_failures` times; return (probe_result, was_dropped)."""
        if cls.is_dropped(url, max_failures=max_failures):
            return HttpProbeResult(ok=False, final_url=url), True

        attempts = max(1, max_failures)
        last_res = HttpProbeResult(ok=False, final_url=url)
        for _ in range(attempts):
            res = probe_http_url(url, headers=headers, timeout=timeout)
            if res.ok and 200 <= res.status_code < 400:
                return res, False
            last_res = res
            if cls.record_failure(url, max_failures=max_failures):
                return last_res, True
        return last_res, cls.is_dropped(url, max_failures=max_failures)

    @classmethod
    def resolve_and_validate(
        cls,
        primary_url: str,
        *,
        cli_mirrors: list[str] | None = None,
        item_mirrors: list[str] | None = None,
        expected_length: int | None = None,
        expected_digest: str | None = None,
        filename_hint: str | None = None,
        accept_ranges_hint: bool | None = None,
        headers: dict[str, str] | None = None,
        out_dir: Path | None = None,
        explicit_metalink: Path | None = None,
        max_failures: int = DEFAULT_MAX_MIRROR_FAILURES,
        mirrors_file: Path | None = None,
    ) -> MirrorResolution:
        """Resolve configured + CLI mirrors, confirm all mirrors report the same size,
        drop mirrors that fail repeatedly, and discover any .meta4 / .metalink companion.
        """
        cfg_mirrors = cls.match_configured_mirrors(primary_url, mirrors_file=mirrors_file)
        candidate_mirrors: list[str] = []
        for m in [*(item_mirrors or []), *(cli_mirrors or []), *cfg_mirrors]:
            m_clean = (m or "").strip()
            if m_clean and m_clean != primary_url and m_clean not in candidate_mirrors:
                candidate_mirrors.append(m_clean)

        # Check for Metalink (.meta4 / .metalink) companion
        metalink_comp = find_or_fetch_metalink(
            primary_url,
            out_dir=out_dir,
            filename=filename_hint,
            explicit_metalink=explicit_metalink,
            headers=headers,
        )
        if metalink_comp is not None:
            if expected_length is None and metalink_comp.expected_length is not None:
                expected_length = metalink_comp.expected_length
            if expected_digest is None and metalink_comp.expected_digest is not None:
                expected_digest = metalink_comp.expected_digest
            if filename_hint is None and metalink_comp.filename is not None:
                filename_hint = metalink_comp.filename
            for m_url in metalink_comp.urls:
                if m_url != primary_url and m_url not in candidate_mirrors:
                    candidate_mirrors.append(m_url)

        # Fast path when no mirrors exist and caller already probed primary_url
        if not candidate_mirrors and expected_length is not None and accept_ranges_hint is not None:
            return MirrorResolution(
                active_uris=[primary_url],
                expected_length=expected_length,
                expected_digest=expected_digest,
                accept_ranges=accept_ranges_hint,
                filename=filename_hint,
                metalink=metalink_comp,
            )

        auth_length = expected_length
        auth_digest = expected_digest
        auth_filename = filename_hint
        auth_ranges = accept_ranges_hint if accept_ranges_hint is not None else True

        active_uris: list[str] = []
        rejected_size_mirrors: list[str] = []
        dropped_failed_mirrors: list[str] = []

        # 1. Probe primary URL if HTTP(S)
        if primary_url.startswith(("http://", "https://")):
            p_probe, p_dropped = cls._probe_uri_with_failure_tracking(
                primary_url,
                headers=headers,
                max_failures=max_failures,
            )
            if p_probe.ok:
                active_uris.append(primary_url)
                if auth_length is None and p_probe.content_length is not None:
                    auth_length = p_probe.content_length
                if accept_ranges_hint is None:
                    auth_ranges = p_probe.accept_ranges
                if not auth_filename and p_probe.filename:
                    auth_filename = p_probe.filename
                if not auth_digest and p_probe.etag_sha256:
                    auth_digest = f"sha256:{p_probe.etag_sha256}"
            else:
                if p_dropped:
                    dropped_failed_mirrors.append(primary_url)
                    print_plain(
                        f"  [warning]• Primary URL failed repeatedly and was dropped: {primary_url}[/warning]"
                    )
                elif not candidate_mirrors:
                    # Keep primary_url so aria2 reports the canonical error if no mirrors exist
                    active_uris.append(primary_url)
        else:
            active_uris.append(primary_url)

        # 2. Probe and confirm size of each candidate mirror
        for m_url in candidate_mirrors:
            if cls.is_dropped(m_url, max_failures=max_failures):
                dropped_failed_mirrors.append(m_url)
                continue

            m_probe, m_dropped = cls._probe_uri_with_failure_tracking(
                m_url,
                headers=headers,
                max_failures=max_failures,
            )
            if not m_probe.ok:
                if m_dropped and m_url not in dropped_failed_mirrors:
                    dropped_failed_mirrors.append(m_url)
                print_plain(
                    f"  [warning]• Mirror unreachable/failed and dropped: {m_url}[/warning]"
                )
                continue

            m_len = m_probe.content_length
            if auth_length is not None and m_len is not None and m_len != auth_length:
                rejected_size_mirrors.append(m_url)
                msg = (
                    f"Mirror rejected due to size mismatch ({m_url}): "
                    f"expected {auth_length} bytes, mirror reported {m_len} bytes"
                )
                print_plain(f"  [warning]⚠ {msg}[/warning]", force=True)
                UI.emit_event(
                    "mirror_rejected",
                    mirror_url=m_url,
                    reason="size_mismatch",
                    expected_length=auth_length,
                    mirror_length=m_len,
                )
                continue

            if auth_length is None and m_len is not None:
                auth_length = m_len
            if not auth_digest and m_probe.etag_sha256:
                auth_digest = f"sha256:{m_probe.etag_sha256}"
            if not auth_filename and m_probe.filename:
                auth_filename = m_probe.filename
            if m_probe.accept_ranges:
                auth_ranges = True

            active_uris.append(m_url)

        return MirrorResolution(
            active_uris=active_uris,
            rejected_size_mirrors=rejected_size_mirrors,
            dropped_failed_mirrors=dropped_failed_mirrors,
            expected_length=auth_length,
            expected_digest=auth_digest,
            accept_ranges=auth_ranges,
            filename=auth_filename,
            metalink=metalink_comp,
        )


# ---------------------------------------------------------------------------
# Metalink (.meta4 / .metalink) Companion Discovery & Metadata Extraction
# ---------------------------------------------------------------------------

_HASH_ALGO_MAP: dict[str, str] = {
    "sha-256": "sha256",
    "sha256": "sha256",
    "sha-512": "sha512",
    "sha512": "sha512",
    "sha-1": "sha1",
    "sha1": "sha1",
    "md5": "md5",
}


def parse_metalink_bytes(raw_bytes: bytes, *, source: str = "metalink") -> MetalinkCompanion | None:
    """Parse RFC 5854 (.meta4) or Metalink v3 (.metalink) XML bytes to extract file metadata.

    Note: Chunk repair itself is delegated natively to `aria2c --check-integrity=true` via `aria2.addMetalink`.
    """
    decoded = raw_bytes.decode("utf-8", errors="replace")
    # Defend against XXE and entity expansion attacks in untrusted mirror responses
    decoded_lower = decoded.lower()
    if "<!entity" in decoded_lower:
        logger.warning("Rejected Metalink XML containing ENTITY declaration from %s", source)
        return None

    try:
        root = ET.fromstring(decoded)  # nosec B314
    except ET.ParseError as exc:
        logger.debug("Invalid Metalink XML from %s: %s", source, exc)
        return None

    tag_low = root.tag.lower()
    if "metalink" not in tag_low:
        return None

    filename: str | None = None
    expected_len: int | None = None
    expected_digest: str | None = None
    urls: list[str] = []

    for elem in root.iter():
        el_tag = elem.tag.split("}", 1)[-1].lower()
        if el_tag == "file" and filename is None:
            fname_attr = elem.attrib.get("name")
            if fname_attr:
                filename = os.path.basename(fname_attr.strip())
        elif el_tag == "size" and expected_len is None and elem.text:
            txt = elem.text.strip()
            if txt.isdigit():
                expected_len = int(txt)
        elif el_tag == "hash" and elem.text:
            # Whole-file <hash type="sha-256"> (skip piece-level <hash> inside <pieces>)
            htype = (elem.attrib.get("type") or "").strip().lower()
            algo = _HASH_ALGO_MAP.get(htype)
            if algo and expected_digest is None:
                expected_digest = f"{algo}:{elem.text.strip().lower()}"
        elif el_tag == "url" and elem.text:
            u = elem.text.strip()
            if u.startswith(("http://", "https://", "ftp://")) and u not in urls:
                urls.append(u)

    return MetalinkCompanion(
        source=source,
        raw_bytes=raw_bytes,
        filename=filename,
        expected_length=expected_len,
        expected_digest=expected_digest,
        urls=urls,
    )


def find_or_fetch_metalink(
    url: str,
    *,
    out_dir: Path | None = None,
    filename: str | None = None,
    explicit_metalink: Path | None = None,
    headers: dict[str, str] | None = None,
) -> MetalinkCompanion | None:
    """Discover a local or remote .meta4 or .metalink companion for `url`."""
    # 1. Explicit metalink path
    if explicit_metalink and explicit_metalink.is_file():
        try:
            return parse_metalink_bytes(
                explicit_metalink.read_bytes(), source=str(explicit_metalink)
            )
        except OSError:
            pass

    # 2. Target itself is a local .meta4 / .metalink file
    if url.lower().endswith((".meta4", ".metalink")) and not url.startswith(
        ("http://", "https://")
    ):
        local_p = Path(url).expanduser()
        if local_p.is_file():
            try:
                return parse_metalink_bytes(local_p.read_bytes(), source=str(local_p))
            except OSError:
                pass

    # 3. Target is a direct HTTP(S) .meta4 / .metalink URL
    url_clean = url.split("?", 1)[0]
    if url_clean.lower().endswith((".meta4", ".metalink")) and url.startswith(
        ("http://", "https://")
    ):
        try:
            resp = request_with_retry("GET", url, headers=headers, retries=2, timeout=4.0)
            if resp.status_code == 200 and resp.content:
                parsed = parse_metalink_bytes(bytes(resp.content), source=url)
                if parsed is not None:
                    return parsed
        except Exception as exc:
            logger.debug("Failed to fetch remote metalink %s: %s", url, exc)

    # 4. Local companion file in out_dir (<filename>.meta4 or <filename>.metalink)
    if out_dir is not None:
        base_names: list[str] = []
        if filename:
            base_names.append(filename)
        url_base = os.path.basename(url_clean.rstrip("/"))
        if url_base and url_base not in base_names:
            base_names.append(url_base)

        for bname in base_names:
            safe_bname = sanitize_filename(bname)[:180]
            for ext in (".meta4", ".metalink"):
                try:
                    cand = out_dir / f"{safe_bname}{ext}"
                    if cand.is_file():
                        parsed = parse_metalink_bytes(cand.read_bytes(), source=str(cand))
                        if parsed is not None:
                            return parsed
                except OSError:
                    pass

    return None
