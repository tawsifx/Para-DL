//! Pure target-construction, locality classification, and DNS TXT helpers.
//!
//! Port of the deterministic parts of `src/paradl/throttle/targets.py`. Network
//! validation (HTTP range probes, RDAP, UDP DNS) lives in the net crate; these
//! helpers work only on already-known strings and bytes.

use crate::models::{TargetCandidate, ValidatedTarget};

/// Parsed URL components relevant to target validation.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TargetUrlParts {
    /// Lower-cased scheme.
    pub scheme: String,
    /// Lower-cased hostname.
    pub host: String,
    /// Explicit port, when present.
    pub port: Option<u16>,
}

/// Parse the scheme, hostname, and port of an HTTP(S) URL.
///
/// Mirrors the subset of `urllib.parse.urlparse` used by target validation.
pub fn parse_target_url(url: &str) -> Option<TargetUrlParts> {
    let s = url.trim();
    let (scheme, rest) = s.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    if authority.is_empty() {
        return None;
    }
    let host = match authority.rsplit_once('@') {
        Some((_, hostport)) => hostport,
        None => authority,
    };
    let default_port = if scheme == "https" { 443 } else { 80 };
    if let Some((h, p)) = host.rsplit_once(':') {
        if !h.is_empty() && p.chars().all(|c| c.is_ascii_digit()) {
            let port = p.parse::<u16>().ok()?;
            return Some(TargetUrlParts {
                scheme,
                host: h.trim_end_matches('.').to_ascii_lowercase(),
                port: Some(port),
            });
        }
    }
    Some(TargetUrlParts {
        scheme,
        host: host.trim_end_matches('.').to_ascii_lowercase(),
        port: Some(default_port),
    })
}

/// Determine the locality group for a validated target.
///
/// Mirrors `validate_single_candidate`: explicit `isp-cached` group or a URL
/// containing `cached` wins first, then domestic group/country/keywords.
pub fn classify_locality(
    group: &str,
    url: &str,
    country: &str,
    home_country: &str,
) -> &'static str {
    let low_url = url.to_ascii_lowercase();
    if group == "isp-cached" || low_url.contains("cached") {
        "isp-cached"
    } else if group == "domestic"
        || country.eq_ignore_ascii_case(home_country)
        || low_url.contains("domestic")
        || low_url.contains("bdix")
    {
        "domestic"
    } else {
        "international"
    }
}

/// Loopback ASN simulation used when probing local test servers.
///
/// Mirrors `resolve_ip_asn_country` for `127.*`, `::1`, and `localhost`.
pub fn loopback_asn(host_hint: &str) -> (&'static str, &'static str, &'static str) {
    let low = host_hint.to_ascii_lowercase();
    if low.contains("bd") || low.contains("domestic") {
        ("AS58717", "BD", "BDIX-Domestic-Sim")
    } else if low.contains("cached") || low.contains("google") || low.contains("fbcdn") {
        ("AS15169", "BD", "ISP-Edge-Cache-Sim")
    } else {
        ("AS13335", "US", "Loopback-Sim")
    }
}

/// Build a synthetic domestic candidate from a repeatable `--domestic` value.
pub fn build_domestic_candidate(raw: &str, index: usize) -> TargetCandidate {
    let s = raw.trim();
    let url = if s.starts_with("http://") || s.starts_with("https://") {
        s.to_owned()
    } else {
        format!("http://{s}")
    };
    let host = parse_target_url(&url)
        .map(|p| p.host)
        .unwrap_or_else(|| format!("domestic-{}", index + 1));
    TargetCandidate {
        id: format!("domestic-cli-{}", index + 1),
        url,
        group: "domestic".to_owned(),
        label: format!("Domestic ({host})"),
        ..TargetCandidate::default()
    }
}

/// Build the synthetic own-server candidate from `--via-proxy`/VPS flags.
pub fn build_own_server_candidate(raw: &str) -> TargetCandidate {
    let trimmed = raw.trim();
    let url = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_owned()
    } else {
        format!("http://{trimmed}")
    };
    TargetCandidate {
        id: "own-server-cli".to_owned(),
        url,
        group: "own-server".to_owned(),
        label: "VPS Own-Server".to_owned(),
        supports_upload: true,
        ..TargetCandidate::default()
    }
}

/// Append CLI-provided candidates to a list loaded from a targets file.
pub fn append_cli_candidates(
    candidates: &mut Vec<TargetCandidate>,
    domestic_urls: &[String],
    own_server_url: Option<&str>,
) {
    for (idx, dom) in domestic_urls.iter().enumerate() {
        if !dom.trim().is_empty() {
            candidates.push(build_domestic_candidate(dom, idx));
        }
    }
    if let Some(url) = own_server_url {
        if !url.trim().is_empty() {
            candidates.push(build_own_server_candidate(url));
        }
    }
}

/// Build a minimal RFC 1035 DNS TXT query packet (QTYPE=16, QCLASS=1).
///
/// Byte-exact against `targets._build_dns_txt_query`.
pub fn build_dns_txt_query(qname: &str) -> Vec<u8> {
    let mut packet = vec![
        0x42, 0x42, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];
    for part in qname.trim_matches('.').split('.') {
        if !part.is_empty() {
            packet.push(part.len() as u8);
            packet.extend_from_slice(part.as_bytes());
        }
    }
    packet.push(0);
    packet.extend_from_slice(&[0x00, 0x10, 0x00, 0x01]);
    packet
}

fn skip_name(packet: &[u8], mut idx: usize) -> usize {
    while idx < packet.len() && packet[idx] != 0 {
        if packet[idx] & 0xC0 == 0xC0 {
            return idx + 2;
        }
        idx += 1 + packet[idx] as usize;
    }
    idx + 1
}

/// Extract the first TXT string from a raw DNS UDP response packet.
///
/// Byte-exact against `targets._parse_dns_txt_response`.
pub fn parse_dns_txt_response(packet: &[u8]) -> String {
    if packet.len() < 12 {
        return String::new();
    }
    let ancount = u16::from_be_bytes([packet[6], packet[7]]);
    if ancount < 1 {
        return String::new();
    }
    let mut idx = skip_name(packet, 12) + 4;
    for _ in 0..ancount {
        if idx >= packet.len() {
            break;
        }
        idx = skip_name(packet, idx);
        if idx + 10 > packet.len() {
            break;
        }
        let rtype = u16::from_be_bytes([packet[idx], packet[idx + 1]]);
        let rdlen = u16::from_be_bytes([packet[idx + 8], packet[idx + 9]]) as usize;
        idx += 10;
        let rdata = packet.get(idx..idx + rdlen).unwrap_or_default();
        idx += rdlen;
        if rtype == 16 && rdata.len() > 1 {
            let txt_len = rdata[0] as usize;
            let text = &rdata[1..1 + txt_len.min(rdata.len() - 1)];
            return String::from_utf8_lossy(text).into_owned();
        }
    }
    String::new()
}

/// Convert a validated target into a redacted, log-safe summary string.
pub fn redacted_target_summary(target: &ValidatedTarget) -> String {
    format!("{} ({}) [{}]", target.host, target.ip, target.locality)
}
