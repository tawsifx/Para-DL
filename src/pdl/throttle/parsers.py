"""Pure text parsers for macOS network diagnostic CLI outputs (ping, traceroute, netstat, nettop, networksetup, scutil)."""

from __future__ import annotations

import re
from typing import Any

from pdl.throttle.models import CompetingProcess, TracerouteHop

_PING_LOSS_RE = re.compile(
    r"(\d+)\s+packets\s+transmitted,\s+(\d+)\s+(?:packets\s+)?received,\s+([0-9.]+)%\s+packet\s+loss",
    re.IGNORECASE,
)
_PING_RTT_RE = re.compile(
    r"(?:round-trip|rtt)\s+min/avg/max/(?:stddev|mdev)\s*=\s*([0-9.]+)/([0-9.]+)/([0-9.]+)/([0-9.]+)\s*ms",
    re.IGNORECASE,
)
_PING_DF_ERR_RE = re.compile(
    r"(?:Message too long|frag needed and DF set|Packet too big|fragneeded)",
    re.IGNORECASE,
)

_TRACEROUTE_HOP_RE = re.compile(r"^\s*(\d+)\s+(.+)$")
_IP_IN_PARENS_RE = re.compile(r"\(([0-9]{1,3}(?:\.[0-9]{1,3}){3}|[0-9a-fA-F:]+)\)")
_BARE_IPV4_RE = re.compile(r"\b([0-9]{1,3}(?:\.[0-9]{1,3}){3})\b")
_RTT_MS_RE = re.compile(r"([0-9]+(?:\.[0-9]+)?)\s*ms")

_TCP_PACKETS_SENT_RE = re.compile(r"(\d+)\s+packets?\s+sent", re.IGNORECASE)
_TCP_DATA_PACKETS_RE = re.compile(r"(\d+)\s+data\s+packets?\s*\((\d+)\s+bytes?\)", re.IGNORECASE)
_TCP_RETRANS_RE = re.compile(
    r"(\d+)\s+data\s+packets?\s*\((\d+)\s+bytes?\)\s+retransmitted",
    re.IGNORECASE,
)
_TCP_RESETS_RE = re.compile(r"(\d+)\s+(?:connection\s+resets?|Connections?\s+reset)", re.IGNORECASE)
_DEFAULT_ROUTE_RE = re.compile(r"^\s*default\s+(\S+)\s+\S+\s+(\S+)", re.MULTILINE)

_AIRPORT_SSID_RE = re.compile(r"Current (?:Wi-Fi|AirPort) Network:\s*(.+)", re.IGNORECASE)
_LINK_RATE_RE = re.compile(r"(?:lastTxRate|LinkRate|maxRate|link rate)\s*[:=]\s*([0-9.]+)", re.IGNORECASE)
_PRIMARY_IFACE_RE = re.compile(r"PrimaryInterface\s*:\s*([A-Za-z0-9_]+)")
_SSID_STR_RE = re.compile(r"SSID_STR\s*:\s*(.+)")
_TUNNEL_IFACE_RE = re.compile(r"\b(utun\d+|ppp\d+|tun\d+|tap\d+|ipsec\d+|wg\d+)\b")


def parse_ping(output: str) -> dict[str, Any]:
    """Parse macOS `ping` (and `ping -D` MTU probe) stdout/stderr."""
    text = output or ""
    transmitted = 0
    received = 0
    loss_pct = 100.0

    m_loss = _PING_LOSS_RE.search(text)
    if m_loss:
        transmitted = int(m_loss.group(1))
        received = int(m_loss.group(2))
        loss_pct = float(m_loss.group(3))

    rtt_min = 0.0
    rtt_avg = 0.0
    rtt_max = 0.0
    jitter = 0.0

    m_rtt = _PING_RTT_RE.search(text)
    if m_rtt:
        rtt_min = float(m_rtt.group(1))
        rtt_avg = float(m_rtt.group(2))
        rtt_max = float(m_rtt.group(3))
        jitter = float(m_rtt.group(4))
        if transmitted == 0:
            transmitted = 1
            received = 1
            loss_pct = 0.0

    df_exceeded = bool(_PING_DF_ERR_RE.search(text))
    ok = (received > 0) and not df_exceeded

    return {
        "ok": ok,
        "transmitted": transmitted,
        "received": received,
        "loss_pct": loss_pct,
        "rtt_min_ms": rtt_min,
        "rtt_avg_ms": rtt_avg,
        "rtt_max_ms": rtt_max,
        "jitter_ms": jitter,
        "df_fragmentation_needed": df_exceeded,
        "mtu_exceeded": df_exceeded,
    }


def parse_traceroute(output: str) -> list[TracerouteHop]:
    """Parse `traceroute -n -q 1 -w 1` (or standard macOS traceroute) into TracerouteHop items."""
    hops: list[TracerouteHop] = []
    for raw_line in (output or "").splitlines():
        line = raw_line.strip()
        if not line or line.lower().startswith("traceroute to"):
            continue
        m_hop = _TRACEROUTE_HOP_RE.match(line)
        if not m_hop:
            continue
        hop_num = int(m_hop.group(1))
        rest = m_hop.group(2).strip()
        if rest.startswith("*"):
            hops.append(TracerouteHop(hop=hop_num, ip="*", rtt_ms=None))
            continue

        ip = ""
        m_paren = _IP_IN_PARENS_RE.search(rest)
        if m_paren:
            ip = m_paren.group(1)
        else:
            m_ipv4 = _BARE_IPV4_RE.search(rest)
            if m_ipv4:
                ip = m_ipv4.group(1)
            else:
                first_tok = rest.split()[0] if rest.split() else "*"
                ip = first_tok

        m_rtt = _RTT_MS_RE.search(rest)
        rtt = float(m_rtt.group(1)) if m_rtt else None
        hops.append(TracerouteHop(hop=hop_num, ip=ip, rtt_ms=rtt))

    return hops


