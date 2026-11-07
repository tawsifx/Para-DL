"""Target loading, candidate runtime validation, and RDAP / Team Cymru DNS ASN & country resolution."""

from __future__ import annotations

import logging
import socket
import struct
import tomllib
from collections.abc import Callable
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

import httpx

from pdl.paths import get_targets_file
from pdl.throttle.models import TargetCandidate, ValidatedTarget
from pdl.throttle.store import ThrottleStore

logger = logging.getLogger(__name__)

DEFAULT_TARGETS_PATH = Path(__file__).resolve().parent / "default_targets.toml"

# Type alias for injectable ASN lookup: ip -> (asn, country, org)
ASNResolverFn = Callable[[str], tuple[str, str, str]]
_CUSTOM_ASN_RESOLVER: ASNResolverFn | None = None


def set_asn_resolver(resolver: ASNResolverFn | None) -> None:
    """Inject a custom ASN/country resolver (used by unit/integration tests)."""
    global _CUSTOM_ASN_RESOLVER
    _CUSTOM_ASN_RESOLVER = resolver


def reset_asn_resolver() -> None:
    """Restore the default RDAP + Team Cymru DNS resolver."""
    global _CUSTOM_ASN_RESOLVER
    _CUSTOM_ASN_RESOLVER = None


def _build_dns_txt_query(qname: str) -> bytes:
    """Construct a minimal RFC 1035 DNS TXT query packet (QTYPE=16, QCLASS=1)."""
    header = struct.pack("!HHHHHH", 0x4242, 0x0100, 1, 0, 0, 0)
    labels = (
        b"".join(bytes([len(part)]) + part.encode("ascii") for part in qname.strip(".").split(".") if part) + b"\x00"
    )
    question = labels + struct.pack("!HH", 16, 1)
    return header + question


def _parse_dns_txt_response(packet: bytes) -> str:
    """Extract the first TXT string from a raw DNS UDP response packet."""
    if len(packet) < 12:
        return ""
    ancount = struct.unpack("!H", packet[6:8])[0]
    if ancount < 1:
        return ""
    # Skip header (12B) and question section
    idx = 12
    while idx < len(packet) and packet[idx] != 0:
        if (packet[idx] & 0xC0) == 0xC0:
            idx += 2
            break
        idx += 1 + packet[idx]
    else:
        idx += 1
    idx += 4  # QTYPE + QCLASS

    for _ in range(ancount):
        if idx >= len(packet):
            break
        if (packet[idx] & 0xC0) == 0xC0:
            idx += 2
        else:
            while idx < len(packet) and packet[idx] != 0:
                idx += 1 + packet[idx]
            idx += 1
        if idx + 10 > len(packet):
            break
        rtype, _rclass, _ttl, rdlen = struct.unpack("!HHIH", packet[idx : idx + 10])
        idx += 10
        rdata = packet[idx : idx + rdlen]
        idx += rdlen
        if rtype == 16 and len(rdata) > 1:
            txt_len = rdata[0]
            return rdata[1 : 1 + txt_len].decode("utf-8", errors="ignore")
    return ""


def query_team_cymru_dns(ip: str, timeout: float = 1.5) -> tuple[str, str, str] | None:
    """Look up ASN and country via Team Cymru IP-to-ASN DNS TXT service (`origin.asn.cymru.com`)."""
    parts = ip.strip().split(".")
    if len(parts) != 4:
        return None
    rev_ip = ".".join(reversed(parts))
    qname = f"{rev_ip}.origin.asn.cymru.com"
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.settimeout(timeout)
        try:
            sock.connect(("1.1.1.1", 53))
            sock.sendall(_build_dns_txt_query(qname))
            resp = sock.recv(512)
        finally:
            sock.close()
        txt = _parse_dns_txt_response(resp)
        # Format: "13335 | 1.1.1.0/24 | US | apnic | 2011-08-11"
        fields = [f.strip() for f in txt.split("|")]
        if len(fields) >= 3 and fields[0]:
            asn_num = fields[0].split()[0]
            country = fields[2].upper() or "UNKNOWN"
            org = fields[3].upper() if len(fields) >= 4 else "Unknown"
            return f"AS{asn_num}", country, org
    except Exception as exc:
        logger.debug("Team Cymru DNS lookup fallback for %s: %s", ip, exc)
    return None


