//! Where a download's server is (#140): the country of an IP address, from DB-IP's IP to Country
//! Lite database (CC BY 4.0: "IP Geolocation by DB-IP", <https://db-ip.com>), shipped with MDM as a
//! compressed range table (`tools/geo/build.py`). Addresses are looked up here, on this computer;
//! none is ever sent anywhere. And the countries' flags, in one picture (`examples/flag_atlas.rs`,
//! lipis/flag-icons, MIT).
//!
//! The table is unpacked once, on first use (about 3 MB, a few tens of milliseconds): call
//! [`prepare`] early, off the window's thread, and [`country`] answers at once once [`ready`].

use std::io::Read;
use std::net::IpAddr;
use std::sync::OnceLock;

/// The table (`tools/geo/build.py`), xz.
const TABLE: &[u8] = include_bytes!("../data/ipcountry.bin.xz");

/// The flags (`examples/flag_atlas.rs`): `COLUMNS` a row, each `FLAG_W` x `FLAG_H` pixels; cell
/// `i` is country index `i + 1`'s ([`Country::index`]), in `data/countries.txt`'s order, which
/// `tools/geo/build.py` keeps from month to month. `FLAGS` cells are drawn: a country the data
/// gained since has none.
pub const FLAGS_PNG: &[u8] = include_bytes!("../data/flags.png");
pub const FLAGS: u32 = 250;
pub const FLAG_W: u32 = 32;
pub const FLAG_H: u32 = 24;
pub const COLUMNS: u32 = 16;

/// What the data's licence asks to credit, and where it comes from.
pub const ATTRIBUTION: &str = "IP Geolocation by DB-IP";
pub const ATTRIBUTION_URL: &str = "https://db-ip.com";

/// A country of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Country {
    /// Its index in the table (1 and up): its flag's cell is `index - 1`.
    pub index: u8,
    code: [u8; 2],
}

impl Country {
    /// Its ISO 3166-1 alpha-2 code, upper case ("DE").
    pub fn code(&self) -> &str {
        std::str::from_utf8(&self.code).unwrap_or("??")
    }

    /// Its flag's place in [`FLAGS_PNG`], in pixels: (x, y); None for a country newer than the
    /// picture.
    pub fn flag_cell(&self) -> Option<(u32, u32)> {
        let i = u32::from(self.index.checked_sub(1)?);
        (i < FLAGS).then_some(((i % COLUMNS) * FLAG_W, (i / COLUMNS) * FLAG_H))
    }
}

/// One family's ranges: `count` entries in the unpacked table, and every `SPARSE`-th entry's
/// first address, offset and number (the first one's included), for a binary search.
struct Part {
    count: usize,
    sparse: Vec<(u64, usize, usize)>,
}

const SPARSE: usize = 256;

struct Table {
    data: Vec<u8>,
    codes: Vec<[u8; 2]>,
    v4: Part,
    v6: Part,
}

static LOADED: OnceLock<Option<Table>> = OnceLock::new();

fn leb128(data: &[u8], at: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let b = *data.get(*at)?;
        *at += 1;
        v |= u64::from(b & 0x7f).checked_shl(shift)?;
        if b & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
}

/// A part's entries from `at`: the sparse index, and where the part ends.
fn part(data: &[u8], at: &mut usize) -> Option<Part> {
    let count = u32::from_le_bytes(data.get(*at..*at + 4)?.try_into().ok()?) as usize;
    *at += 4;
    let mut sparse = Vec::with_capacity(count / SPARSE + 1);
    let mut first = 0u64;
    for i in 0..count {
        let entry = *at;
        first = first.checked_add(leb128(data, at)?)?;
        *at += 1;
        if i % SPARSE == 0 {
            sparse.push((first, entry, i));
        }
    }
    Some(Part { count, sparse })
}

fn load() -> Option<Table> {
    let mut data = Vec::new();
    lzma_rust2::XzReader::new(TABLE, false)
        .read_to_end(&mut data)
        .ok()?;
    if data.get(..7)? != b"MDMGEO\x01" {
        return None;
    }
    let n = *data.get(7)? as usize;
    let codes = (0..n)
        .map(|i| data.get(8 + 2 * i..10 + 2 * i).map(|c| [c[0], c[1]]))
        .collect::<Option<Vec<_>>>()?;
    let mut at = 8 + 2 * n;
    let v4 = part(&data, &mut at)?;
    let v6 = part(&data, &mut at)?;
    Some(Table {
        data,
        codes,
        v4,
        v6,
    })
}

fn table() -> Option<&'static Table> {
    LOADED.get_or_init(load).as_ref()
}

/// Unpack the table now on a thread of its own, if it isn't yet (it is unpacked once).
pub fn prepare() {
    if LOADED.get().is_none() {
        std::thread::spawn(|| {
            let _ = table();
        });
    }
}

/// Whether [`country`] answers at once.
pub fn ready() -> bool {
    LOADED.get().is_some()
}

