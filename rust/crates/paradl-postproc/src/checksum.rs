//! Cryptographic checksum computation, GNU/BSD text parsing, and companion discovery.
//!
//! Direct port of `src/paradl/postproc/checksum.py`. Digests are computed with a
//! 1 MiB streaming buffer (the Python mmap fast-path is unnecessary in Rust since
//! the OS page cache already avoids redundant copies).
//!
//! ```
//! use paradl_postproc::checksum::{parse_checksum_text, Algorithm};
//! let text = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad  a.bin\n";
//! assert_eq!(parse_checksum_text(text, None).unwrap().0, Algorithm::Sha256);
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufReader, Read};
use std::path::Path;

use blake2::Blake2b512;
use blake2::Digest as _;
use md5::Md5;
use paradl_core::Error;
use regex::Regex;
use sha1::Sha1;
use sha2::{Sha256, Sha512};

use crate::{ChecksumSource, Result, Verification};

/// Algorithms supported by the Python oracle (`checksum.py:19`).
pub const SUPPORTED_ALGOS: &[&str] = &["md5", "sha1", "sha256", "sha512", "blake2b"];

/// Companion checksum suffixes, in discovery order (`checksum.py:167-172`).
const DIRECT_SUFFIXES: &[&str] = &[".sha256", ".sha256sum", ".sha512", ".md5"];
/// Multi-file checksum manifests, in discovery order (`checksum.py:185-190`).
const MULTI_NAMES: &[&str] = &[
    "sha256sums.txt",
    "SHA256SUMS",
    "checksums.txt",
    "SHASUMS256.txt",
];

/// Supported digest algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Algorithm {
    /// MD5 (32 hex chars).
    Md5,
    /// SHA-1 (40 hex chars).
    Sha1,
    /// SHA-256 (64 hex chars).
    Sha256,
    /// SHA-512 (128 hex chars).
    Sha512,
    /// BLAKE2b (64 hex chars).
    Blake2b,
}

impl Algorithm {
    /// Parse an algorithm name, normalising `SHA-256` → `Sha256`.
    ///
    /// ```
    /// use paradl_postproc::checksum::Algorithm;
    /// assert_eq!(Algorithm::parse("SHA-256"), Some(Algorithm::Sha256));
    /// assert_eq!(Algorithm::parse("blake2b"), Some(Algorithm::Blake2b));
    /// ```
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().replace('-', "").as_str() {
            "md5" => Some(Self::Md5),
            "sha1" => Some(Self::Sha1),
            "sha256" => Some(Self::Sha256),
            "sha512" => Some(Self::Sha512),
            "blake2b" => Some(Self::Blake2b),
            _ => None,
        }
    }

    /// Canonical lowercase name used as a key in [`compute_hashes`] output.
    pub fn name(self) -> &'static str {
        match self {
            Self::Md5 => "md5",
            Self::Sha1 => "sha1",
            Self::Sha256 => "sha256",
            Self::Sha512 => "sha512",
            Self::Blake2b => "blake2b",
        }
    }

    /// Expected hex digest length.
    pub fn hex_len(self) -> usize {
        match self {
            Self::Md5 => 32,
            Self::Sha1 => 40,
            Self::Sha256 | Self::Blake2b => 64,
            Self::Sha512 => 128,
        }
    }

    fn from_hex_len(length: usize) -> Option<Self> {
        match length {
            32 => Some(Self::Md5),
            40 => Some(Self::Sha1),
            64 => Some(Self::Sha256),
            128 => Some(Self::Sha512),
            _ => None,
        }
    }
}

/// A parsed `[algo:]<hex>` digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest {
    /// Algorithm to compute.
    pub algorithm: Algorithm,
    /// Expected hex digest (case-insensitive comparison).
    pub hex: String,
}

