//! Where the remote control listens: the computer's local network addresses, one listener
//! each, never every address at once (`0.0.0.0` or `::`), so an adapter with a public address
//! (a modem plugged in directly, a VPN's) is never reached by surprise.

use std::net::IpAddr;

use mdm_engine::net::Adapter;

/// [`chosen`]'s choice for this computer only: other devices can't reach it (tests, a reverse
/// proxy on the same computer).
pub const THIS_COMPUTER: &str = "127.0.0.1";

/// A private address that the internet can't reach: IPv4's 10/8, 172.16/12 and 192.168/16, and
/// IPv6's unique local fc00::/7.
pub fn is_local_network(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private(),
        IpAddr::V6(v) => (v.segments()[0] & 0xfe00) == 0xfc00,
    }
}

/// The addresses to listen on for the choice `listen`: "" every local network address of
/// `adapters`; an address (this computer's `127.0.0.1` or `::1`, or one of an adapter's, even a
/// public one: the user chose it); an adapter's name, its addresses. Nothing when the choice
/// isn't there now, and never an unspecified address.
pub fn chosen(listen: &str, adapters: &[Adapter]) -> Vec<IpAddr> {
    let listen = listen.trim();
    let all = |a: &Adapter| -> Vec<IpAddr> {
        a.v4.iter()
            .map(|v| IpAddr::V4(*v))
            .chain(a.v6.iter().map(|v| IpAddr::V6(*v)))
            .collect()
    };
    let mut out: Vec<IpAddr> = if listen.is_empty() {
        adapters
            .iter()
            .flat_map(all)
            .filter(|ip| is_local_network(*ip))
            .collect()
    } else if let Ok(ip) = listen.trim_matches(['[', ']']).parse::<IpAddr>() {
        if ip.is_unspecified() {
            Vec::new()
        } else if ip.is_loopback() || adapters.iter().any(|a| all(a).contains(&ip)) {
            vec![ip]
        } else {
            Vec::new()
        }
    } else {
        adapters
            .iter()
            .filter(|a| a.name.eq_ignore_ascii_case(listen))
            .flat_map(all)
            .collect()
    };
    out.sort();
    out.dedup();
    out
}

/// How likely a phone reaches `ip`, best first: a home network's 192.168/16, then 10/8, then
/// 172.16/12 (often a virtual machine's or a container's own network), IPv6, this computer.
pub fn rank(ip: IpAddr) -> u8 {
    match ip {
        IpAddr::V4(v) if v.octets()[0] == 192 => 0,
        IpAddr::V4(v) if v.octets()[0] == 10 => 1,
        IpAddr::V4(v) if v.is_private() => 2,
        IpAddr::V4(v) if !v.is_loopback() => 3,
        IpAddr::V6(v) if !v.is_loopback() => 4,
        _ => 5,
    }
}

/// The page's address for a listener at `ip`:`port`.
pub fn url(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(v) => format!("https://{v}:{port}/"),
        IpAddr::V6(v) => format!("https://[{v}]:{port}/"),
    }
}

/// The computer's name, for the certificate and the names a request may use (`Host`).
pub fn host_name() -> Option<String> {
    // Windows sets COMPUTERNAME for every process; Linux keeps the name in /proc and
    // /etc/hostname; macOS has neither (its own call isn't used: the addresses are enough).
    let name = std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/proc/sys/kernel/hostname").ok())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())?;
    let name = name.trim().to_ascii_lowercase();
    (!name.is_empty() && name.len() <= 63).then_some(name)
}

/// Is `host` (a request's `Host` without its port) a name this server answers to? Any address
/// (the one a phone used may be the router's forwarding, or a container's host), `localhost`,
/// the computer's name with a local suffix, and the names the user added. Any other name is a
/// web page's (DNS rebinding) and is refused.
pub fn host_allowed(host: &str, extra: &[String]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .is_ok()
    {
        return true;
    }
    if host == "localhost" || extra.iter().any(|e| e.eq_ignore_ascii_case(&host)) {
        return true;
    }
    let Some(own) = host_name() else {
        return false;
    };
    let suffixes = ["", ".local", ".lan", ".home", ".home.arpa", ".localdomain"];
    suffixes.iter().any(|s| host == format!("{own}{s}"))
}

/// A `Host` header's name, without its port.
pub fn host_of(header: &str) -> &str {
    let header = header.trim();
    if header.starts_with('[') {
        return header.split_inclusive(']').next().unwrap_or(header);
    }
    match header.rsplit_once(':') {
        Some((name, port)) if port.bytes().all(|b| b.is_ascii_digit()) => name,
        _ => header,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn adapter(name: &str, v4: &[&str], v6: &[&str]) -> Adapter {
        Adapter {
            name: name.into(),
            index: 1,
            v4: v4.iter().map(|s| s.parse().unwrap()).collect(),
            v6: v6.iter().map(|s| s.parse().unwrap()).collect(),
            local_only: Vec::new(),
        }
    }

    #[test]
    fn only_local_network_addresses_by_default() {
        let list = [
            adapter("Wi-Fi", &["192.168.1.20"], &["fd12::5", "2a01:db8::1"]),
            adapter("Modem", &["81.2.3.4"], &[]),
            adapter("Docker", &["172.17.0.2"], &[]),
        ];
        let got = chosen("", &list);
        assert_eq!(
            got,
            vec![
                IpAddr::V4(Ipv4Addr::new(172, 17, 0, 2)),
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20)),
                "fd12::5".parse::<IpAddr>().unwrap(),
            ]
        );
        // Never everything at once, and a public address only when chosen by name.
        assert!(chosen("0.0.0.0", &list).is_empty());
        assert!(chosen("::", &list).is_empty());
        assert_eq!(chosen("81.2.3.4", &list).len(), 1);
        assert_eq!(chosen("modem", &list).len(), 1);
        assert!(chosen("10.0.0.9", &list).is_empty(), "not this computer's");
        assert_eq!(
            chosen(THIS_COMPUTER, &list),
            vec![IpAddr::V4(Ipv4Addr::LOCALHOST)]
        );
        assert_eq!(chosen("::1", &list), vec![IpAddr::V6(Ipv6Addr::LOCALHOST)]);
    }

    #[test]
    fn hosts() {
        assert_eq!(host_of("192.168.1.2:8463"), "192.168.1.2");
        assert_eq!(host_of("[fd00::1]:8463"), "[fd00::1]");
        assert_eq!(host_of("nas.local"), "nas.local");
        assert!(host_allowed("192.168.1.2", &[]));
        assert!(host_allowed("[fd00::1]", &[]));
        assert!(host_allowed("localhost", &[]));
        assert!(host_allowed("NAS.example", &["nas.example".into()]));
        assert!(
            !host_allowed("evil.example", &[]),
            "a rebinding page's name"
        );
        assert!(!host_allowed("192.168.1.2.evil.example", &[]));
    }

    #[test]
    fn urls() {
        assert_eq!(
            url("192.168.1.2".parse().unwrap(), 8463),
            "https://192.168.1.2:8463/"
        );
        assert_eq!(url("fd00::1".parse().unwrap(), 1), "https://[fd00::1]:1/");
    }
}
