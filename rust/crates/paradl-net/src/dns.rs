//! DNS resolution: system resolver, IP-family selection, and opt-in
//! DNS-over-HTTPS (the `--doh-endpoint` capability) via hickory-dns.
//!
//! Without the `doh` feature the system resolver is used exclusively; the
//! endpoint string is still accepted and validated by [`DohEndpoint::parse`]
//! so that CLI/config plumbing can be exercised in a default build.

use std::net::IpAddr;

use tracing::debug;
use url::Url;

use crate::{Error, Result};

/// Preferred IP family for outbound connections.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IpFamily {
    /// Use both IPv4 and IPv6, in the order the resolver returns them.
    #[default]
    Auto,
    /// Keep only IPv4 addresses.
    Ipv4,
    /// Keep only IPv6 addresses.
    Ipv6,
}

/// Filter resolved addresses by [`IpFamily`], preserving resolver order and
/// removing duplicates.
///
/// ```
/// use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
/// use paradl_net::{IpFamily, filter_addresses};
/// let addrs = [
///     IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
///     IpAddr::V6(Ipv6Addr::LOCALHOST),
/// ];
/// assert_eq!(filter_addresses(&addrs, IpFamily::Ipv4), vec![addrs[0]]);
/// ```
pub fn filter_addresses(addresses: &[IpAddr], family: IpFamily) -> Vec<IpAddr> {
    let mut out = Vec::with_capacity(addresses.len());
    for addr in addresses {
        if !matches!(
            (family, addr),
            (IpFamily::Ipv4, IpAddr::V6(_)) | (IpFamily::Ipv6, IpAddr::V4(_))
        ) && !out.contains(addr)
        {
            out.push(*addr);
        }
    }
    out
}

/// A parsed DNS-over-HTTPS endpoint (the `--doh-endpoint` URL).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DohEndpoint {
    host: String,
    port: u16,
    path: String,
    tls_name: String,
    https: bool,
}

impl DohEndpoint {
    /// Parse a DoH endpoint URL. Only `http`/`https` schemes are accepted;
    /// an empty path defaults to `/dns-query` (RFC 8484).
    pub fn parse(endpoint: &str) -> Result<Self> {
        let parsed = Url::parse(endpoint)
            .map_err(|e| Error::usage(format!("Invalid DoH endpoint {endpoint:?}: {e}")))?;
        let https = match parsed.scheme() {
            "https" => true,
            "http" => false,
            other => {
                return Err(Error::usage(format!(
                    "DoH endpoint must use http or https, got {other:?}"
                )))
            }
        };
        let host = parsed
            .host_str()
            .filter(|host| !host.is_empty())
            .ok_or_else(|| Error::usage(format!("DoH endpoint has no host: {endpoint:?}")))?
            .trim_end_matches('.')
            .to_owned();
        let port = parsed.port_or_known_default().unwrap_or(if https { 443 } else { 80 });
        let raw_path = parsed.path();
        let path = if raw_path.is_empty() || raw_path == "/" {
            "/dns-query".to_owned()
        } else {
            raw_path.to_owned()
        };
        Ok(Self {
            tls_name: host.clone(),
            host,
            port,
            path,
            https,
        })
    }

    /// The endpoint's hostname or IP literal.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The endpoint's TCP port (default 443 for https, 80 for http).
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The DNS query path (default `/dns-query`).
    pub fn path(&self) -> &str {
        &self.path
    }

    /// The TLS SNI name (same as the host).
    pub fn tls_name(&self) -> &str {
        &self.tls_name
    }

    /// Whether the endpoint uses HTTPS.
    pub fn is_https(&self) -> bool {
        self.https
    }

