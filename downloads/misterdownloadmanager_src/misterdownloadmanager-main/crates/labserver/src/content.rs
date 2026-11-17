//! Deterministic file content: byte `i` of a file with seed `s` is a pure function of `(s, i)`,
//! so any range can be generated without storage and verified by any client.

/// Fill `buf` with the content of the file `seed` starting at byte `offset`.
pub fn fill(seed: u64, offset: u64, buf: &mut [u8]) {
    let mut pos = offset;
    let mut i = 0;
    while i < buf.len() {
        let word = splitmix64(seed.wrapping_mul(0xD1B5_4A32_D192_ED03) ^ (pos / 8)).to_le_bytes();
        let start = (pos % 8) as usize;
        let n = (8 - start).min(buf.len() - i);
        buf[i..i + n].copy_from_slice(&word[start..start + n]);
        i += n;
        pos += n as u64;
    }
}

fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Parse a size like `12345`, `64K`, `100M`, `1G` (binary multiples, case-insensitive).
pub fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (digits, mult) = match s.chars().last()?.to_ascii_uppercase() {
        'K' => (&s[..s.len() - 1], 1u64 << 10),
        'M' => (&s[..s.len() - 1], 1 << 20),
        'G' => (&s[..s.len() - 1], 1 << 30),
        _ => (s, 1),
    };
    digits.parse::<u64>().ok()?.checked_mul(mult)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_offset_matches_whole_file() {
        let mut whole = vec![0u8; 1000];
        fill(7, 0, &mut whole);
        for off in [0usize, 1, 7, 8, 9, 333, 999] {
            let mut part = vec![0u8; 1000 - off];
            fill(7, off as u64, &mut part);
            assert_eq!(part, whole[off..]);
        }
    }

    #[test]
    fn seeds_differ() {
        let (mut a, mut b) = ([0u8; 64], [0u8; 64]);
        fill(1, 0, &mut a);
        fill(2, 0, &mut b);
        assert_ne!(a, b);
    }

    #[test]
    fn sizes() {
        assert_eq!(parse_size("12345"), Some(12345));
        assert_eq!(parse_size("64k"), Some(65536));
        assert_eq!(parse_size("100M"), Some(100 << 20));
        assert_eq!(parse_size("1G"), Some(1 << 30));
        assert_eq!(parse_size("x"), None);
        assert_eq!(parse_size(""), None);
    }
}