def query_rdap_ip(ip: str, timeout: float = 2.0) -> tuple[str, str, str] | None:
    """Look up IP country and network name via RDAP (`https://rdap.org/ip/<ip>`) with graceful fallback."""
    try:
        with httpx.Client(timeout=timeout, follow_redirects=True) as client:
            resp = client.get(f"https://rdap.org/ip/{ip}")
            if resp.status_code != 200:
                return None
            data = resp.json()
            if not isinstance(data, dict):
                return None
            country = str(data.get("country") or "UNKNOWN").upper()
            name = str(data.get("name") or data.get("handle") or "Unknown")
            asn = "AS0"
            # Check remarks or autnum hints if present
            for ent in data.get("entities", []) or []:
                if isinstance(ent, dict) and str(ent.get("handle", "")).upper().startswith("AS"):
                    asn = str(ent["handle"]).upper()
                    break
            return asn, country, name
    except Exception as exc:
        logger.debug("RDAP lookup fallback for %s: %s", ip, exc)
        return None


def resolve_ip_asn_country(ip: str, *, host_hint: str = "") -> tuple[str, str, str]:
    """Resolve IP -> (asn, country, org) using cache -> injectable resolver -> Team Cymru DNS -> RDAP -> fallback."""
    if not ip or ip == "*":
        return "AS0", "UNKNOWN", "Unknown"

    if _CUSTOM_ASN_RESOLVER is not None:
        return _CUSTOM_ASN_RESOLVER(ip)

    if ip.startswith("127.") or ip in ("::1", "localhost"):
        h_low = host_hint.lower()
        if "bd" in h_low or "domestic" in h_low:
            return "AS58717", "BD", "BDIX-Domestic-Sim"
        if "cached" in h_low or "google" in h_low or "fbcdn" in h_low:
            return "AS15169", "BD", "ISP-Edge-Cache-Sim"
        return "AS13335", "US", "Loopback-Sim"

    cached = ThrottleStore.get_cached_asn(ip)
    if cached is not None:
        return cached

    cymru = query_team_cymru_dns(ip)
    if cymru is not None:
        asn, country, org = cymru
        if country == "UNKNOWN":
            rdap = query_rdap_ip(ip)
            if rdap and rdap[1] != "UNKNOWN":
                country = rdap[1]
                org = rdap[2] or org
        ThrottleStore.set_cached_asn(ip, asn, country, org)
        return asn, country, org

    rdap = query_rdap_ip(ip)
    if rdap is not None:
        ThrottleStore.set_cached_asn(ip, rdap[0], rdap[1], rdap[2])
        return rdap

    return "AS0", "UNKNOWN", "Unknown"


def load_target_candidates(
    *,
    targets_file: Path | None = None,
    domestic_urls: list[str] | None = None,
    own_server_url: str | None = None,
) -> list[TargetCandidate]:
    """Load target candidates from default_targets.toml, optional custom TOML file, and CLI flags."""
    raw_bytes = DEFAULT_TARGETS_PATH.read_bytes()
    base_doc = tomllib.loads(raw_bytes.decode("utf-8"))

    # Merge user targets.toml if present in PDL_HOME or passed via --targets
    custom_path = targets_file or (get_targets_file() if get_targets_file().is_file() else None)
    if custom_path and Path(custom_path).is_file():
        custom_doc = tomllib.loads(Path(custom_path).read_text(encoding="utf-8"))
        # If user provides a custom --targets file, use its groups (or merge)
        if targets_file is not None:
            base_doc = custom_doc
        else:
            for grp_name, grp_val in (custom_doc.get("groups") or {}).items():
                if isinstance(grp_val, dict) and "targets" in grp_val:
                    base_doc.setdefault("groups", {}).setdefault(grp_name, {})["targets"] = grp_val["targets"]

    candidates: list[TargetCandidate] = []
    groups_dict: dict[str, Any] = base_doc.get("groups", {})

    for grp_name, grp_data in groups_dict.items():
        if not isinstance(grp_data, dict):
            continue
        t_list = grp_data.get("targets", [])
        if not isinstance(t_list, list):
            continue
        for idx, item in enumerate(t_list):
            if not isinstance(item, dict) or not item.get("url"):
                continue
            url = str(item["url"]).strip()
            t_id = str(item.get("id") or f"{grp_name}-{idx + 1}")
            candidates.append(
                TargetCandidate(
                    id=t_id,
                    url=url,
                    group=grp_name,
                    label=str(item.get("label") or t_id),
                    alt_port=int(item["alt_port"]) if item.get("alt_port") is not None else None,
                    http_url=str(item["http_url"]) if item.get("http_url") else None,
                    supports_upload=bool(item.get("supports_upload", False)),
                    sni_probe_allowed=bool(item.get("sni_probe_allowed", False)),
                    sni_alt_host=str(item["sni_alt_host"]) if item.get("sni_alt_host") else None,
                )
            )

    # Append repeatable --domestic HOST_OR_URL entries
    for idx, dom_raw in enumerate(domestic_urls or []):
        s = dom_raw.strip()
        if not s:
            continue
        url = s if s.startswith(("http://", "https://")) else f"http://{s}"
        parsed = urlparse(url)
        host = parsed.hostname or f"domestic-{idx + 1}"
        candidates.append(
            TargetCandidate(
                id=f"domestic-cli-{idx + 1}",
                url=url,
                group="domestic",
                label=f"Domestic ({host})",
            )
        )

    if own_server_url:
        u = own_server_url.strip()
        if not u.startswith(("http://", "https://")):
            u = f"http://{u}"
        candidates.append(
            TargetCandidate(
                id="own-server-cli",
                url=u,
                group="own-server",
                label="VPS Own-Server",
                supports_upload=True,
            )
        )

    return candidates