impl Digest {
    /// Parse a checksum specification, inferring the algorithm from hex length
    /// when no explicit `algo:` prefix is present (`checksum.py:74-103`).
    pub fn parse(spec: &str) -> Result<Self> {
        let trimmed = spec.trim();
        let (algorithm, hex) = if let Some((prefix, value)) = trimmed.split_once(':') {
            let algorithm = Algorithm::parse(prefix).ok_or_else(|| {
                Error::verification_failed(format!("Unsupported checksum algorithm: {prefix:?}"))
            })?;
            (algorithm, value.trim().to_string())
        } else {
            (
                Algorithm::from_hex_len(trimmed.len()).unwrap_or(Algorithm::Sha256),
                trimmed.to_string(),
            )
        };
        Ok(Self { algorithm, hex })
    }
}

/// Compute one or more digests over `path` using a 1 MiB streaming buffer.
///
/// Returned map keys are canonical names from [`Algorithm::name`].
pub fn compute_hashes(path: &Path, algos: &[Algorithm]) -> Result<BTreeMap<String, String>> {
    let mut md5 = algos.contains(&Algorithm::Md5).then(Md5::new);
    let mut sha1 = algos.contains(&Algorithm::Sha1).then(Sha1::new);
    let mut sha256 = algos.contains(&Algorithm::Sha256).then(Sha256::new);
    let mut sha512 = algos.contains(&Algorithm::Sha512).then(Sha512::new);
    let mut blake2b = algos.contains(&Algorithm::Blake2b).then(Blake2b512::new);

    let mut reader = BufReader::with_capacity(1024 * 1024, fs::File::open(path)?);
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        if let Some(hasher) = &mut md5 {
            hasher.update(chunk);
        }
        if let Some(hasher) = &mut sha1 {
            hasher.update(chunk);
        }
        if let Some(hasher) = &mut sha256 {
            hasher.update(chunk);
        }
        if let Some(hasher) = &mut sha512 {
            hasher.update(chunk);
        }
        if let Some(hasher) = &mut blake2b {
            hasher.update(chunk);
        }
    }

    let mut results = BTreeMap::new();
    if let Some(hasher) = md5 {
        results.insert("md5".to_string(), hex::encode(hasher.finalize()));
    }
    if let Some(hasher) = sha1 {
        results.insert("sha1".to_string(), hex::encode(hasher.finalize()));
    }
    if let Some(hasher) = sha256 {
        results.insert("sha256".to_string(), hex::encode(hasher.finalize()));
    }
    if let Some(hasher) = sha512 {
        results.insert("sha512".to_string(), hex::encode(hasher.finalize()));
    }
    if let Some(hasher) = blake2b {
        results.insert("blake2b".to_string(), hex::encode(hasher.finalize()));
    }
    Ok(results)
}

/// Verify `path` against `expected`, or against a discovered companion file.
///
/// Returns a [`Verification`] data structure; callers wanting process semantics
/// should use [`crate::PostProcessor::verify`], which converts `verified == false`
/// into [`Error::VerificationFailed`] (exit code `3`).
pub fn verify_file(path: &Path, expected: Option<&Digest>) -> Result<Verification> {
    let (algorithm, expected_hex, source) = if let Some(digest) = expected {
        (
            digest.algorithm,
            Some(digest.hex.clone()),
            ChecksumSource::Explicit,
        )
    } else if let Some((algorithm, hex)) = find_companion_checksum(path) {
        (algorithm, Some(hex), ChecksumSource::Companion)
    } else {
        return Ok(Verification {
            verified: false,
            algorithm: Algorithm::Sha256,
            expected: None,
            computed: String::new(),
            source: ChecksumSource::None,
        });
    };

    let computed = compute_hashes(path, &[algorithm])?
        .remove(algorithm.name())
        .unwrap_or_default();
    let verified = expected_hex
        .as_ref()
        .is_some_and(|expected| expected.trim().eq_ignore_ascii_case(&computed));
    Ok(Verification {
        verified,
        algorithm,
        expected: expected_hex,
        computed,
        source,
    })
}

