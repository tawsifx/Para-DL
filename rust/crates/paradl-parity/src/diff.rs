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

/// Replace the numeric value of every JSON `timestamp` field with the stable
/// token `"<TS>"`, leaving **all other bytes untouched**.
///
/// This is used on the raw-stdout channel for JSONL-mode cases: both the Python
/// oracle and the Rust binary emit live wall-clock values, so a byte compare of
/// the unmodified streams would flag a difference on every event. The scan is
/// deliberately textual (not a JSON re-serialization) so field order, spacing,
/// and every other byte remain byte-exact. Non-UTF-8 output and streams without
/// a numeric `timestamp` field are returned unchanged.
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
    const KEY: &str = "\"timestamp\"";
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(_) => return bytes.to_vec(),
    };
    if !text.contains(KEY) {
        return bytes.to_vec();
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    while let Some(relative) = text[cursor..].find(KEY) {
        let start = cursor + relative;
        let key_end = start + KEY.len();
        let after_key = &text[key_end..];
        let replaced = after_key
            .find(':')
            .and_then(|colon| {
                let before_colon = &after_key[..colon];
                if !before_colon.chars().all(|ch: char| ch.is_ascii_whitespace()) {
                    return None;
                }
                let after_colon = &after_key[colon + 1..];
                let whitespace = after_colon
                    .len()
                    - after_colon
                        .trim_start_matches(|ch: char| ch.is_ascii_whitespace())
                        .len();
                let number_at = colon + 1 + whitespace;
                let number = &after_key[number_at..];
                let length = number
                    .bytes()
                    .take_while(|byte| {
                        byte.is_ascii_digit() || matches!(byte, b'-' | b'+' | b'.' | b'e' | b'E')
                    })
                    .count();
                (length > 0).then_some((number_at, length))
            });
        match replaced {
            Some((number_at, length)) => {
                out.push_str(&text[cursor..key_end]);
                out.push_str(&after_key[..number_at]);
                out.push_str("\"<TS>\"");
                cursor = key_end + number_at + length;
            }
            None => {
                out.push_str(&text[cursor..key_end]);
                cursor = key_end;
            }
        }
    }
    out.push_str(&text[cursor..]);
    out.into_bytes()
}

/// Parse newline-delimited JSONL, replacing each `timestamp` with `0.0`.
///
/// Wall-clock timestamps are inherently nondeterministic; the harness compares
/// the *shape and payload* of events, never the instant they were emitted.
pub fn normalize_jsonl(text: &str) -> Result<Vec<Value>, serde_json::Error> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut value: Value = serde_json::from_str(line)?;
            strip_timestamp(&mut value);
            Ok(value)
        })
        .collect()
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
    fn normalize_jsonl_removes_timestamps() {
        let normalized = normalize_jsonl(
            "{\"event\":\"x\",\"timestamp\":1.5,\"n\":1}\n{\"event\":\"y\",\"timestamp\":2.0}\n",
        )
        .unwrap();
        assert_eq!(normalized.len(), 2);
        assert!(normalized[0].get("timestamp").is_none());
        assert_eq!(normalized[0]["n"], 1);
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