def validate_single_candidate(
    cand: TargetCandidate,
    *,
    timeout: float = 4.0,
    home_country: str = "BD",
) -> ValidatedTarget | None:
    """Validate a single TargetCandidate at runtime (status, Range support, size, TLS).

    Returns None on any failure so unreachable/broken candidates are silently dropped.
    """
    parsed = urlparse(cand.url)
    host = (parsed.hostname or "").lower()
    if not host or parsed.scheme not in ("http", "https"):
        return None

    # Resolve IP
    try:
        infos = socket.getaddrinfo(host, parsed.port or (443 if parsed.scheme == "https" else 80))
        ip = str(infos[0][4][0]) if infos else "127.0.0.1"
    except OSError:
        ip = "127.0.0.1" if host in ("localhost", "127.0.0.1") else ""
        if not ip:
            return None

    # Probe HTTP Range bytes=0-1023
    headers = {
        "User-Agent": "pdl-throttle/4.0",
        "Range": "bytes=0-1023",
    }
    try:
        with httpx.Client(timeout=timeout, follow_redirects=True) as client:
            with client.stream("GET", cand.url, headers=headers) as resp:
                status = resp.status_code
                if status not in (200, 206):
                    return None
                supports_range = (status == 206) or (resp.headers.get("Accept-Ranges", "").lower() == "bytes")
                content_len: int | None = None
                cr = resp.headers.get("Content-Range", "")
                if "/" in cr:
                    tail = cr.split("/")[-1].strip()
                    if tail.isdigit():
                        content_len = int(tail)
                if content_len is None:
                    cl = resp.headers.get("Content-Length", "")
                    if cl.isdigit():
                        content_len = int(cl)
                # Read up to 1 KiB to verify body stream works
                for _chunk in resp.iter_bytes(chunk_size=1024):
                    break
    except Exception as exc:
        logger.debug("Silently dropping unreachable target candidate %s (%s): %s", cand.id, cand.url, exc)
        return None

    asn, country, org = resolve_ip_asn_country(ip, host_hint=f"{cand.group}:{cand.id}:{cand.url}")

    # Auto-label locality (domestic vs international vs isp-cached)
    if cand.group == "isp-cached" or "cached" in cand.url.lower():
        locality = "isp-cached"
    elif (
        cand.group == "domestic"
        or country.upper() == home_country.upper()
        or "domestic" in cand.url.lower()
        or "bdix" in cand.url.lower()
    ):
        locality = "domestic"
    else:
        locality = "international"

    return ValidatedTarget(
        id=cand.id,
        url=cand.url,
        host=host,
        ip=ip,
        group=cand.group,
        label=cand.label or cand.id,
        status_code=status,
        supports_range=supports_range,
        content_length=content_len,
        tls_ok=(parsed.scheme == "https"),
        country=country,
        asn=asn,
        asn_org=org,
        locality=locality,
        alt_port=cand.alt_port,
        http_url=cand.http_url,
        supports_upload=cand.supports_upload,
        sni_probe_allowed=cand.sni_probe_allowed,
        sni_alt_host=cand.sni_alt_host,
    )


def validate_candidates(
    candidates: list[TargetCandidate],
    *,
    timeout: float = 4.0,
    home_country: str = "BD",
) -> list[ValidatedTarget]:
    """Validate all candidate URLs and silently drop any that fail."""
    validated: list[ValidatedTarget] = []
    for cand in candidates:
        vt = validate_single_candidate(cand, timeout=timeout, home_country=home_country)
        if vt is not None:
            validated.append(vt)
    return validated
