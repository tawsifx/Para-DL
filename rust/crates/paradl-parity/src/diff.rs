//! Byte-for-byte diff primitives shared by all three lanes.
//!
//! Every observable channel (stdout, stderr, JSONL, files, sidecars, exit code)
//! is reduced to a labelled pair of byte strings. A lane is clean only when
//! every pair is identical; the harness exits nonzero otherwise.
#![warn(missing_docs)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io;
use std::path::Path;

use serde_json::Value;

/// One observable difference between the Python oracle and the Rust side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Difference {
    /// Channel that differed, e.g. `stdout` or `file:artifact.m4a`.
    pub field: String,
    /// Oracle (expected) value, truncated for a concise report.
    pub expected: String,
    /// Rust (actual) value, truncated for a concise report.
    pub actual: String,
}

/// The concise report printed by every lane.
#[derive(Debug, Clone)]
pub struct DiffReport {
    /// Lane name (`non-media`, `media`, or `boundary`).
    pub lane: &'static str,
    /// Case name within the lane.
    pub case: String,
    /// All detected differences; empty means the case passed.
    pub differences: Vec<Difference>,
    /// Honesty notes (drift, deferrals, cold/warm caveats).
    pub notes: Vec<String>,
}

impl DiffReport {
    /// Create an empty report for one lane case.
    ///
    /// ```
    /// use paradl_parity::DiffReport;
    /// let report = DiffReport::new("boundary", "ok");
    /// assert!(report.is_clean());
    /// ```
    pub fn new(lane: &'static str, case: impl Into<String>) -> Self {
        Self {
            lane,
            case: case.into(),
            differences: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Whether the case has zero differences.
    pub fn is_clean(&self) -> bool {
        self.differences.is_empty()
    }

    /// Record a difference (or do nothing when the values are equal).
    pub fn compare(&mut self, field: impl Into<String>, expected: &[u8], actual: &[u8]) {
        if expected == actual {
            return;
        }
        self.differences.push(Difference {
            field: field.into(),
            expected: preview(expected),
            actual: preview(actual),
        });
    }

    /// Add an honesty note to the report.
    pub fn note(&mut self, note: impl Into<String>) {
        self.notes.push(note.into());
    }

    /// Render a concise human-readable report.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "[{}/{}] {} ({} difference(s))",
            self.lane,
            self.case,
            if self.is_clean() { "CLEAN" } else { "DIFF" },
            self.differences.len(),
        );
        for difference in &self.differences {
            let _ = writeln!(
                out,
                "  - {}: expected={:?} actual={:?}",
                difference.field, difference.expected, difference.actual,
            );
        }
        for note in &self.notes {
            let _ = writeln!(out, "  note: {note}");
        }
        out
    }
}

fn preview(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes).replace('\n', "\\n");
    if text.chars().count() <= 160 {
        text
    } else {
        let short: String = text.chars().take(157).collect();
        format!("{short}...")
    }
}

/// Compare two byte buffers, returning a difference when they differ.
pub fn diff_bytes(field: impl Into<String>, expected: &[u8], actual: &[u8]) -> Option<Difference> {
    (expected != actual).then(|| Difference {
        field: field.into(),
        expected: preview(expected),
        actual: preview(actual),
    })
}

/// Compare two UTF-8 strings, returning a difference when they differ.
pub fn diff_text(field: impl Into<String>, expected: &str, actual: &str) -> Option<Difference> {
    diff_bytes(field, expected.as_bytes(), actual.as_bytes())
}

/// A JSON field whose value is inherently run-dependent and therefore replaced
/// by a stable token before comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizedField {
    /// Exact JSON key name (matched as `"key"` in the raw text scan).
    pub key: &'static str,
    /// Stable replacement token, without JSON quotes.
    pub token: &'static str,
    /// Why this field cannot be compared byte-for-byte.
    pub reason: &'static str,
}

/// The four named fields normalized in **both** the raw-stdout and structural
/// JSONL channels. Every other field stays byte-exact.
///
/// - `timestamp`: live wall-clock time (`round(time.time(), 3)`).
/// - `duration`: measured wall-clock transfer duration.
/// - `avg_speed_bps`: derived from `duration`, hence wall-clock dependent.
/// - `file_path`: per-side output directory plus `/private/var` vs `/var`.
pub const NONDETERMINISTIC_FIELDS: [NormalizedField; 4] = [
    NormalizedField {
        key: "timestamp",
        token: "<TS>",
        reason: "live wall-clock time",
    },
    NormalizedField {
        key: "duration",
        token: "<DURATION>",
        reason: "measured wall-clock transfer duration",
    },
    NormalizedField {
        key: "avg_speed_bps",
        token: "<SPEED>",
        reason: "derived from wall-clock duration",
    },
    NormalizedField {
        key: "file_path",
        token: "<PATH>",
        reason: "per-side out dir and /private/var vs /var symlink form",
    },
];

