//! Reports on GitHub: a new issue on the Crash report form (.github/ISSUE_TEMPLATE/crash.yml)
//! filled from the report, and a search for one already filed for the same problem.

use serde::Deserialize;

pub const REPO: &str = "anefzaoui/misterdownloadmanager";

/// GitHub turns away very long addresses: the form's address is kept under this, the report cut
/// to fit (the full one goes in as the attached file).
pub const URL_BUDGET: usize = 7000;

/// The form's fields MDM fills, besides the report: (field id, value).
pub type Fields = Vec<(&'static str, String)>;

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// A new Crash report issue: `fields` (crash.yml's ids) and as much of `report` as fits, cut at
/// a line, with a note that the rest is in the file to attach.
pub fn new_issue_url(title: &str, fields: &Fields, report: &str, cut_note: &str) -> String {
    let mut url = format!(
        "https://github.com/{REPO}/issues/new?template=crash.yml&title={}",
        enc(title)
    );
    for (k, v) in fields {
        url.push_str(&format!("&{k}={}", enc(v)));
    }
    let head = format!("{url}&report=");
    let room = URL_BUDGET.saturating_sub(head.len());
    let whole = enc(report);
    if whole.len() <= room {
        return head + &whole;
    }
    let note = format!("\n\n{cut_note}\n");
    let room = room.saturating_sub(enc(&note).len());
    let mut kept = String::new();
    let mut used = 0;
    for line in report.lines() {
        let l = enc(&format!("{line}\n"));
        if used + l.len() > room {
            break;
        }
        used += l.len();
        kept.push_str(line);
        kept.push('\n');
    }
    head + &enc(&(kept + &note))
}

/// An issue that matches.
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Found {
    pub number: u64,
    pub title: String,
    pub html_url: String,
    pub state: String,
}

#[derive(Deserialize)]
struct Search {
    items: Vec<Found>,
}

/// The issue already filed for `signature` (its body holds it: the form's Signature field), open
/// ones first. None when there is none or GitHub can't be asked.
pub async fn find(http: &reqwest::Client, signature: &str) -> Option<Found> {
    let q = format!("repo:{REPO} is:issue in:body \"{signature}\"");
    let url = format!(
        "https://api.github.com/search/issues?q={}&sort=updated&per_page=5",
        enc(&q)
    );
    let r = http
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "MisterDownloadManager crash reporter")
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?;
    let s: Search = serde_json::from_slice(&r.bytes().await.ok()?).ok()?;
    let mut items = s.items;
    items.sort_by_key(|i| i.state != "open");
    items.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_report_is_cut_at_a_line_within_the_budget() {
        let fields = vec![
            ("version", "0.2.3".to_owned()),
            ("kind", "freeze".to_owned()),
        ];
        let report: String = (0..2000)
            .map(|i| format!("line {i} of the report\n"))
            .collect();
        let url = new_issue_url("Freeze in x", &fields, &report, "(cut: see the file)");
        assert!(url.len() <= URL_BUDGET);
        assert!(url.contains("template=crash.yml"));
        assert!(url.contains("&version=0.2.3"));
        let report_part = url.split("&report=").nth(1).unwrap();
        let decoded: String = url::form_urlencoded::parse(format!("r={report_part}").as_bytes())
            .next()
            .unwrap()
            .1
            .into_owned();
        assert!(decoded.starts_with("line 0 of the report\n"));
        assert!(decoded.trim_end().ends_with("(cut: see the file)"));
        // Whole lines only.
        assert!(
            decoded
                .lines()
                .all(|l| l.is_empty() || l.starts_with("line ") || l.starts_with('('))
        );
    }

    #[test]
    fn a_short_report_goes_whole() {
        let url = new_issue_url("t", &vec![], "short report", "cut");
        assert!(url.ends_with("&report=short+report"));
    }
}
