//! Pure regex parsers for macOS network-diagnostic CLI output.
//!
//! Direct port of `src/paradl/throttle/parsers.py`. Every parser is a pure
//! function of its input string; no subprocess is spawned here. The regular
//! expressions are byte-equivalent to the Python `re` patterns (all use the
//! subset supported by the `regex` crate — no look-around is needed).

use regex::{Regex, RegexBuilder};
use std::sync::LazyLock;

use crate::models::{CompetingProcess, TracerouteHop};
use crate::numeric::py_round;

fn ci(pattern: &str) -> Regex {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .expect("static parser regex is valid")
}

static PING_LOSS_RE: LazyLock<Regex> = LazyLock::new(|| {
    ci(
        r"(\d+)\s+packets\s+transmitted,\s+(\d+)\s+(?:packets\s+)?received,\s+([0-9.]+)%\s+packet\s+loss",
    )
});
static PING_RTT_RE: LazyLock<Regex> = LazyLock::new(|| {
    ci(
        r"(?:round-trip|rtt)\s+min/avg/max/(?:stddev|mdev)\s*=\s*([0-9.]+)/([0-9.]+)/([0-9.]+)/([0-9.]+)\s*ms",
    )
});
static PING_DF_ERR_RE: LazyLock<Regex> =
    LazyLock::new(|| ci(r"(?:Message too long|frag needed and DF set|Packet too big|fragneeded)"));
static TRACEROUTE_HOP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(\d+)\s+(.+)$").unwrap());
static IP_IN_PARENS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(([0-9]{1,3}(?:\.[0-9]{1,3}){3}|[0-9a-fA-F:]+)\)").unwrap());
static BARE_IPV4_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([0-9]{1,3}(?:\.[0-9]{1,3}){3})\b").unwrap());
static RTT_MS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([0-9]+(?:\.[0-9]+)?)\s*ms").unwrap());
static TCP_PACKETS_SENT_RE: LazyLock<Regex> = LazyLock::new(|| ci(r"(\d+)\s+packets?\s+sent"));
static TCP_DATA_PACKETS_RE: LazyLock<Regex> =
    LazyLock::new(|| ci(r"(\d+)\s+data\s+packets?\s*\((\d+)\s+bytes?\)"));
static TCP_RETRANS_RE: LazyLock<Regex> =
    LazyLock::new(|| ci(r"(\d+)\s+data\s+packets?\s*\((\d+)\s+bytes?\)\s+retransmitted"));
static TCP_RESETS_RE: LazyLock<Regex> =
    LazyLock::new(|| ci(r"(\d+)\s+(?:connection\s+resets?|Connections?\s+reset)"));
static DEFAULT_ROUTE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*default\s+(\S+)\s+\S+\s+(\S+)").unwrap());
static AIRPORT_SSID_RE: LazyLock<Regex> =
    LazyLock::new(|| ci(r"Current (?:Wi-Fi|AirPort) Network:\s*(.+)"));
static LINK_RATE_RE: LazyLock<Regex> =
    LazyLock::new(|| ci(r"(?:lastTxRate|LinkRate|maxRate|link rate)\s*[:=]\s*([0-9.]+)"));
static PRIMARY_IFACE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"PrimaryInterface\s*:\s*([A-Za-z0-9_]+)").unwrap());
static SSID_STR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"SSID_STR\s*:\s*(.+)").unwrap());
static TUNNEL_IFACE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(utun\d+|ppp\d+|tun\d+|tap\d+|ipsec\d+|wg\d+)\b").unwrap());

fn parse_f64_or(text: &str, default: f64) -> f64 {
    text.parse::<f64>().unwrap_or(default)
}

fn parse_i64_or(text: &str, default: i64) -> i64 {
    text.parse::<i64>().unwrap_or(default)
}

/// Parsed `ping` output.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PingResult {
    /// Whether the probe succeeded.
    pub ok: bool,
    /// Packets transmitted.
    pub transmitted: i64,
    /// Packets received.
    pub received: i64,
    /// Loss percentage.
    pub loss_pct: f64,
    /// Minimum RTT in ms.
    pub rtt_min_ms: f64,
    /// Average RTT in ms.
    pub rtt_avg_ms: f64,
    /// Maximum RTT in ms.
    pub rtt_max_ms: f64,
    /// Jitter in ms.
    pub jitter_ms: f64,
    /// Whether a DF/fragmentation error was seen.
    pub df_fragmentation_needed: bool,
    /// Alias for [`PingResult::df_fragmentation_needed`].
    pub mtu_exceeded: bool,
}