/// Return the length of the JSON scalar starting at `text[start..]`, if it is a
/// value we normalize (a quoted string or a number). Boolean/null/object/array
/// values are deliberately left untouched.
fn scalar_value_len(text: &str, start: usize) -> Option<usize> {
    let rest = &text[start..];
    let first = *rest.as_bytes().first()?;
    if first == b'"' {
        // JSON string: scan to the unescaped closing quote.
        let mut escaped = false;
        for (index, byte) in rest.bytes().enumerate().skip(1) {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                return Some(index + 1);
            }
        }
        None
    } else if first.is_ascii_digit() || matches!(first, b'-' | b'+' | b'.') {
        let length = rest
            .bytes()
            .take_while(|byte| {
                byte.is_ascii_digit() || matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E')
            })
            .count();
        (length > 0).then_some(length)
    } else {
        None
    }
}

/// Replace the values of every named field in [`NONDETERMINISTIC_FIELDS`] with
/// stable JSON string tokens, leaving **all other bytes untouched**.
///
/// The scan is deliberately textual (not a JSON re-serialization) so field
/// order, spacing and every other byte remain byte-exact. It targets only the
/// exact key names, so a genuine difference in any other field still fails.
/// Non-UTF-8 output is returned unchanged.
///
/// ```
/// use paradl_parity::normalize_stdout_fields;
/// let a = br#"{"event":"result","timestamp":1.1,"duration":0.4,"avg_speed_bps":121.1,"file_path":"/a/origin.bin"}"#;
/// let b = br#"{"event":"result","timestamp":9.9,"duration":0.0,"avg_speed_bps":1845.6,"file_path":"/b/origin.bin"}"#;
/// assert_eq!(normalize_stdout_fields(a), normalize_stdout_fields(b));
/// let c = br#"{"event":"result","timestamp":9.9,"duration":0.0,"avg_speed_bps":1845.6,"file_path":"/b/origin.bin","success":false}"#;
/// assert_ne!(normalize_stdout_fields(a), normalize_stdout_fields(c));
/// ```
pub fn normalize_stdout_fields(bytes: &[u8]) -> Vec<u8> {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return bytes.to_vec(),
    };
    if !NONDETERMINISTIC_FIELDS
        .iter()
        .any(|field| text.contains(&format!("\"{}\"", field.key)))
    {
        return bytes.to_vec();
    }

    // Single left-to-right pass; whichever named key occurs earliest is
    // replaced next. Whitespace after the key/colon is preserved verbatim.
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while cursor < text.len() {
        let mut best: Option<(usize, usize, &'static NormalizedField)> = None;
        for field in &NONDETERMINISTIC_FIELDS {
            let needle = format!("\"{}\"", field.key);
            if let Some(relative) = text[cursor..].find(&needle) {
                let start = cursor + relative;
                if best.is_none_or(|(earliest, _, _)| start < earliest) {
                    best = Some((start, start + needle.len(), field));
                }
            }
        }
        let Some((start, key_end, field)) = best else {
            break;
        };
        let after_key = &text[key_end..];
        let replacement = after_key
            .find(':')
            .and_then(|colon| {
                if !after_key[..colon]
                    .chars()
                    .all(|ch: char| ch.is_ascii_whitespace())
                {
                    return None;
                }
                let after_colon = &after_key[colon + 1..];
                let whitespace = after_colon
                    .len()
                    - after_colon
                        .trim_start_matches(|ch: char| ch.is_ascii_whitespace())
                        .len();
                let value_at = colon + 1 + whitespace;
                scalar_value_len(after_key, value_at).map(|len| (value_at, len))
            });

        match replacement {
            Some((value_at, value_len)) => {
                out.push_str(&text[cursor..start]);
                out.push_str(&text[start..key_end]);
                out.push_str(&after_key[..value_at]);
                out.push('"');
                out.push_str(field.token);
                out.push('"');
                cursor = key_end + value_at + value_len;
            }
            None => {
                // Key present but no scalar value: copy through unchanged so a
                // real difference in that value is still detected.
                out.push_str(&text[cursor..key_end]);
                cursor = key_end;
            }
        }
    }
    out.push_str(&text[cursor..]);
    out.into_bytes()
}

