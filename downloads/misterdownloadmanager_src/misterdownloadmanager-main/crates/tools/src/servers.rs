//! Media servers that can't be reached, swapped for the next one the site lists. YouTube hands
//! yt-dlp links to one cache server (`rrN---sn-XXXX.googlevideo.com`) and names its
//! alternatives in the link's `mn` parameter (`mn=sn-XXXX,sn-YYYY`); a browser's player falls
//! over to them when the first is dead, yt-dlp doesn't (2026-10-29: an ISP's in-network cache
//! unreachable from the user's line, blackholed in the hosts file; yt-dlp gave up after 10
//! retries while the second server answered at once). The host is not part of the link's
//! signature (`sparams`), so the same link works on the alternative.

use std::collections::{BTreeMap, BTreeSet};
use std::net::ToSocketAddrs;
use std::time::Duration;

use reqwest::Url;
use serde_json::Value;

/// How long a server gets to accept a connection.
const CONNECT: Duration = Duration::from_secs(2);

/// Whether `host` accepts a connection on 443: it resolves to a real address (a hosts-file
/// block answers 0.0.0.0 or ::) and one of those connects within [`CONNECT`].
pub fn reachable(host: &str) -> bool {
    let Ok(addrs) = (host, 443).to_socket_addrs() else {
        return false;
    };
    // Through the chosen network adapter, as yt-dlp's own connections go (#35).
    let route = crate::proc::route();
    addrs
        .filter(|a| !a.ip().is_unspecified())
        .any(|a| mdm_engine::net::tcp_connect_blocking(&route, a, CONNECT).is_ok())
}

/// Every string in `v` (links sit in `formats[].url`, `requested_formats`, fragments…).
fn strings_mut(v: &mut Value, f: &mut impl FnMut(&mut String)) {
    match v {
        Value::String(s) => f(s),
        Value::Array(a) => a.iter_mut().for_each(|x| strings_mut(x, f)),
        Value::Object(o) => o.values_mut().for_each(|x| strings_mut(x, f)),
        _ => {}
    }
}

/// The servers to try instead of `host`, from a link's `mn` list: `rr1---sn-A.googlevideo.com`
/// with `mn=sn-A,sn-B` gives `rr1---sn-B.googlevideo.com`.
fn alternatives(host: &str, link: &Url) -> Vec<String> {
    let Some((prefix, rest)) = host.split_once("---") else {
        return Vec::new();
    };
    let Some((node, domain)) = rest.split_once('.') else {
        return Vec::new();
    };
    link.query_pairs()
        .find(|(k, _)| k == "mn")
        .map(|(_, v)| {
            v.split(',')
                .map(str::trim)
                .filter(|n| !n.is_empty() && *n != node)
                .map(|n| format!("{prefix}---{n}.{domain}"))
                .collect()
        })
        .unwrap_or_default()
}

/// Asks media servers whether they really serve a stream: a 1 KiB request the way yt-dlp asks
/// (`&range=` in the link), with the stream's own headers, one at a time over one client.
/// YouTube's alternatives refuse some streams some of the time (live 2026-10-29: 403 for one
/// audio stream, 200 for its neighbours) — and refuse a burst: the same 30 streams checked at
/// once came back refused but one, checked one after another all served.
pub struct Prober {
    // Dropped before the runtime (fields drop in order), inside it (`Drop`).
    client: Option<reqwest::Client>,
    rt: tokio::runtime::Runtime,
}

impl Prober {
    pub fn new() -> Self {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime for the checks");
        // The client belongs to this runtime: made, used and dropped inside it.
        // Through the chosen network adapter, as yt-dlp's own connections go (#35); none at all
        // while it's down.
        let route = crate::proc::route();
        let client = rt.block_on(async {
            mdm_engine::net::client_builder(
                &Default::default(),
                &route,
                "https://www.googlevideo.com/",
            )
            .ok()?
            .timeout(Duration::from_secs(8))
            .build()
            .ok()
        });
        Prober { client, rt }
    }

    pub fn serves(&self, link: &str, headers: &BTreeMap<String, String>) -> bool {
        let Some(client) = &self.client else {
            return false;
        };
        let sep = if link.contains('?') { '&' } else { '?' };
        let mut req = client.get(format!("{link}{sep}range=0-1023"));
        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }
        self.rt
            .block_on(async { req.send().await.is_ok_and(|r| r.status().is_success()) })
    }
}

impl Drop for Prober {
    fn drop(&mut self) {
        let _inside = self.rt.enter();
        self.client = None;
    }
}