/// Parse macOS `ping` (and `ping -D` MTU probe) stdout/stderr.
pub fn parse_ping(output: &str) -> PingResult {
    let mut transmitted = 0_i64;
    let mut received = 0_i64;
    let mut loss_pct = 100.0_f64;
    if let Some(caps) = PING_LOSS_RE.captures(output) {
        transmitted = parse_i64_or(&caps[1], 0);
        received = parse_i64_or(&caps[2], 0);
        loss_pct = parse_f64_or(&caps[3], 100.0);
    }
    let mut rtt_min = 0.0_f64;
    let mut rtt_avg = 0.0_f64;
    let mut rtt_max = 0.0_f64;
    let mut jitter = 0.0_f64;
    if let Some(caps) = PING_RTT_RE.captures(output) {
        rtt_min = parse_f64_or(&caps[1], 0.0);
        rtt_avg = parse_f64_or(&caps[2], 0.0);
        rtt_max = parse_f64_or(&caps[3], 0.0);
        jitter = parse_f64_or(&caps[4], 0.0);
        if transmitted == 0 {
            transmitted = 1;
            received = 1;
            loss_pct = 0.0;
        }
    }
    let df_exceeded = PING_DF_ERR_RE.is_match(output);
    PingResult {
        ok: received > 0 && !df_exceeded,
        transmitted,
        received,
        loss_pct,
        rtt_min_ms: rtt_min,
        rtt_avg_ms: rtt_avg,
        rtt_max_ms: rtt_max,
        jitter_ms: jitter,
        df_fragmentation_needed: df_exceeded,
        mtu_exceeded: df_exceeded,
    }
}

/// Parse `traceroute -n -q 1 -w 1` output into [`TracerouteHop`] items.
pub fn parse_traceroute(output: &str) -> Vec<TracerouteHop> {
    let mut hops = Vec::new();
    for raw_line in output.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.to_ascii_lowercase().starts_with("traceroute to") {
            continue;
        }
        let Some(caps) = TRACEROUTE_HOP_RE.captures(line) else {
            continue;
        };
        let hop_num = parse_i64_or(&caps[1], 0);
        let rest = caps[2].trim();
        if rest.starts_with('*') {
            hops.push(TracerouteHop {
                hop: hop_num,
                ip: "*".to_owned(),
                rtt_ms: None,
                ..TracerouteHop::default()
            });
            continue;
        }
        let ip = if let Some(m) = IP_IN_PARENS_RE.captures(rest) {
            m[1].to_owned()
        } else if let Some(m) = BARE_IPV4_RE.captures(rest) {
            m[1].to_owned()
        } else {
            rest.split_whitespace().next().unwrap_or("*").to_owned()
        };
        let rtt = RTT_MS_RE.captures(rest).map(|m| parse_f64_or(&m[1], 0.0));
        hops.push(TracerouteHop {
            hop: hop_num,
            ip,
            rtt_ms: rtt,
            ..TracerouteHop::default()
        });
    }
    hops
}

/// Parsed `netstat -s -p tcp` output.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetstatResult {
    /// Packets sent.
    pub packets_sent: i64,
    /// Data packets.
    pub data_packets: i64,
    /// Data bytes.
    pub data_bytes: i64,
    /// Retransmitted packets.
    pub retransmitted_packets: i64,
    /// Retransmitted bytes.
    pub retransmitted_bytes: i64,
    /// Retransmission rate percentage (rounded to 4 places).
    pub retransmit_rate_pct: f64,
    /// Connection resets.
    pub connection_resets: i64,
    /// Default gateway.
    pub default_gateway: String,
    /// Default interface.
    pub default_interface: String,
}

/// Parse `netstat -s -p tcp` statistics and optional routing-table lines.
pub fn parse_netstat_tcp(output: &str) -> NetstatResult {
    let mut packets_sent = 0_i64;
    if let Some(m) = TCP_PACKETS_SENT_RE.captures(output) {
        packets_sent = parse_i64_or(&m[1], 0);
    }
    let mut data_packets = 0_i64;
    let mut data_bytes = 0_i64;
    let mut retrans_packets = 0_i64;
    let mut retrans_bytes = 0_i64;
    for raw_line in output.lines() {
        let line = raw_line.trim();
        if let Some(m) = TCP_RETRANS_RE.captures(line) {
            retrans_packets += parse_i64_or(&m[1], 0);
            retrans_bytes += parse_i64_or(&m[2], 0);
            continue;
        }
        if let Some(m) = TCP_DATA_PACKETS_RE.captures(line) {
            if !line.to_ascii_lowercase().contains("retransmit") {
                data_packets += parse_i64_or(&m[1], 0);
                data_bytes += parse_i64_or(&m[2], 0);
            }
        }
    }
    let mut resets = 0_i64;
    if let Some(m) = TCP_RESETS_RE.captures(output) {
        resets = parse_i64_or(&m[1], 0);
    }
    let (mut gateway, mut iface) = (String::new(), String::new());
    if let Some(m) = DEFAULT_ROUTE_RE.captures(output) {
        gateway = m[1].to_owned();
        iface = m[2].to_owned();
    }
    let base_pkts = packets_sent.max(data_packets);
    let retrans_pct = if base_pkts > 0 {
        retrans_packets as f64 / base_pkts as f64 * 100.0
    } else {
        0.0
    };
    NetstatResult {
        packets_sent,
        data_packets,
        data_bytes,
        retransmitted_packets: retrans_packets,
        retransmitted_bytes: retrans_bytes,
        retransmit_rate_pct: py_round(retrans_pct, 4),
        connection_resets: resets,
        default_gateway: gateway,
        default_interface: iface,
    }
}