/// Backward-compatible alias for [`normalize_stdout_fields`].
///
/// Historically this normalized only `timestamp`; it now applies the full set
/// of [`NONDETERMINISTIC_FIELDS`] so every caller gets identical treatment in
/// the raw-stdout channel.
///
/// ```
/// use paradl_parity::normalize_stdout_timestamps;
/// let a = br#"{"event":"x","timestamp":1.1,"n":1}"#;
/// let b = br#"{"event":"x","timestamp":9.9,"n":1}"#;
/// assert_eq!(normalize_stdout_timestamps(a), normalize_stdout_timestamps(b));
/// let c = br#"{"event":"x","timestamp":9.9,"n":2}"#;
/// assert_ne!(normalize_stdout_timestamps(a), normalize_stdout_timestamps(c));
/// ```
pub fn normalize_stdout_timestamps(bytes: &[u8]) -> Vec<u8> {
    normalize_stdout_fields(bytes)
}

/// Parse newline-delimited JSONL and replace the values of
/// [`NONDETERMINISTIC_FIELDS`] with stable tokens, recursively inside event
/// objects. This is the structural counterpart of [`normalize_stdout_fields`].
pub fn normalize_jsonl(text: &str) -> Result<Vec<Value>, serde_json::Error> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut value: Value = serde_json::from_str(line)?;
            strip_nondeterministic_fields(&mut value);
            Ok(value)
        })
        .collect()
}

/// Replace the values of [`NONDETERMINISTIC_FIELDS`] in a parsed JSON value
/// (recursing into nested objects/arrays) with stable string tokens.
pub fn strip_nondeterministic_fields(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for field in &NONDETERMINISTIC_FIELDS {
                if map.contains_key(field.key) {
                    map.insert(field.key.to_owned(), Value::String(field.token.to_owned()));
                }
            }
            for nested in map.values_mut() {
                strip_nondeterministic_fields(nested);
            }
        }
        Value::Array(values) => {
            for nested in values {
                strip_nondeterministic_fields(nested);
            }
        }
        _ => {}
    }
}

/// Remove the `timestamp` field from an event value so goldens compare cleanly.
pub fn strip_timestamp(value: &mut Value) {
    if let Value::Object(map) = value {
        map.remove("timestamp");
    }
}

/// Snapshot a directory as `relative path -> hex-encoded bytes`.
///
/// Used for `.pdl.part` / `.pdl.json` sidecars and any files a case produces.
pub fn snapshot_dir(path: &Path) -> io::Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    if !path.exists() {
        return Ok(out);
    }
    let mut entries: Vec<_> = std::fs::read_dir(path)?.collect::<io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let entry_path = entry.path();
        if entry_path.is_file() {
            let bytes = std::fs::read(&entry_path)?;
            let name = entry_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("<non-utf8>")
                .to_owned();
            out.insert(name, hex_encode(&bytes));
        }
    }
    Ok(out)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(*byte >> 4) as usize] as char);
        out.push(HEX[(*byte & 0x0f) as usize] as char);
    }
    out
}

