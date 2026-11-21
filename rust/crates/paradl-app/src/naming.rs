//! Pure naming, routing-shape and destination helpers split out of
//! [`super::App`] by C2. No I/O beyond the deliberate orphan cleanup in
//! [`clean_orphans`]; every function is a byte-identical move from `app.rs`.

use std::path::PathBuf;

use paradl_cli::Cli;
use paradl_net::routing::RouteDecision;

pub(super) fn url_host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_owned))
        .unwrap_or_default()
}

pub(super) fn decision_name(decision: &RouteDecision) -> String {
    match decision {
        RouteDecision::Direct { .. } => "direct".to_string(),
        RouteDecision::Torrent { .. } => "torrent".to_string(),
        RouteDecision::Site { target_type, .. } => format!("{target_type:?}").to_ascii_lowercase(),
        RouteDecision::DelegateToWorker { target_type, .. } => {
            format!("{target_type:?}").to_ascii_lowercase()
        }
    }
}

/// Data shared by `target_classified` and `info`, straight from pure routing.
pub(super) struct ClassificationInfo {
    pub(super) target_type: String,
    pub(super) clean_url: String,
    pub(super) filename_hint: Option<String>,
    pub(super) content_length: Option<i64>,
    pub(super) accept_ranges: Option<bool>,
}

pub(super) fn classification_info(target: &str, decision: &RouteDecision) -> ClassificationInfo {
    let hint = decision_filename(target, decision);
    ClassificationInfo {
        target_type: decision_name(decision),
        clean_url: decision_clean_url(decision, target),
        filename_hint: Some(hint),
        content_length: match decision {
            RouteDecision::Direct { content_length, .. } => {
                content_length.and_then(|value| i64::try_from(value).ok())
            }
            _ => None,
        },
        accept_ranges: match decision {
            RouteDecision::Direct { accept_ranges, .. } => *accept_ranges,
            _ => None,
        },
    }
}

pub(super) fn decision_clean_url(decision: &RouteDecision, fallback: &str) -> String {
    let clean = match decision {
        RouteDecision::Direct { clean_url, .. }
        | RouteDecision::DelegateToWorker { clean_url, .. }
        | RouteDecision::Site { clean_url, .. }
        | RouteDecision::Torrent { clean_url } => clean_url.trim().to_owned(),
    };
    if clean.is_empty() {
        fallback.to_owned()
    } else {
        clean
    }
}

pub(super) fn decision_filename(target: &str, decision: &RouteDecision) -> String {
    match decision {
        RouteDecision::Direct { filename_hint, .. } => filename_hint
            .clone()
            .unwrap_or_else(|| fallback_name(target)),
        RouteDecision::Site { .. } => fallback_name(target),
        _ => fallback_name(target),
    }
}

pub(super) fn fallback_name(target: &str) -> String {
    target
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(|name| name.split('?').next().unwrap_or(name).to_owned())
        .unwrap_or_else(|| "download.bin".to_string())
}

pub(super) fn destination(cli: &Cli, file_name: &str) -> PathBuf {
    let dir = cli.output.as_ref().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let clean = paradl_core::sanitize_filename(file_name);
    dir.join(clean)
}

pub(super) fn clean_orphans(directory: &std::path::Path) -> Vec<String> {
    let mut removed = Vec::new();
    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let orphan = name.ends_with(".aria2")
                || name.ends_with(".part")
                || name.ends_with(".ytdl")
                || name.starts_with(".pdl_raw_");
            if orphan && std::fs::remove_file(entry.path()).is_ok() {
                removed.push(name);
            }
        }
    }
    removed
}