fn is_hex_of_length(value: &str, min: usize, max: usize) -> bool {
    let length = value.len();
    length >= min && length <= max && value.bytes().all(|b| b.is_ascii_hexdigit())
}

fn file_name_matches(candidate: &str, target: Option<&str>) -> bool {
    match target {
        None => true,
        Some(target) => Path::new(candidate)
            .file_name()
            .is_some_and(|name| name == target),
    }
}

/// Parse GNU (`<hex>  file`) or BSD (`ALGO (file) = <hex>`) checksum text.
///
/// Mirrors `ChecksumEngine.parse_checksum_text` (`checksum.py:105-160`), including
/// comment/blank skipping, target-filename matching, the `*` binary marker, and
/// the single bare-hash fallback.
pub fn parse_checksum_text(
    text: &str,
    target_filename: Option<&str>,
) -> Option<(Algorithm, String)> {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    if lines.is_empty() {
        return None;
    }

    let bsd = Regex::new(
        r"(?i)^(MD5|SHA1|SHA256|SHA-256|SHA512|SHA-512|BLAKE2b)\s*\(([^)]+)\)\s*=\s*([0-9a-fA-F]+)$",
    )
    .expect("static BSD checksum regex");

    // Pass 1: match the target filename when one was supplied.
    for line in &lines {
        if let Some(captures) = bsd.captures(line) {
            let algorithm = Algorithm::parse(captures.get(1)?.as_str())?;
            let filename = captures.get(2)?.as_str();
            let digest = captures.get(3)?.as_str().to_ascii_lowercase();
            if file_name_matches(filename, target_filename) {
                return Some((algorithm, digest));
            }
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 && is_hex_of_length(parts[0], 32, 128) {
            let digest = parts[0].to_ascii_lowercase();
            let filename = parts[1].trim_start_matches('*');
            if let Some(algorithm) = Algorithm::from_hex_len(digest.len())
                && file_name_matches(filename, target_filename)
            {
                return Some((algorithm, digest));
            }
        }
    }

    // Pass 2: a single bare hash in a dedicated `<filename>.<algo>` file.
    if lines.len() == 1 {
        let parts: Vec<&str> = lines[0].split_whitespace().collect();
        if let Some(first) = parts.first()
            && is_hex_of_length(first, 32, 128)
        {
            let name_ok = parts.len() == 1
                || target_filename.is_none()
                || file_name_matches(parts[1].trim_start_matches('*'), target_filename);
            if name_ok && let Some(algorithm) = Algorithm::from_hex_len(first.len()) {
                return Some((algorithm, first.to_ascii_lowercase()));
            }
        }
    }
    None
}

/// Discover a companion checksum for `path` (`checksum.py:162-201`).
pub fn find_companion_checksum(path: &Path) -> Option<(Algorithm, String)> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path.file_name()?.to_string_lossy();
    let name_ref = name.as_ref();

    for suffix in DIRECT_SUFFIXES {
        let candidate = parent.join(format!("{name_ref}{suffix}"));
        if candidate.is_file() {
            let text = fs::read(&candidate)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            if let Some(parsed) = parse_checksum_text(&text, Some(name_ref))
                .or_else(|| parse_checksum_text(&text, None))
            {
                return Some(parsed);
            }
        }
    }

    for manifest in MULTI_NAMES {
        let candidate = parent.join(manifest);
        if candidate.is_file() {
            let text = fs::read(&candidate)
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                .unwrap_or_default();
            if let Some(parsed) = parse_checksum_text(&text, Some(name_ref)) {
                return Some(parsed);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_hex_len_matches_oracle() {
        assert_eq!(Algorithm::from_hex_len(32), Some(Algorithm::Md5));
        assert_eq!(Algorithm::from_hex_len(40), Some(Algorithm::Sha1));
        assert_eq!(Algorithm::from_hex_len(64), Some(Algorithm::Sha256));
        assert_eq!(Algorithm::from_hex_len(128), Some(Algorithm::Sha512));
        assert_eq!(Algorithm::from_hex_len(48), None);
    }
}
