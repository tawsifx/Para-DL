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
}