    /// The endpoint as a `host:port` socket address string.
    pub fn authority(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

/// Resolve `host` to IP addresses using the configured strategy.
///
/// When the `doh` feature is enabled and `doh_endpoint` is `Some`, DoH is
/// attempted first; any failure falls back to the system resolver. Without
/// the feature, the system resolver is always used. The returned addresses are
/// filtered by `family` and de-duplicated.
pub async fn resolve_dns(host: &str, doh_endpoint: Option<&str>, family: IpFamily) -> Vec<IpAddr> {
    if host.is_empty() {
        return Vec::new();
    }

    #[cfg(feature = "doh")]
    if let Some(endpoint) = doh_endpoint {
        let resolved = resolve_dns_doh(host, endpoint, family).await;
        if !resolved.is_empty() {
            return resolved;
        }
        debug!("DoH resolution for {host} returned no addresses; using system resolver");
    }

    #[cfg(not(feature = "doh"))]
    if doh_endpoint.is_some() {
        debug!("doh feature is not compiled in; --doh-endpoint ignored for {host}");
    }

    let mut addresses = Vec::new();
    match tokio::net::lookup_host((host, 0)).await {
        Ok(sockets) => {
            for socket in sockets {
                let ip = socket.ip();
                if !addresses.contains(&ip) {
                    addresses.push(ip);
                }
            }
        }
        Err(error) => debug!("System DNS resolution for {host} failed: {error}"),
    }
    filter_addresses(&addresses, family)
}

/// Re-resolve a host with the system resolver, mirroring Python's
/// `re_resolve_dns`. Returns de-duplicated IPv4 and IPv6 addresses.
pub async fn re_resolve_dns(host: &str) -> Vec<IpAddr> {
    resolve_dns(host, None, IpFamily::Auto).await
}

/// Resolve `host` through a DNS-over-HTTPS endpoint using hickory-dns's
/// resolver client. Available only with the `doh` feature.
///
/// Returns an empty vector on any parse, transport, or lookup failure so that
/// callers can fall back to the system resolver.
#[cfg(feature = "doh")]
pub async fn resolve_dns_doh(host: &str, endpoint: &str, family: IpFamily) -> Vec<IpAddr> {
    use std::net::SocketAddr;

    use hickory_resolver::Resolver;
    use hickory_resolver::config::{NameServerConfig, ResolverConfig};
    use hickory_resolver::name_server::TokioConnectionProvider;
    use hickory_resolver::proto::xfer::Protocol;

    if host.is_empty() {
        return Vec::new();
    }
    let Ok(doh) = DohEndpoint::parse(endpoint) else {
        return Vec::new();
    };

    // hickory needs a concrete IP for the upstream; resolve the endpoint host
    // through the system resolver first (the bootstrap hop).
    let server_addr: SocketAddr = match tokio::net::lookup_host((doh.host(), doh.port())).await {
        Ok(mut addresses) => match addresses.next() {
            Some(addr) => addr,
            None => return Vec::new(),
        },
        Err(error) => {
            debug!("DoH endpoint bootstrap resolution failed: {error}");
            return Vec::new();
        }
    };

    let mut name_server = NameServerConfig::new(server_addr, Protocol::Https);
    name_server.tls_dns_name = Some(doh.tls_name().to_owned());
    name_server.http_endpoint = Some(doh.path().to_owned());
    let mut config = ResolverConfig::new();
    config.add_name_server(name_server);

    let resolver = Resolver::builder_with_config(config, TokioConnectionProvider::default()).build();
    match resolver.lookup_ip(host).await {
        Ok(lookup) => {
            let addresses: Vec<IpAddr> = lookup.iter().collect();
            filter_addresses(&addresses, family)
        }
        Err(error) => {
            debug!("DoH lookup for {host} via {endpoint} failed: {error}");
            Vec::new()
        }
    }
}

/// Resolve `host` through DNS-over-HTTPS. Without the `doh` feature this is a
/// no-op returning an empty vector.
#[cfg(not(feature = "doh"))]
pub async fn resolve_dns_doh(_host: &str, _endpoint: &str, _family: IpFamily) -> Vec<IpAddr> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn filter_preserves_order_and_deduplicates() {
        let addrs = [
            IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
            IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ];
        assert_eq!(
            filter_addresses(&addrs, IpFamily::Auto),
            vec![addrs[0], addrs[2]]
        );
    }

    #[test]
    fn doh_endpoint_authority() {
        let e = DohEndpoint::parse("https://1.1.1.1:8443/custom-path").unwrap();
        assert_eq!(e.authority(), "1.1.1.1:8443");
        assert_eq!(e.path(), "/custom-path");
        assert!(e.is_https());
    }
}
