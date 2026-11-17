//! IP blocklists for torrents (#40). The usual lists come as PeerGuardian/P2P text
//! (`name:1.2.3.4-1.2.3.255`) or eMule's `.dat` (`001.002.003.004 - 001.002.003.255 , 000 , name`),
//! often gzipped. librqbit reads only the first, rejects zero-padded addresses, and a list it
//! can't read stops the whole torrent session from starting: MDM converts a list itself, to
//! plain P2P lines it has checked, and hands librqbit that file.

use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// A list, converted.
#[derive(Debug, PartialEq, Eq)]
pub struct Converted {
    /// P2P lines (`mdm:start-end`), one per blocked range.
    pub text: String,
    pub ranges: usize,
    /// Lines that weren't a range MDM could read, and eMule ranges marked allowed (level > 127).
    pub skipped: usize,
}

/// The most a gzipped list may unpack to (the largest public lists are a few tens of MB).
const MAX_UNPACKED: u64 = 256 << 20;

/// `bytes` (a list, gzipped or not) as P2P lines; None when no line is a range.
pub fn convert(bytes: &[u8]) -> Option<Converted> {
    let mut unpacked = Vec::new();
    let raw: &[u8] = if bytes.starts_with(&[0x1f, 0x8b]) {
        flate2::read::GzDecoder::new(bytes)
            .take(MAX_UNPACKED)
            .read_to_end(&mut unpacked)
            .ok()?;
        &unpacked
    } else {
        bytes
    };
    let text = String::from_utf8_lossy(raw);
    let mut out = Converted {
        text: String::new(),
        ranges: 0,
        skipped: 0,
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        match range(line) {
            Some(Some((start, end))) => {
                out.text.push_str(&format!("mdm:{start}-{end}\n"));
                out.ranges += 1;
            }
            // eMule's "allowed": not blocked.
            Some(None) => out.skipped += 1,
            None => out.skipped += 1,
        }
    }
    (out.ranges > 0).then_some(out)
}

/// One line's range: Some(Some) blocked, Some(None) an eMule range marked allowed, None not a
/// range.
fn range(line: &str) -> Option<Option<(IpAddr, IpAddr)>> {
    // eMule: "start - end , level , description" (a level above 127 is allowed), told apart
    // from a P2P line whose name has a comma by the range before the first comma.
    let emule = line.split_once(',').filter(|(span, _)| {
        span.split_once('-')
            .is_some_and(|(a, b)| ip(a).is_some() && ip(b).is_some())
    });
    let span = if let Some((span, rest)) = emule {
        let level: u32 = rest
            .split(',')
            .next()
            .and_then(|l| l.trim().parse().ok())
            .unwrap_or(0);
        if level > 127 {
            return Some(None);
        }
        span
    } else if line.matches(':').count() > 2 {
        // P2P with IPv6 addresses: the name ends at the first colon.
        line.split_once(':')?.1
    } else {
        line.rsplit_once(':')?.1
    };
    let (start, end) = span.split_once('-')?;
    let (start, end) = (ip(start)?, ip(end)?);
    let ordered = match (start, end) {
        (IpAddr::V4(a), IpAddr::V4(b)) => a <= b,
        (IpAddr::V6(a), IpAddr::V6(b)) => a <= b,
        _ => false,
    };
    ordered.then_some(Some((start, end)))
}

/// An address, IPv4's parts possibly zero-padded (`001.002.003.004`).
fn ip(s: &str) -> Option<IpAddr> {
    let s = s.trim();
    if s.contains(':') {
        return s.parse::<Ipv6Addr>().ok().map(IpAddr::V6);
    }
    let parts: Vec<u8> = s
        .split('.')
        .map(|p| p.parse::<u8>().ok())
        .collect::<Option<_>>()?;
    let [a, b, c, d] = parts[..] else {
        return None;
    };
    Some(IpAddr::V4(Ipv4Addr::new(a, b, c, d)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn p2p_lines_are_kept() {
        let c =
            convert(b"# a comment\nSome ISP:1.2.3.0-1.2.3.255\nv6 net:2001:db8::-2001:db8::ffff\n")
                .unwrap();
        assert_eq!(
            c.text,
            "mdm:1.2.3.0-1.2.3.255\nmdm:2001:db8::-2001:db8::ffff\n"
        );
        assert_eq!((c.ranges, c.skipped), (2, 0));
        // A name with a comma is still P2P.
        let c = convert(b"Acme, Inc:4.4.4.0-4.4.4.255\n").unwrap();
        assert_eq!(c.text, "mdm:4.4.4.0-4.4.4.255\n");
    }

    /// eMule's format, zero-padded (librqbit refuses that), and its "allowed" levels.
    #[test]
    fn emule_dat_is_converted() {
        let c = convert(
            b"001.002.003.000 - 001.002.003.255 , 000 , Bad\n\
              010.000.000.000 - 010.255.255.255 , 200 , Allowed\n",
        )
        .unwrap();
        assert_eq!(c.text, "mdm:1.2.3.0-1.2.3.255\n");
        assert_eq!((c.ranges, c.skipped), (1, 1));
    }

    #[test]
    fn gzipped_lists_are_read() {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(b"x:5.6.7.8-5.6.7.9\n").unwrap();
        let c = convert(&gz.finish().unwrap()).unwrap();
        assert_eq!(c.text, "mdm:5.6.7.8-5.6.7.9\n");
    }

    #[test]
    fn what_isnt_a_list_is_none() {
        assert_eq!(convert(b"<html>not found</html>"), None);
        assert_eq!(convert(b""), None);
        // Reversed, mixed families, bad parts: skipped.
        let c = convert(b"a:9.9.9.9-1.1.1.1\nb:1.1.1.1-::1\nc:1.2.3-1.2.4\nd:1.1.1.1-1.1.1.2\n")
            .unwrap();
        assert_eq!((c.ranges, c.skipped), (1, 3));
    }
}
