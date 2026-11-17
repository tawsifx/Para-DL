//! Just enough OpenPGP (RFC 9580, and RFC 4880 before it) to check a detached signature by one
//! of the keys MDM pins: yt-dlp's (RSA) and MDM's own release key (Ed25519, in its legacy
//! EdDSA form, algorithm 22). Both sign their checksum lists as binary documents with v4
//! signatures; that, with SHA-256 or SHA-512, is all this understands, and anything else fails.
//! The keys are built into the program, so their self-signatures aren't checked: every key and
//! subkey in a pinned key is trusted to sign. The cryptography is aws-lc-rs's, the library
//! rustls already brings.
//!
//! The input is untrusted: every read is bounds-checked, and a malformed byte only makes a
//! check fail.

use aws_lc_rs::digest::{self, Digest};
use aws_lc_rs::signature::{self, RsaPublicKeyComponents, UnparsedPublicKey};
use base64::Engine;

// Packet tags (RFC 9580 §5).
const TAG_SIGNATURE: u8 = 2;
const TAG_PUBLIC_KEY: u8 = 6;
const TAG_PUBLIC_SUBKEY: u8 = 14;

// Public-key algorithms (§9.1).
const RSA: u8 = 1;
const EDDSA_LEGACY: u8 = 22;

// Hash algorithms (§9.5).
const SHA256: u8 = 8;
const SHA512: u8 = 10;

// Signature subpackets (§5.2.3.7).
const SUB_CREATED: u8 = 2;
const SUB_EXPIRES: u8 = 3;
const SUB_ISSUER: u8 = 16;
const SUB_ISSUER_FINGERPRINT: u8 = 33;

/// Ed25519's curve OID as legacy EdDSA keys carry it (1.3.6.1.4.1.11591.15.1, §9.2).
const ED25519_OID: &[u8] = &[0x2b, 0x06, 0x01, 0x04, 0x01, 0xda, 0x47, 0x0f, 0x01];

/// Whether `sig` (one signature packet, binary or ASCII-armored) is a valid signature of `data`
/// by the public key `key` (binary or ASCII-armored) or one of its subkeys.
pub fn verify(key: &[u8], data: &[u8], sig: &[u8]) -> bool {
    let (Some(key), Some(sig)) = (unarmor(key, "PUBLIC KEY BLOCK"), unarmor(sig, "SIGNATURE"))
    else {
        return false;
    };
    let (Some(keys), Some(sig)) = (public_keys(&key), Signature::parse(&sig)) else {
        return false;
    };
    keys.iter().any(|key| sig.verifies(key, data))
}

/// The packets of `armored` if it's an ASCII-armored block of `kind` ("SIGNATURE", ...), or
/// `armored` itself if it's binary.
fn unarmor(armored: &[u8], kind: &str) -> Option<Vec<u8>> {
    // A binary packet starts with a byte whose top bit is set, which no armor does.
    if armored.first().is_some_and(|b| b & 0x80 != 0) {
        return Some(armored.to_vec());
    }
    let text = std::str::from_utf8(armored).ok()?;
    let mut lines = text.lines().map(str::trim_end);
    let begin = format!("-----BEGIN PGP {kind}-----");
    let end = format!("-----END PGP {kind}-----");
    lines.find(|line| *line == begin)?;
    // Headers ("Version: ..."), up to the blank line before the data.
    for line in lines.by_ref() {
        if line.is_empty() {
            break;
        }
        if !line.contains(": ") {
            return None;
        }
    }
    let mut body = String::new();
    let mut crc = None;
    loop {
        let line = lines.next()?;
        if line == end {
            break;
        }
        if let Some(sum) = line.strip_prefix('=') {
            // The optional checksum comes last, just before the end line.
            let sum = base64::engine::general_purpose::STANDARD.decode(sum).ok()?;
            let [a, b, c] = sum[..] else {
                return None;
            };
            crc = Some(u32::from_be_bytes([0, a, b, c]));
            if lines.next()? != end {
                return None;
            }
            break;
        }
        body.push_str(line);
    }
    let packets = base64::engine::general_purpose::STANDARD
        .decode(body)
        .ok()?;
    if crc.is_some_and(|crc| crc != crc24(&packets)) {
        return None;
    }
    Some(packets)
}

/// The armor's checksum (RFC 9580 §6.1.1).
fn crc24(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xb7_04ce;
    for &byte in bytes {
        crc ^= u32::from(byte) << 16;
        for _ in 0..8 {
            crc <<= 1;
            if crc & 0x100_0000 != 0 {
                crc ^= 0x186_4cfb;
            }
        }
    }
    crc & 0xff_ffff
}