/// Diff two directory snapshots, including missing/extra files.
pub fn diff_snapshots(
    expected: &BTreeMap<String, String>,
    actual: &BTreeMap<String, String>,
) -> Vec<Difference> {
    let mut differences = Vec::new();
    for (name, expected_bytes) in expected {
        match actual.get(name) {
            Some(actual_bytes) => {
                if let Some(difference) = diff_bytes(format!("file:{name}"), expected_bytes.as_bytes(), actual_bytes.as_bytes()) {
                    differences.push(difference);
                }
            }
            None => differences.push(Difference {
                field: format!("file:{name}"),
                expected: preview(expected_bytes.as_bytes()),
                actual: "<missing>".to_owned(),
            }),
        }
    }
    for name in actual.keys() {
        if !expected.contains_key(name) {
            differences.push(Difference {
                field: format!("file:{name}"),
                expected: "<missing>".to_owned(),
                actual: preview(actual[name].as_bytes()),
            });
        }
    }
    differences
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_escapes_newlines() {
        assert_eq!(preview(b"a\nb"), "a\\nb");
    }

    #[test]
    fn normalize_jsonl_replaces_timestamps_with_tokens() {
        let normalized = normalize_jsonl(
            "{\"event\":\"x\",\"timestamp\":1.5,\"n\":1}\n{\"event\":\"y\",\"timestamp\":2.0}\n",
        )
        .unwrap();
        assert_eq!(normalized.len(), 2);
        assert_eq!(normalized[0]["timestamp"], "<TS>");
        assert_eq!(normalized[0]["n"], 1);
    }

    #[test]
    fn normalize_both_channels_replace_all_four_named_fields() {
        // (a) Only the three run-dependent fields differ -> CLEAN in both
        //     the raw-stdout and the parsed-JSONL channels.
        let python = br#"{"event":"result","timestamp":1.1,"duration":0.382,"avg_speed_bps":121.17,"file_path":"/var/folders/x/python/origin.bin","success":true}"#;
        let rust = br#"{"event":"result","timestamp":9.9,"duration":0.042,"avg_speed_bps":1845.64,"file_path":"/private/var/folders/x/rust/origin.bin","success":true}"#;
        assert_eq!(normalize_stdout_fields(python), normalize_stdout_fields(rust));

        let a = normalize_jsonl(std::str::from_utf8(python).unwrap()).unwrap();
        let b = normalize_jsonl(std::str::from_utf8(rust).unwrap()).unwrap();
        assert_eq!(a, b);
        assert_eq!(a[0]["duration"], "<DURATION>");
        assert_eq!(a[0]["avg_speed_bps"], "<SPEED>");
        assert_eq!(a[0]["file_path"], "<PATH>");

        // (b) A genuine difference in a NON-normalized field still fails in
        //     both channels.
        let rust_different = br#"{"event":"result","timestamp":9.9,"duration":0.042,"avg_speed_bps":1845.64,"file_path":"/private/var/folders/x/rust/origin.bin","success":false}"#;
        assert_ne!(
            normalize_stdout_fields(python),
            normalize_stdout_fields(rust_different),
        );
        let c = normalize_jsonl(std::str::from_utf8(rust_different).unwrap()).unwrap();
        assert_ne!(a, c);

        // Nested values are normalized too (events can carry nested payloads).
        let nested = br#"{"event":"result","properties":{"duration":0.1,"file_path":"/a"},"list":[{"avg_speed_bps":1.0}]}"#;
        let normalized = String::from_utf8(normalize_stdout_fields(nested)).unwrap();
        assert!(normalized.contains("\"duration\":\"<DURATION>\""), "{normalized}");
        let parsed = normalize_jsonl(std::str::from_utf8(nested).unwrap()).unwrap();
        assert_eq!(parsed[0]["properties"]["duration"], "<DURATION>");
        assert_eq!(parsed[0]["list"][0]["avg_speed_bps"], "<SPEED>");
    }

    #[test]
    fn raw_stdout_normalizer_ignores_timestamp_but_keeps_everything_else_byte_exact() {
        let first = br#"{"event": "x", "timestamp": 1.125, "n": 1}
"#;
        let second = br#"{"event": "x", "timestamp": 99.875, "n": 1}
"#;
        let different_payload = br#"{"event": "x", "timestamp": 99.875, "n": 2}
"#;
        assert_eq!(
            normalize_stdout_timestamps(first),
            normalize_stdout_timestamps(second),
        );
        assert_ne!(
            normalize_stdout_timestamps(first),
            normalize_stdout_timestamps(different_payload),
        );
        let normalized = String::from_utf8(normalize_stdout_timestamps(first)).unwrap();
        assert!(normalized.contains("\"event\": \"x\""), "{normalized}");
        assert!(normalized.contains("\"timestamp\": \"<TS>\""), "{normalized}");
        assert!(normalized.contains("\"n\": 1"), "{normalized}");
    }

    #[test]
    fn raw_stdout_normalizer_is_a_noop_for_non_json_and_non_numeric_timestamps() {
        let plain = b"parallel-dl (paradl) v1.0.0\n";
        assert_eq!(normalize_stdout_timestamps(plain), plain.to_vec());
        let non_numeric = br#"{"event":"x","timestamp":"<TS>","n":1}"#;
        assert_eq!(
            normalize_stdout_timestamps(non_numeric),
            non_numeric.to_vec(),
        );
        let binary = [0xff, 0xfe, 0x00, b'"', b't', b'i', b'm', b'e', 0x80];
        assert_eq!(normalize_stdout_timestamps(&binary), binary.to_vec());
    }
}