/// The country of address `key` in `part`: the last range starting at or before it.
fn find(t: &Table, part: &Part, key: u64) -> u8 {
    if part.count == 0 {
        return 0;
    }
    let s = part.sparse.partition_point(|(first, _, _)| *first <= key);
    let Some(&(mut first, mut at, mut i)) = s.checked_sub(1).and_then(|s| part.sparse.get(s))
    else {
        return 0;
    };
    // The sparse entry's own first address is absolute; walk on from it.
    let mut idx = {
        let mut p = at;
        let _ = leb128(&t.data, &mut p);
        let v = t.data.get(p).copied().unwrap_or(0);
        at = p + 1;
        v
    };
    i += 1;
    while i < part.count {
        let mut p = at;
        let Some(delta) = leb128(&t.data, &mut p) else {
            break;
        };
        let next = first.saturating_add(delta);
        if next > key {
            break;
        }
        first = next;
        idx = t.data.get(p).copied().unwrap_or(0);
        at = p + 1;
        i += 1;
    }
    idx
}

/// The country of `ip` (None: an address of this computer or a local network, a reserved one,
/// or one the table doesn't place). Unpacks the table on first use (see [`prepare`]).
pub fn country(ip: IpAddr) -> Option<Country> {
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    };
    let local = match ip {
        IpAddr::V4(a) => {
            a.is_private()
                || a.is_loopback()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                || a.is_multicast()
        }
        IpAddr::V6(a) => {
            let s = a.segments()[0];
            a.is_loopback()
                || a.is_unspecified()
                || a.is_multicast()
                || (s & 0xfe00) == 0xfc00
                || (s & 0xffc0) == 0xfe80
        }
    };
    if local {
        return None;
    }
    let t = table()?;
    let idx = match ip {
        IpAddr::V4(a) => find(t, &t.v4, u64::from(u32::from(a))),
        IpAddr::V6(a) => find(t, &t.v6, (u128::from(a) >> 64) as u64),
    };
    let code = *t.codes.get(usize::from(idx).checked_sub(1)?)?;
    Some(Country { index: idx, code })
}

/// The table's countries, in its order (index 1 first): their codes.
pub fn countries() -> Vec<String> {
    table().map_or_else(Vec::new, |t| {
        t.codes
            .iter()
            .map(|c| String::from_utf8_lossy(c).into_owned())
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(ip: &str) -> Option<String> {
        country(ip.parse().unwrap()).map(|c| c.code().to_owned())
    }

    #[test]
    fn addresses_have_their_countries() {
        // Well-known, long-held blocks: DB-IP's sample row (8.8.8.0/24 US), APNIC's 1.0.0.0/24
        // (AU) and 1.0.1.0/24 (CN).
        assert_eq!(code("8.8.8.8").as_deref(), Some("US"));
        assert_eq!(code("1.0.0.1").as_deref(), Some("AU"));
        assert_eq!(code("1.0.2.3").as_deref(), Some("CN"));
        // An IPv6 address is placed (which country moves with the monthly data: DB-IP had
        // Google's public DNS in CA in 2026-11).
        assert!(code("2001:4860:4860::8888").is_some());
        // The same address, IPv4-mapped.
        assert_eq!(code("::ffff:8.8.8.8").as_deref(), Some("US"));
    }

    #[test]
    fn this_computer_and_local_networks_have_none() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.1.1",
            "169.254.1.1",
            "::1",
            "fe80::1",
            "fd00::1",
            "0.0.0.0",
        ] {
            assert_eq!(code(ip), None, "{ip}");
        }
    }

    #[test]
    fn the_flags_follow_the_tables_order() {
        // The table's countries start with countries.txt's, in its order: the flags' order.
        let order: Vec<String> = include_str!("../data/countries.txt")
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(order.len() as u32, FLAGS);
        let table = countries();
        assert!(table.len() >= order.len(), "{} countries", table.len());
        assert_eq!(table[..order.len()], order[..]);
        let rows = FLAGS.div_ceil(COLUMNS);
        let last = Country {
            index: FLAGS as u8,
            code: *b"ZW",
        };
        let (x, y) = last.flag_cell().unwrap();
        assert!(x + FLAG_W <= COLUMNS * FLAG_W && y + FLAG_H <= rows * FLAG_H);
        // One newer than the picture has no flag.
        let newer = Country {
            index: FLAGS as u8 + 1,
            code: *b"XX",
        };
        assert_eq!(newer.flag_cell(), None);
        // The picture is a PNG of that size.
        assert_eq!(&FLAGS_PNG[1..4], b"PNG");
        let w = u32::from_be_bytes(FLAGS_PNG[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(FLAGS_PNG[20..24].try_into().unwrap());
        assert_eq!((w, h), (COLUMNS * FLAG_W, rows * FLAG_H));
    }

    #[test]
    fn the_whole_space_is_walked_in_order() {
        // Every range's lookup at its own first address gives its own country (the sparse index
        // and the walk agree).
        let t = table().unwrap();
        for part in [&t.v4, &t.v6] {
            let Some(&(_, mut at, _)) = part.sparse.first() else {
                continue;
            };
            let mut first = 0u64;
            for i in 0..part.count.min(5000) {
                first += leb128(&t.data, &mut at).unwrap();
                let idx = t.data[at];
                at += 1;
                assert_eq!(find(t, part, first), idx, "entry {i}");
            }
        }
    }
}