def parse_netstat_tcp(output: str) -> dict[str, Any]:
    """Parse `netstat -s -p tcp` statistics and optional routing table lines."""
    text = output or ""
    packets_sent = 0
    data_packets = 0
    data_bytes = 0
    retrans_packets = 0
    retrans_bytes = 0
    resets = 0
    default_gateway = ""
    default_interface = ""

    m_sent = _TCP_PACKETS_SENT_RE.search(text)
    if m_sent:
        packets_sent = int(m_sent.group(1))

    # Match all data packets lines, distinguishing normal from retransmitted
    for raw_line in text.splitlines():
        line = raw_line.strip()
        m_ret = _TCP_RETRANS_RE.search(line)
        if m_ret:
            retrans_packets += int(m_ret.group(1))
            retrans_bytes += int(m_ret.group(2))
            continue
        m_dp = _TCP_DATA_PACKETS_RE.search(line)
        if m_dp and "retransmit" not in line.lower():
            data_packets += int(m_dp.group(1))
            data_bytes += int(m_dp.group(2))

    m_rst = _TCP_RESETS_RE.search(text)
    if m_rst:
        resets = int(m_rst.group(1))

    m_route = _DEFAULT_ROUTE_RE.search(text)
    if m_route:
        default_gateway = m_route.group(1)
        default_interface = m_route.group(2)

    base_pkts = max(packets_sent, data_packets)
    retrans_pct = (retrans_packets / base_pkts * 100.0) if base_pkts > 0 else 0.0

    return {
        "packets_sent": packets_sent,
        "data_packets": data_packets,
        "data_bytes": data_bytes,
        "retransmitted_packets": retrans_packets,
        "retransmitted_bytes": retrans_bytes,
        "retransmit_rate_pct": round(retrans_pct, 4),
        "connection_resets": resets,
        "default_gateway": default_gateway,
        "default_interface": default_interface,
    }


def parse_nettop(output: str) -> list[CompetingProcess]:
    """Parse macOS `nettop` snapshot into a list of active network processes."""
    procs: list[CompetingProcess] = []
    for raw_line in (output or "").splitlines():
        line = raw_line.strip()
        if not line:
            continue
        low = line.lower()
        if "bytes_in" in low or "interface" in low:
            continue

        # Support both whitespace-aligned columns and CSV format
        parts = [p.strip() for p in line.split(",") if p.strip()] if "," in line else line.split()
        if len(parts) < 4:
            continue

        try:
            bytes_out = int(parts[-1])
            bytes_in = int(parts[-2])
            state = parts[-3] if len(parts) >= 5 else "Established"
            iface = parts[-4] if len(parts) >= 5 else parts[1]
            proc_name = parts[0]
            procs.append(
                CompetingProcess(
                    process=proc_name,
                    interface=iface,
                    state=state,
                    bytes_in=bytes_in,
                    bytes_out=bytes_out,
                )
            )
        except ValueError:
            continue

    return procs


def parse_networksetup(output: str) -> dict[str, Any]:
    """Parse `networksetup -getairportnetwork <iface>` and hardware port output."""
    text = output or ""
    ssid = ""
    is_wifi_off = "wi-fi power is currently off" in text.lower() or "not associated" in text.lower()
    m_ssid = _AIRPORT_SSID_RE.search(text)
    if m_ssid and not is_wifi_off:
        cand = m_ssid.group(1).strip()
        if cand and cand != "<redacted>":
            ssid = cand

    m_rate = _LINK_RATE_RE.search(text)
    link_mbps = float(m_rate.group(1)) if m_rate else None

    return {
        "ssid": ssid,
        "wifi_associated": bool(ssid) and not is_wifi_off,
        "wifi_link_mbps": link_mbps,
    }


def parse_scutil_env(output: str) -> dict[str, Any]:
    """Parse `scutil` / `scutil --nwi` output for primary interface, SSID, link rate, and VPN tunnels."""
    text = output or ""
    primary_iface = "en0"
    m_iface = _PRIMARY_IFACE_RE.search(text)
    if m_iface:
        primary_iface = m_iface.group(1).strip()

    ssid = ""
    m_ssid = _SSID_STR_RE.search(text)
    if m_ssid:
        cand = m_ssid.group(1).strip()
        if cand and cand not in ("<redacted>", "''", '""'):
            ssid = cand

    tunnels = sorted(set(_TUNNEL_IFACE_RE.findall(text)))
    m_rate = _LINK_RATE_RE.search(text)
    link_mbps = float(m_rate.group(1)) if m_rate else None

    iface_type = "Wi-Fi" if ssid else ("VPN/Tunnel" if primary_iface.startswith(("utun", "ppp", "tun")) else "Ethernet")

    return {
        "primary_interface": primary_iface,
        "ssid": ssid,
        "interface_type": iface_type,
        "tunnel_interfaces": tunnels,
        "vpn_or_tunnel_active": len(tunnels) > 0,
        "wifi_link_mbps": link_mbps,
    }