impl Default for Prober {
    fn default() -> Self {
        Self::new()
    }
}

/// A format's request headers (`http_headers` in yt-dlp's information).
fn headers_of(format: &Value) -> BTreeMap<String, String> {
    format
        .get("http_headers")
        .and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

/// What [`reroute`] changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rerouted {
    /// (dead host, replacement), each pair once.
    pub swaps: Vec<(String, String)>,
    /// The streams left out because no reachable alternative serves them, as "137 (1080p)" (for
    /// the download's log: a quality chosen from the window may be among them, #94).
    pub left_out: Vec<String>,
}

impl Rerouted {
    pub fn is_empty(&self) -> bool {
        self.swaps.is_empty() && self.left_out.is_empty()
    }
}

/// A format as the log names it: its id and its height (or "audio").
fn describe(f: &Value) -> String {
    let id = f.get("format_id").and_then(Value::as_str).unwrap_or("?");
    match f.get("height").and_then(Value::as_u64) {
        Some(h) => format!("{id} ({h}p)"),
        None if f.get("vcodec").and_then(Value::as_str) == Some("none") => format!("{id} (audio)"),
        None => id.to_owned(),
    }
}

/// The media servers yt-dlp's failure names as unreachable (#94): it couldn't resolve one, or
/// connect to it, or it timed out (`HTTPSConnection(host='rr1---sn-x.googlevideo.com',
/// port=443): Failed to resolve 'rr1---sn-x.googlevideo.com'`). A server answered at look-up
/// time may be gone by download time (a flaky resolver): the next look-up treats these as dead.
pub fn unreachable_in(message: &str) -> BTreeSet<String> {
    let network = [
        "Failed to resolve",
        "getaddrinfo",
        "Unable to connect",
        "timed out",
        "Connection refused",
        "Max retries",
    ]
    .iter()
    .any(|w| message.contains(w));
    if !network {
        return BTreeSet::new();
    }
    message
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '.'))
        .filter(|w| w.ends_with(".googlevideo.com") && w.contains("---"))
        .map(str::to_owned)
        .collect()
}

/// yt-dlp couldn't find the chosen format among the ones the page (or our edit of it) offers.
pub fn format_unavailable(message: &str) -> bool {
    message.contains("Requested format is not available")
}

