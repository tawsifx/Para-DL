// Modifications:
//   - craft_atomic::atomic_write -> std write-temp-then-rename (`atomic_write` below),
//     per the trimmed-vendoring ruling (no git-patched deps).
//   - Removed the `migrate_legacy` (.idmnext renaming) and inline tests;
//     the latter live in src/tests.rs. Credentials/request headers remain deliberately unstored.
//! Resume state: `<file>.pdl.json` beside the part file (notes/ENGINE.md §3).
//!
//! Request headers (cookies, authorization) are deliberately not stored: whoever resumes the
//! download supplies them again, so no credentials sit in the download folder.

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use url::Url;

use crate::planner::SavedSegment;
use crate::verify::Hash;

const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeState {
    pub version: u32,
    /// The URL the user asked for.
    pub url: Url,
    /// The URL after redirects; segments are fetched from here. **Never dropped on recycle.**
    pub final_url: Url,
    /// The finished file's path (the part file is `naming::part_path(dest)`).
    pub dest: PathBuf,
    pub size: u64,
    /// What identifies the version of the file the data so far was fetched under.
    #[serde(default)]
    pub validators: Validators,
    /// Before `validators`: the one `If-Range` value used (read once, then migrated).
    #[serde(default, rename = "validator", skip_serializing)]
    legacy_validator: Option<String>,
    /// Bytes `[start, cursor)` of each segment are on disk.
    pub segments: Vec<SavedSegment>,
    /// The `Content-Type` the file was first served with.
    #[serde(default)]
    pub content_type: Option<String>,
    /// The hash the finished file must have, if known.
    #[serde(default)]
    pub checksum: Option<Hash>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub used_agent_fallback: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub final_get: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub browser_like: bool,
    /// The server gave no Content-Length / Accept-Ranges: the download was one plain stream.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unknown_size: bool,
    /// `Content-Disposition` (or URL) file name learned during the probe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_name: Option<String>,
}

impl ResumeState {
    pub fn new(url: Url, final_url: Url, dest: PathBuf, size: u64, validators: Validators) -> Self {
        ResumeState {
            version: VERSION,
            url,
            final_url,
            dest,
            size,
            validators,
            legacy_validator: None,
            segments: Vec::new(),
            content_type: None,
            checksum: None,
            used_agent_fallback: false,
            final_get: false,
            browser_like: false,
            unknown_size: false,
            suggested_name: None,
        }
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut s: ResumeState =
            serde_json::from_slice(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some(v) = s.legacy_validator.take()
            && s.validators == Validators::default()
        {
            s.validators = Validators::migrate(&v, s.final_url.host_str());
        }
        if s.version != VERSION {
            return Err(format!(
                "{}: unsupported version {}",
                path.display(),
                s.version
            ));
        }
        Ok(s)
    }

    /// Persist with `segments`. `part` is flushed first, so the saved positions never claim
    /// bytes that are not on disk yet — even across a power loss.
    pub fn save(&mut self, path: &Path, segments: Vec<SavedSegment>, part: &File) -> io::Result<()> {
        part.sync_data()?;
        self.segments = segments;
        self.write(path)
    }

    /// Write as is (its segments must already be on disk).
    pub fn write(&self, path: &Path) -> io::Result<()> {
        let json = serde_json::to_vec_pretty(self).map_err(io::Error::other)?;
        atomic_write(path, &json)
    }

    /// Bytes already on disk.
    pub fn downloaded(&self) -> u64 {
        self.segments
            .iter()
            .map(|s| s.cursor.saturating_sub(s.start))
            .sum()
    }
}

/// Replacement for the `craft_atomic::atomic_write`: write to a sibling temp file,
/// flush it, then rename over the destination. A reader sees either the old or the new file.
fn atomic_write(path: &Path, data: &[u8]) -> io::Result<()> {
    // F13: a unique temp name avoids two concurrent writers clobbering each other's file.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp-{}-{nanos}", std::process::id()));
    let tmp = PathBuf::from(tmp);
    {
        let mut f = File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// What identifies a version of the file, for `If-Range` (RFC 9110 §13.1.5).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Validators {
    /// A strong ETag, and the host that issued it (CDN nodes may each have their own).
    pub etag: Option<String>,
    pub etag_host: Option<String>,
    /// Last-Modified, kept only when it is a strong validator (`Date` at least 1 s later).
    pub last_modified: Option<String>,
}

impl Validators {
    pub fn from_response(
        etag: Option<&str>,
        last_modified: Option<&str>,
        date: Option<&str>,
        host: Option<&str>,
    ) -> Self {
        let etag = match etag.map(str::trim) {
            Some(e) if is_strong_etag(e) => Some(e.to_owned()),
            Some(_) => return Validators::default(),
            None => None,
        };
        let last_modified = last_modified
            .filter(|lm| is_strong_date(lm, date))
            .map(str::to_owned);
        Validators {
            etag_host: etag.as_ref().and(host.map(str::to_owned)),
            etag,
            last_modified,
        }
    }

    /// The `If-Range` value for a request to `host`: the ETag only to the host that issued it,
    /// else the date.
    pub fn if_range(&self, host: Option<&str>) -> Option<&str> {
        self.etag
            .as_deref()
            .filter(|_| self.etag_host.as_deref() == host)
            .or(self.last_modified.as_deref())
    }

    fn migrate(v: &str, host: Option<&str>) -> Self {
        if is_strong_etag(v) {
            Validators {
                etag: Some(v.to_owned()),
                etag_host: host.map(str::to_owned),
                last_modified: None,
            }
        } else {
            Validators {
                last_modified: httpdate::parse_http_date(v).is_ok().then(|| v.to_owned()),
                ..Validators::default()
            }
        }
    }
}

/// `"…"` without inner quotes (a weak `W/"…"` or an unquoted value is not).
fn is_strong_etag(e: &str) -> bool {
    e.len() >= 2 && e.starts_with('"') && e.ends_with('"') && !e[1..e.len() - 1].contains('"')
}

/// A Last-Modified is strong only if the response's `Date` is at least a second later (RFC
/// 9110 §8.8.2.2).
fn is_strong_date(last_modified: &str, date: Option<&str>) -> bool {
    let (Ok(lm), Some(Ok(date))) = (
        httpdate::parse_http_date(last_modified),
        date.map(httpdate::parse_http_date),
    ) else {
        return false;
    };
    date >= lm + std::time::Duration::from_secs(1)
}

/// `<dest>.pdl.json`.
pub fn state_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".pdl.json");
    PathBuf::from(s)
}