/// Reads through a packet or subpacket, failing at its end instead of panicking.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if n > self.0.len() {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn u16(&mut self) -> Option<u16> {
        let b = self.take(2)?;
        Some(u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Option<u32> {
        let b = self.take(4)?;
        Some(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// A multiprecision integer (§3.2): big-endian, no leading zeros, its length in bits
    /// counted from its first set bit. Any other length fails.
    fn mpi(&mut self) -> Option<&'a [u8]> {
        let bits = usize::from(self.u16()?);
        let bytes = self.take(bits.div_ceil(8))?;
        let top = bytes.first().map_or(0, |&b| 8 - b.leading_zeros() as usize);
        (bits == (bytes.len().saturating_sub(1)) * 8 + top).then_some(bytes)
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// `bytes`, split into its packets as (tag, body); none if any packet is malformed.
fn packets(bytes: &[u8]) -> Option<Vec<(u8, &[u8])>> {
    let mut r = Reader(bytes);
    let mut packets = Vec::new();
    while !r.is_empty() {
        let first = r.u8()?;
        if first & 0x80 == 0 {
            return None;
        }
        let (tag, len) = if first & 0x40 != 0 {
            // The current format (§4.2.1). Partial lengths are only for data packets.
            let len = match r.u8()? {
                n @ 0..192 => usize::from(n),
                n @ 192..224 => (usize::from(n - 192) << 8) + usize::from(r.u8()?) + 192,
                255 => usize::try_from(r.u32()?).ok()?,
                _ => return None,
            };
            (first & 0x3f, len)
        } else {
            // The legacy format (§4.2.2), which gpg still writes; never of indeterminate length.
            let len = match first & 0x03 {
                0 => usize::from(r.u8()?),
                1 => usize::from(r.u16()?),
                2 => usize::try_from(r.u32()?).ok()?,
                _ => return None,
            };
            ((first >> 2) & 0x0f, len)
        };
        packets.push((tag, r.take(len)?));
    }
    Some(packets)
}

/// A v4 public key or subkey this can check signatures with.
struct PublicKey {
    /// SHA-1 over the key packet (§5.5.4.2); its last 8 bytes are the key ID.
    fingerprint: [u8; 20],
    material: KeyMaterial,
}

enum KeyMaterial {
    /// The modulus and the public exponent, big-endian, no leading zeros.
    Rsa {
        n: Vec<u8>,
        e: Vec<u8>,
    },
    Ed25519([u8; 32]),
}

impl PublicKey {
    fn algorithm(&self) -> u8 {
        match self.material {
            KeyMaterial::Rsa { .. } => RSA,
            KeyMaterial::Ed25519(_) => EDDSA_LEGACY,
        }
    }

    fn key_id(&self) -> &[u8] {
        &self.fingerprint[12..]
    }

    /// The key in a public key or subkey packet's `body`; none if it's of another version or
    /// algorithm.
    fn parse(body: &[u8]) -> Option<Self> {
        let mut r = Reader(body);
        if r.u8()? != 4 {
            return None;
        }
        let _created = r.u32()?;
        let material = match r.u8()? {
            RSA => {
                let n = r.mpi()?.to_vec();
                let e = r.mpi()?.to_vec();
                KeyMaterial::Rsa { n, e }
            }
            EDDSA_LEGACY => {
                let oid_len = usize::from(r.u8()?);
                if r.take(oid_len)? != ED25519_OID {
                    return None;
                }
                // The point, prefixed with 0x40 (native encoding, §11.2.2).
                let point = r.mpi()?;
                let [0x40, point @ ..] = point else {
                    return None;
                };
                KeyMaterial::Ed25519(point.try_into().ok()?)
            }
            _ => return None,
        };
        if !r.is_empty() {
            return None;
        }
        let len = u16::try_from(body.len()).ok()?.to_be_bytes();
        let mut sha1 = digest::Context::new(&digest::SHA1_FOR_LEGACY_USE_ONLY);
        sha1.update(&[0x99, len[0], len[1]]);
        sha1.update(body);
        let fingerprint = sha1.finish().as_ref().try_into().ok()?;
        Some(Self {
            fingerprint,
            material,
        })
    }
}

/// The usable keys in a transferable public key (§10.1): the primary key and its subkeys. The
/// other packets (user IDs, self-signatures) are skipped.
fn public_keys(bytes: &[u8]) -> Option<Vec<PublicKey>> {
    let packets = packets(bytes)?;
    if packets.first()?.0 != TAG_PUBLIC_KEY {
        return None;
    }
    Some(
        packets
            .iter()
            .filter(|(tag, _)| matches!(*tag, TAG_PUBLIC_KEY | TAG_PUBLIC_SUBKEY))
            .filter_map(|(_, body)| PublicKey::parse(body))
            .collect(),
    )
}

/// A v4 signature of a binary document (type 0x00).
struct Signature<'a> {
    algorithm: u8,
    hash: u8,
    /// The signature's hashed part: version, type, algorithms and hashed subpackets.
    hashed: &'a [u8],
    left16: [u8; 2],
    /// Seconds since 1970 when made, and for how long it holds (0: forever).
    created: u32,
    expires: u32,
    issuer: Option<[u8; 8]>,
    issuer_fingerprint: Option<[u8; 20]>,
    /// RSA: the signature; Ed25519: R and S.
    mpis: Vec<&'a [u8]>,
}

impl<'a> Signature<'a> {
    /// The only packet in `bytes`, a signature this understands.
    fn parse(bytes: &'a [u8]) -> Option<Self> {
        let [(TAG_SIGNATURE, body)] = packets(bytes)?[..] else {
            return None;
        };
        let mut r = Reader(body);
        let [4, 0x00, algorithm, hash] = *r.take(4)? else {
            return None;
        };
        let hashed_len = usize::from(r.u16()?);
        let hashed_area = r.take(hashed_len)?;
        let unhashed_len = usize::from(r.u16()?);
        let unhashed_area = r.take(unhashed_len)?;
        let left16 = r.take(2)?.try_into().ok()?;
        let mut sig = Signature {
            algorithm,
            hash,
            hashed: body.get(..6 + hashed_len)?,
            left16,
            created: 0,
            expires: 0,
            issuer: None,
            issuer_fingerprint: None,
            mpis: Vec::new(),
        };
        let mut created = None;
        sig.subpackets(hashed_area, true, &mut created)?;
        sig.subpackets(unhashed_area, false, &mut created)?;
        // The creation time must be there, and signed (§5.2.3.11).
        sig.created = created?;
        let count = match algorithm {
            RSA => 1,
            EDDSA_LEGACY => 2,
            _ => return None,
        };
        for _ in 0..count {
            sig.mpis.push(r.mpi()?);
        }
        if !r.is_empty() {
            return None;
        }
        Some(sig)
    }

    /// Reads the subpackets in `area` (§5.2.3.7). A critical one this doesn't know, or one
    /// that is malformed, fails the signature; times only count from the hashed area.
    fn subpackets(&mut self, area: &[u8], hashed: bool, created: &mut Option<u32>) -> Option<()> {
        let mut r = Reader(area);
        while !r.is_empty() {
            let len = match r.u8()? {
                n @ 0..192 => usize::from(n),
                n @ 192..255 => (usize::from(n - 192) << 8) + usize::from(r.u8()?) + 192,
                255 => usize::try_from(r.u32()?).ok()?,
            };
            let mut sub = Reader(r.take(len)?);
            let kind = sub.u8()?;
            let critical = kind & 0x80 != 0;
            match kind & 0x7f {
                SUB_CREATED if hashed => {
                    if created.replace(sub.u32()?).is_some() {
                        return None;
                    }
                }
                SUB_EXPIRES if hashed => self.expires = sub.u32()?,
                SUB_ISSUER => self.issuer = Some(sub.take(8)?.try_into().ok()?),
                SUB_ISSUER_FINGERPRINT => {
                    // A v4 key's fingerprint; any other version can't be one of ours.
                    if sub.u8()? != 4 {
                        return None;
                    }
                    self.issuer_fingerprint = Some(sub.take(20)?.try_into().ok()?);
                }
                _ if critical => return None,
                _ => continue,
            }
            if !sub.is_empty() {
                return None;
            }
        }
        Some(())
    }

    /// Whether this signs `data` with `key`.
    fn verifies(&self, key: &PublicKey, data: &[u8]) -> bool {
        if self.algorithm != key.algorithm()
            || self.issuer.is_some_and(|id| id != key.key_id())
            || self
                .issuer_fingerprint
                .is_some_and(|fp| fp != key.fingerprint)
            || self.expired()
        {
            return false;
        }
        let Some(digest) = self.digest(data) else {
            return false;
        };
        // A quick check first (§5.2.4): the hash's first two bytes are in the packet.
        if digest.as_ref().get(..2) != Some(&self.left16[..]) {
            return false;
        }
        match (&key.material, &self.mpis[..]) {
            (KeyMaterial::Rsa { n, e }, [sig]) => {
                let params = match self.hash {
                    SHA256 => &signature::RSA_PKCS1_2048_8192_SHA256,
                    SHA512 => &signature::RSA_PKCS1_2048_8192_SHA512,
                    _ => return false,
                };
                // As long as the modulus (the MPI dropped the leading zeros).
                let Some(sig) = left_pad(sig, n.len()) else {
                    return false;
                };
                RsaPublicKeyComponents { n, e }
                    .to_parsed_public_key(params)
                    .is_ok_and(|key| key.verify_digest_sig(&digest, &sig).is_ok())
            }
            (KeyMaterial::Ed25519(point), [r, s]) => {
                // Legacy EdDSA signs the hash; R and S are 32 bytes each.
                let (Some(mut rs), Some(s)) = (left_pad(r, 32), left_pad(s, 32)) else {
                    return false;
                };
                rs.extend_from_slice(&s);
                UnparsedPublicKey::new(&signature::ED25519, point)
                    .verify(digest.as_ref(), &rs)
                    .is_ok()
            }
            _ => false,
        }
    }

    /// The hash the signature is over: the data, the signature's hashed part and the v4 trailer
    /// (§5.2.4).
    fn digest(&self, data: &[u8]) -> Option<Digest> {
        let algorithm = match self.hash {
            SHA256 => &digest::SHA256,
            SHA512 => &digest::SHA512,
            _ => return None,
        };
        let mut hash = digest::Context::new(algorithm);
        hash.update(data);
        hash.update(self.hashed);
        hash.update(&[4, 0xff]);
        hash.update(&u32::try_from(self.hashed.len()).ok()?.to_be_bytes());
        Some(hash.finish())
    }

    /// Whether the signature has run out. Most never do (no expiry); one made "in the future"
    /// by a clock ahead of this one still counts.
    fn expired(&self) -> bool {
        if self.expires == 0 {
            return false;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        now >= u64::from(self.created) + u64::from(self.expires)
    }
}

/// `bytes` with zeros in front up to `len` bytes; none if it's longer.
fn left_pad(bytes: &[u8], len: usize) -> Option<Vec<u8>> {
    let mut padded = vec![0; len.checked_sub(bytes.len())?];
    padded.extend_from_slice(bytes);
    Some(padded)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MDM_KEY: &[u8] = include_bytes!("../keys/mdm-release.asc");
    const YTDLP_KEY: &[u8] = include_bytes!("../keys/yt-dlp.asc");
    const MDM_SUMS: &[u8] = include_bytes!("../tests/fixtures/mdm-SHA256SUMS.txt");
    const MDM_SIG: &[u8] = include_bytes!("../tests/fixtures/mdm-SHA256SUMS.txt.sig");
    const YTDLP_SUMS: &[u8] = include_bytes!("../tests/fixtures/SHA2-256SUMS");
    const YTDLP_SIG: &[u8] = include_bytes!("../tests/fixtures/SHA2-256SUMS.sig");
    const BOTH: [(&[u8], &[u8], &[u8]); 2] = [
        (MDM_KEY, MDM_SUMS, MDM_SIG),
        (YTDLP_KEY, YTDLP_SUMS, YTDLP_SIG),
    ];

    /// `sig` in ASCII armor with the checksum `crc`.
    fn armor(sig: &[u8], crc: u32) -> String {
        let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
        let body = b64(sig);
        let lines: Vec<&str> = (0..body.len())
            .step_by(64)
            .map(|at| &body[at..(at + 64).min(body.len())])
            .collect();
        format!(
            "-----BEGIN PGP SIGNATURE-----\r\n\r\n{}\r\n={}\r\n-----END PGP SIGNATURE-----\r\n",
            lines.join("\r\n"),
            b64(&crc.to_be_bytes()[1..])
        )
    }

    /// Where the unhashed subpackets of the signature packet `sig` are: the one part no
    /// signature covers.
    fn unhashed(sig: &[u8]) -> std::ops::Range<usize> {
        let header = 1 + [1, 2, 4][usize::from(sig[0] & 0x03)];
        let hashed = usize::from(u16::from_be_bytes([sig[header + 4], sig[header + 5]]));
        let at = header + 6 + hashed;
        let len = usize::from(u16::from_be_bytes([sig[at], sig[at + 1]]));
        at + 2..at + 2 + len
    }

    #[test]
    fn the_pinned_keys_have_the_fingerprints_we_expect() {
        let fingerprint = |armored| {
            let keys = public_keys(&unarmor(armored, "PUBLIC KEY BLOCK").unwrap()).unwrap();
            assert_eq!(keys.len(), 1);
            hex::encode_upper(keys[0].fingerprint)
        };
        assert_eq!(
            fingerprint(MDM_KEY),
            "5826D399249DE856EA087D802E293C28E8A69D74"
        );
        assert_eq!(
            fingerprint(YTDLP_KEY),
            "AC0CBBE6848D6A873464AF4E57CF65933B5A7581"
        );
    }

    #[test]
    fn real_signatures_verify_and_any_change_fails_them() {
        for (key, sums, sig) in BOTH {
            assert!(verify(key, sums, sig));
            for at in [0, sums.len() / 2, sums.len() - 1] {
                let mut data = sums.to_vec();
                data[at] ^= 1;
                assert!(!verify(key, &data, sig), "data byte {at} changed");
            }
            // Every byte of the signature counts but the unhashed subpackets (the issuer, which
            // only names the key).
            let unhashed = unhashed(sig);
            for at in (0..sig.len()).filter(|at| !unhashed.contains(at)) {
                for flip in [0x01, 0x10, 0x80] {
                    let mut forged = sig.to_vec();
                    forged[at] ^= flip;
                    assert!(!verify(key, sums, &forged), "signature byte {at} changed");
                }
            }
            for len in 0..sig.len() {
                assert!(!verify(key, sums, &sig[..len]), "cut to {len} bytes");
            }
            assert!(!verify(key, sums, &[sig, &[0]].concat()));
            assert!(!verify(key, sums, &[sig, sig].concat()), "two signatures");
        }
    }

    #[test]
    fn an_issuer_that_names_another_key_fails() {
        for (key, sums, sig) in BOTH {
            let mut forged = sig.to_vec();
            forged[unhashed(sig).end - 1] ^= 1;
            assert!(!verify(key, sums, &forged));
        }
    }

    #[test]
    fn each_key_signs_for_itself_only() {
        assert!(!verify(YTDLP_KEY, MDM_SUMS, MDM_SIG));
        assert!(!verify(MDM_KEY, YTDLP_SUMS, YTDLP_SIG));
        assert!(!verify(MDM_KEY, MDM_SUMS, MDM_KEY));
        assert!(!verify(MDM_SIG, MDM_SUMS, MDM_SIG));
    }

    #[test]
    fn armor_is_checked_by_its_checksum() {
        let armored = armor(MDM_SIG, crc24(MDM_SIG));
        assert!(verify(MDM_KEY, MDM_SUMS, armored.as_bytes()));
        let wrong = armor(MDM_SIG, crc24(MDM_SIG) ^ 1);
        assert!(!verify(MDM_KEY, MDM_SUMS, wrong.as_bytes()));
        // A damaged key: "mDME" begins its data.
        let key = String::from_utf8(MDM_KEY.to_vec())
            .unwrap()
            .replacen("mDME", "mDMF", 1);
        assert!(!verify(key.as_bytes(), MDM_SUMS, MDM_SIG));
    }

    #[test]
    fn garbage_fails_without_panicking() {
        // xorshift64: the same "random" bytes on every run.
        let mut state = 0x2545_f491_4f6c_dd1d_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for round in 0..20_000 {
            let len = (next() % 600) as usize;
            let mut bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            // Half begin like the real signature, to get past the first checks.
            if round % 2 == 0 && len >= 6 {
                bytes[..6].copy_from_slice(&YTDLP_SIG[..6]);
            }
            assert!(!verify(YTDLP_KEY, YTDLP_SUMS, &bytes));
            assert!(!verify(&bytes, YTDLP_SUMS, YTDLP_SIG));
            let text: String = bytes.iter().map(|&b| char::from(b'+' + b % 80)).collect();
            let armored = format!("-----BEGIN PGP SIGNATURE-----\n\n{text}\n=");
            assert!(!verify(MDM_KEY, MDM_SUMS, armored.as_bytes()));
        }
        // The real keys and signatures, each byte damaged in turn, and cut short.
        for (key, sums, sig) in BOTH {
            let key = unarmor(key, "PUBLIC KEY BLOCK").unwrap();
            for at in 0..key.len() {
                let mut bad = key.clone();
                bad[at] = next() as u8;
                let _ = verify(&bad, sums, sig);
                let _ = verify(&key[..at], sums, sig);
            }
            for at in 0..sig.len() {
                let mut bad = sig.to_vec();
                bad[at] = 0xff;
                let _ = verify(&key, sums, &bad);
            }
        }
    }
}