/// Move the streams of unreachable media servers to an alternative that serves them; returns
/// what changed. `alive` decides whether a server can be reached ([`reachable`], all at once),
/// `serves` whether it really serves one stream ([`Prober::serves`], one after another). A stream
/// no reachable alternative serves is left out, so yt-dlp chooses an equivalent one (another
/// codec of the same quality, the other audio); the video job falls back to the nearest quality
/// when the chosen one is among them (video.rs).
pub fn reroute(
    info: &mut Value,
    alive: impl Fn(&str) -> bool + Sync,
    serves: impl Fn(&str, &BTreeMap<String, String>) -> bool,
) -> Rerouted {
    // Media hosts with an `mn` list, and one link of each (for its alternatives).
    let mut links: BTreeMap<String, Url> = BTreeMap::new();
    strings_mut(info, &mut |s| {
        if let Ok(u) = Url::parse(s)
            && let Some(h) = u.host_str()
            && h.ends_with(".googlevideo.com")
            && u.query_pairs().any(|(k, _)| k == "mn")
        {
            links.entry(h.to_owned()).or_insert(u);
        }
    });
    let candidates: BTreeSet<String> = links
        .iter()
        .flat_map(|(h, u)| std::iter::once(h.clone()).chain(alternatives(h, u)))
        .collect();
    let up: BTreeSet<String> = std::thread::scope(|s| {
        let checks: Vec<_> = candidates
            .iter()
            .map(|h| (h, s.spawn(|| alive(h))))
            .collect();
        checks
            .into_iter()
            .filter_map(|(h, c)| c.join().unwrap_or(false).then(|| h.clone()))
            .collect()
    });
    let dead: BTreeSet<String> = links
        .iter()
        .filter(|(h, u)| !up.contains(*h) && alternatives(h, u).iter().any(|a| up.contains(a)))
        .map(|(h, _)| h.clone())
        .collect();
    if dead.is_empty() {
        return Rerouted::default();
    }
    let Some(Value::Array(formats)) = info.get_mut("formats") else {
        return Rerouted::default();
    };
    // HLS formats list their pieces in a manifest fetched later, on the same servers, which
    // can't be edited here: leave them out so the choice falls on direct links (live
    // 2026-10-29: "worstaudio" picked HLS 233 and got an empty file).
    formats.retain(|f| {
        !f.get("protocol")
            .and_then(Value::as_str)
            .is_some_and(|p| p.starts_with("m3u8"))
    });
    // Each stream on a dead server: the first reachable alternative that serves it — asked one
    // stream at a time (servers refuse a burst).
    let moves: Vec<(usize, String, Option<String>)> = formats
        .iter()
        .enumerate()
        .filter_map(|(i, f)| {
            let link = Url::parse(f.get("url")?.as_str()?).ok()?;
            let host = link.host_str()?.to_owned();
            if !dead.contains(&host) {
                return None;
            }
            let headers = headers_of(f);
            let to = alternatives(&host, &link)
                .into_iter()
                .filter(|a| up.contains(a))
                .find(|a| {
                    let mut moved = link.clone();
                    moved.set_host(Some(a)).is_ok() && serves(moved.as_str(), &headers)
                });
            Some((i, host, to))
        })
        .collect();
    let mut swaps: Vec<(String, String)> = Vec::new();
    let mut drop = BTreeSet::new();
    let mut left_out = Vec::new();
    for (i, host, to) in moves {
        match to {
            Some(alt) => {
                if let Some(Value::String(u)) = formats[i].get_mut("url") {
                    *u = u.replacen(&format!("://{host}/"), &format!("://{alt}/"), 1);
                }
                if !swaps.iter().any(|(d, a)| *d == host && *a == alt) {
                    swaps.push((host, alt));
                }
            }
            None => {
                left_out.push(describe(&formats[i]));
                drop.insert(i);
            }
        }
    }
    let mut i = 0;
    formats.retain(|_| {
        i += 1;
        !drop.contains(&(i - 1))
    });
    // yt-dlp chooses again from `formats`; its earlier choice would still name the dead server.
    if let Some(o) = info.as_object_mut() {
        o.remove("requested_formats");
    }
    Rerouted { swaps, left_out }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEAD: &str = "rr1---sn-u0opnpxuxa-u0oz.googlevideo.com";
    const ALT: &str = "rr1---sn-hpa7znsz.googlevideo.com";

    fn info() -> Value {
        let link = format!(
            "https://{DEAD}/videoplayback?expire=1&mn=sn-u0opnpxuxa-u0oz%2Csn-hpa7znsz&ms=au%2Crdu&itag=139"
        );
        serde_json::json!({
            "id": "Tg6byXUo8RU",
            "formats": [
                { "format_id": "139", "url": link },
                { "format_id": "249", "url": link.replace("itag=139", "itag=249") },
                { "format_id": "233", "protocol": "m3u8_native",
                  "url": "https://manifest.googlevideo.com/api/manifest/hls" }
            ],
            "requested_formats": [{ "url": link }]
        })
    }

    #[test]
    fn an_unreachable_server_is_swapped_for_its_alternative() {
        let mut v = info();
        let r = reroute(&mut v, |h| h != DEAD, |_, _| true);
        assert_eq!(r.swaps, vec![(DEAD.to_owned(), ALT.to_owned())]);
        assert!(r.left_out.is_empty());
        let text = v.to_string();
        assert!(!text.contains(DEAD), "every link moved: {text}");
        assert_eq!(
            text.matches(ALT).count(),
            2,
            "both direct formats; requested_formats gone"
        );
        // The rest of the link (signature, parameters) is untouched.
        assert!(text.contains(&format!("https://{ALT}/videoplayback?expire=1&mn=")));
        // HLS (its pieces would still come from the dead server) is left out.
        assert!(!text.contains("manifest.googlevideo.com"));
        assert_eq!(v["formats"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn reachable_servers_and_dead_ends_stay() {
        let mut v = info();
        assert!(reroute(&mut v, |_| true, |_, _| true).is_empty());
        assert_eq!(v, info());
        // No alternative answers either: nothing better to point at, yt-dlp reports the error.
        let mut v = info();
        assert!(reroute(&mut v, |_| false, |_, _| true).is_empty());
        assert_eq!(v, info());
    }

    #[test]
    fn a_stream_the_alternative_refuses_is_left_out() {
        // 249 is refused by the alternative (403), 139 is served: only 139 stays.
        let mut v = info();
        let r = reroute(&mut v, |h| h != DEAD, |link, _| !link.contains("itag=249"));
        assert_eq!(r.swaps, vec![(DEAD.to_owned(), ALT.to_owned())]);
        // Said, for the log: what the choice may have lost (#94).
        assert_eq!(r.left_out, ["249"]);
        let ids: Vec<&str> = v["formats"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["format_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["139"]);
        // Nothing served anywhere: every stream on the dead server is out, nothing swapped.
        let mut v = info();
        let r = reroute(&mut v, |h| h != DEAD, |_, _| false);
        assert!(r.swaps.is_empty());
        assert_eq!(r.left_out, ["139", "249"]);
        assert!(v["formats"].as_array().unwrap().is_empty());
    }

    #[test]
    fn a_left_out_stream_is_named_by_its_height() {
        let f = serde_json::json!({"format_id": "137", "height": 1080});
        assert_eq!(describe(&f), "137 (1080p)");
        let a = serde_json::json!({"format_id": "251", "vcodec": "none"});
        assert_eq!(describe(&a), "251 (audio)");
    }

    /// #94: yt-dlp's words for a server it couldn't reach name the server; other failures don't.
    #[test]
    fn unreachable_servers_from_ytdlps_failure() {
        let dns = "[download] Got error: HTTPSConnection(host='rr1---sn-u0opnpxuxa-u0oz.googlevideo.com', port=443): Failed to resolve 'rr1---sn-u0opnpxuxa-u0oz.googlevideo.com' ([Errno 11004] getaddrinfo failed). Giving up after 10 retries";
        assert_eq!(
            unreachable_in(dns).into_iter().collect::<Vec<_>>(),
            ["rr1---sn-u0opnpxuxa-u0oz.googlevideo.com"]
        );
        let refused = "Unable to connect to rr3---sn-abc.googlevideo.com: timed out";
        assert_eq!(unreachable_in(refused).len(), 1);
        // A googlevideo host in another kind of failure (403) isn't "unreachable".
        assert!(
            unreachable_in("HTTP Error 403: Forbidden (rr1---sn-x.googlevideo.com)").is_empty()
        );
        assert!(unreachable_in("Failed to resolve 'example.com'").is_empty());
        assert!(format_unavailable(
            "[youtube] jvfC98jCBuk: Requested format is not available. Use --list-formats for a list of available formats"
        ));
        assert!(!format_unavailable("HTTP Error 403"));
    }

    #[test]
    fn a_blackholed_name_is_unreachable() {
        // A hosts-file block answers 0.0.0.0; "localhost" style real names would connect.
        assert!(!reachable("0.0.0.0"));
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// `MDM_LIVE_INFO=<yt-dlp -J output> cargo test -p mdm-tools live_serves -- --ignored
    /// --nocapture`: each direct stream on its first alternative, one by one.
    #[test]
    #[ignore]
    fn live_serves() {
        let info: Value = serde_json::from_str(
            &std::fs::read_to_string(std::env::var("MDM_LIVE_INFO").unwrap()).unwrap(),
        )
        .unwrap();
        for f in info["formats"].as_array().unwrap() {
            let Some(link) = f["url"].as_str().and_then(|u| Url::parse(u).ok()) else {
                continue;
            };
            if f["protocol"].as_str() != Some("https")
                || !link.host_str().unwrap_or("").ends_with("googlevideo.com")
            {
                continue;
            }
            let host = link.host_str().unwrap().to_owned();
            let Some(alt) = alternatives(&host, &link).into_iter().next() else {
                continue;
            };
            let mut moved = link.clone();
            moved.set_host(Some(&alt)).unwrap();
            let same = moved.as_str() == link.as_str().replacen(&host, &alt, 1);
            println!(
                "{} on {alt}: {} (url unchanged otherwise: {same})",
                f["format_id"],
                Prober::new().serves(moved.as_str(), &headers_of(f))
            );
        }
    }

    #[test]
    #[ignore]
    fn live_reroute() {
        let mut info: Value = serde_json::from_str(
            &std::fs::read_to_string(std::env::var("MDM_LIVE_INFO").unwrap()).unwrap(),
        )
        .unwrap();
        let before = info["formats"].as_array().unwrap().len();
        let t = std::time::Instant::now();
        let prober = Prober::new();
        let swaps = reroute(&mut info, reachable, |l, h| prober.serves(l, h));
        let ids: Vec<String> = info["formats"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["format_id"].to_string())
            .collect();
        println!(
            "swaps {swaps:?}; formats {before} -> {}: {ids:?} in {:?}",
            ids.len(),
            t.elapsed()
        );
        if let Ok(out) = std::env::var("MDM_LIVE_OUT") {
            std::fs::write(out, info.to_string()).unwrap();
        }
    }
}