/// Parse a macOS `nettop` snapshot into active network processes.
pub fn parse_nettop(output: &str) -> Vec<CompetingProcess> {
    let mut procs = Vec::new();
    for raw_line in output.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let low = line.to_ascii_lowercase();
        if low.contains("bytes_in") || low.contains("interface") {
            continue;
        }
        let parts: Vec<&str> = if line.contains(',') {
            line.split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .collect()
        } else {
            line.split_whitespace().collect()
        };
        if parts.len() < 4 {
            continue;
        }
        let Ok(bytes_out) = parts[parts.len() - 1].parse::<i64>() else {
            continue;
        };
        let Ok(bytes_in) = parts[parts.len() - 2].parse::<i64>() else {
            continue;
        };
        let state = if parts.len() >= 5 {
            parts[parts.len() - 3]
        } else {
            "Established"
        };
        let iface = if parts.len() >= 5 {
            parts[parts.len() - 4]
        } else {
            parts[1]
        };
        procs.push(CompetingProcess {
            process: parts[0].to_owned(),
            interface: iface.to_owned(),
            state: state.to_owned(),
            bytes_in,
            bytes_out,
        });
    }
    procs
}

/// Parsed `networksetup` output.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetworksetupResult {
    /// Discovered SSID (empty when unavailable).
    pub ssid: String,
    /// Whether Wi-Fi is associated.
    pub wifi_associated: bool,
    /// Negotiated link rate in Mbps.
    pub wifi_link_mbps: Option<f64>,
}

/// Parse `networksetup -getairportnetwork` and hardware-port output.
pub fn parse_networksetup(output: &str) -> NetworksetupResult {
    let low = output.to_ascii_lowercase();
    let is_wifi_off =
        low.contains("wi-fi power is currently off") || low.contains("not associated");
    let mut ssid = String::new();
    if let Some(m) = AIRPORT_SSID_RE.captures(output) {
        if !is_wifi_off {
            let candidate = m[1].trim();
            if !candidate.is_empty() && candidate != "<redacted>" {
                ssid = candidate.to_owned();
            }
        }
    }
    let link_mbps = LINK_RATE_RE
        .captures(output)
        .map(|m| parse_f64_or(&m[1], 0.0));
    NetworksetupResult {
        wifi_associated: !ssid.is_empty() && !is_wifi_off,
        ssid,
        wifi_link_mbps: link_mbps,
    }
}

/// Parsed `scutil` / `scutil --nwi` output.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScutilResult {
    /// Primary interface name.
    pub primary_interface: String,
    /// Discovered SSID.
    pub ssid: String,
    /// `Wi-Fi`, `VPN/Tunnel`, or `Ethernet`.
    pub interface_type: String,
    /// Sorted tunnel interface names.
    pub tunnel_interfaces: Vec<String>,
    /// Whether a VPN/tunnel is active.
    pub vpn_or_tunnel_active: bool,
    /// Negotiated link rate in Mbps.
    pub wifi_link_mbps: Option<f64>,
}

/// Parse `scutil` / `scutil --nwi` output.
pub fn parse_scutil_env(output: &str) -> ScutilResult {
    let mut primary_iface = "en0".to_owned();
    if let Some(m) = PRIMARY_IFACE_RE.captures(output) {
        primary_iface = m[1].trim().to_owned();
    }
    let mut ssid = String::new();
    if let Some(m) = SSID_STR_RE.captures(output) {
        let candidate = m[1].trim();
        if !matches!(candidate, "<redacted>" | "''" | "\"\"") {
            ssid = candidate.to_owned();
        }
    }
    let mut tunnels: Vec<String> = TUNNEL_IFACE_RE
        .find_iter(output)
        .map(|m| m.as_str().to_owned())
        .collect();
    tunnels.sort();
    tunnels.dedup();
    let link_mbps = LINK_RATE_RE
        .captures(output)
        .map(|m| parse_f64_or(&m[1], 0.0));
    let vpn_active = !tunnels.is_empty();
    let interface_type = if !ssid.is_empty() {
        "Wi-Fi"
    } else if primary_iface.starts_with("utun")
        || primary_iface.starts_with("ppp")
        || primary_iface.starts_with("tun")
    {
        "VPN/Tunnel"
    } else {
        "Ethernet"
    };
    ScutilResult {
        primary_interface: primary_iface,
        ssid,
        interface_type: interface_type.to_owned(),
        tunnel_interfaces: tunnels,
        vpn_or_tunnel_active: vpn_active,
        wifi_link_mbps: link_mbps,
    }
}
