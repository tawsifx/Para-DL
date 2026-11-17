//! Premiumize.me's API, as documented at <https://www.premiumize.me/api> (read 2026-11-05). Only
//! what MDM uses: the account, the supported services, and direct links (`transfer/directdl`).
//!
//! - Every call carries `Authorization: Bearer <API key>` (the docs' recommended way; never the
//!   legacy `?apikey=`, which ends up in logs).
//! - Every answer has `status`: `"success"` with its fields, or `"error"` with `message` and
//!   `code`, with HTTP 200 for business errors too; 500 is a server failure.
//! - Sign-in by code (OAuth device flow) needs a client registered by MDM's maintainer: until then,
//!   the user pastes the API key from the Account page.

use std::collections::HashMap;

use serde::Deserialize;
use url::Url;

use crate::{Fault, Http, ServiceKind};

/// Where Premiumize answers (tests: the lab server's `/pm/api/`).
pub const BASE: &str = "https://www.premiumize.me/api/";

#[derive(Debug, Deserialize)]
struct Status {
    status: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    code: String,
}

/// What an error code means for MDM (the documentation's list of codes).
pub(crate) fn fault_of(code: &str, message: &str) -> Fault {
    use mdm_engine::ServiceFault as F;
    Fault::Service(match code {
        "authentication_failed" => F::SignIn,
        "permission_denied" => F::AccountBlocked,
        "service_unsupported" => return Fault::Unsupported,
        "service_down" => F::HostDown,
        "service_limit_reached" | "account_limit_reached" => F::LimitReached,
        "rate_limit_reached" => F::TooManyRequests(None),
        "not_found" => F::FileUnavailable,
        "link_generation_failed" | "transient_error" | "unknown_error" => F::ServiceDown,
        _ if !message.is_empty() => F::Other(message.to_owned()),
        _ => F::Other(code.to_owned()),
    })
}

fn read<T: serde::de::DeserializeOwned>(
    status: u16,
    body: &[u8],
    retry: Option<u64>,
) -> Result<T, Fault> {
    use mdm_engine::ServiceFault as F;
    if status == 429 {
        return Err(Fault::Service(F::TooManyRequests(
            retry.map(std::time::Duration::from_secs),
        )));
    }
    let Ok(s) = serde_json::from_slice::<Status>(body) else {
        return Err(Fault::Service(if status >= 500 {
            F::ServiceDown
        } else {
            F::Other("an answer MDM can't read".into())
        }));
    };
    if s.status != "success" {
        return Err(fault_of(&s.code, &s.message));
    }
    serde_json::from_slice(body)
        .map_err(|_| Fault::Service(F::Other("an answer MDM can't read".into())))
}

async fn call<T: serde::de::DeserializeOwned>(
    http: &Http,
    req: reqwest::RequestBuilder,
) -> Result<T, Fault> {
    let (status, body, retry) = http.send(ServiceKind::Premiumize, req).await?;
    read(status, &body, retry)
}

/// `GET account/info`.
#[derive(Debug, Deserialize)]
pub(crate) struct Account {
    #[serde(default)]
    pub customer_id: serde_json::Value,
    /// Unix time; null for a free account.
    pub premium_until: Option<i64>,
}

pub(crate) async fn account(http: &Http, key: &str) -> Result<Account, Fault> {
    let url = http.services.pm.join("account/info").expect("relative");
    call(http, http.get(&url).bearer_auth(key)).await
}

#[derive(Debug, Deserialize)]
struct Services {
    #[serde(default)]
    directdl: Vec<String>,
    #[serde(default)]
    aliases: HashMap<String, Vec<String>>,
}

/// `GET services/list`: the domains of the services `transfer/directdl` takes (each service's
/// aliases, and its own name when that is a domain).
pub(crate) async fn domains(http: &Http, key: &str) -> Result<Vec<String>, Fault> {
    let url = http.services.pm.join("services/list").expect("relative");
    let s: Services = call(http, http.get(&url).bearer_auth(key)).await?;
    Ok(domains_of(s))
}

fn domains_of(s: Services) -> Vec<String> {
    let mut out = Vec::new();
    for svc in &s.directdl {
        if svc.contains('.') {
            out.push(svc.clone());
        }
        out.extend(s.aliases.get(svc).cloned().unwrap_or_default());
    }
    out
}

/// `POST transfer/directdl`: one entry per file of the source.
#[derive(Debug, Deserialize)]
pub(crate) struct DirectDl {
    #[serde(default)]
    pub content: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Entry {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub link: String,
}

pub(crate) async fn direct(http: &Http, key: &str, link: &Url) -> Result<DirectDl, Fault> {
    let url = http
        .services
        .pm
        .join("transfer/directdl")
        .expect("relative");
    call(
        http,
        http.post_form(&url, &[("src", link.as_str())])
            .bearer_auth(key),
    )
    .await
}

/// `POST transfer/create` (`src`): the transfer's id.
pub(crate) async fn create(http: &Http, key: &str, magnet: &str) -> Result<String, Fault> {
    #[derive(Deserialize)]
    struct Created {
        id: serde_json::Value,
    }
    let url = http.services.pm.join("transfer/create").expect("relative");
    let c: Created = call(
        http,
        http.post_form(&url, &[("src", magnet)]).bearer_auth(key),
    )
    .await?;
    match c.id {
        serde_json::Value::String(s) => Ok(s),
        serde_json::Value::Number(n) => Ok(n.to_string()),
        _ => Err(Fault::Service(mdm_engine::ServiceFault::ServiceDown)),
    }
}

/// `GET transfer/list`, then for a finished one `item/details` (one file) or `folder/list`,
/// walked (several).
pub(crate) async fn transfer_status(
    http: &Http,
    key: &str,
    id: &str,
) -> Result<crate::torrents::Status, Fault> {
    use crate::torrents::{PmTransfer, RemoteFile, Status};
    #[derive(Deserialize)]
    struct List {
        #[serde(default)]
        transfers: Vec<PmTransfer>,
    }
    let url = http.services.pm.join("transfer/list").expect("relative");
    let l: List = call(http, http.get(&url).bearer_auth(key)).await?;
    let Some(t) = l.transfers.into_iter().find(|t| t.id == id) else {
        return Ok(Status::Failed(mdm_engine::ServiceFault::FileUnavailable));
    };
    if let Some(s) = crate::torrents::pm_working(&t) {
        return Ok(s);
    }
    #[derive(Deserialize)]
    struct Item {
        #[serde(default)]
        name: String,
        #[serde(default)]
        size: u64,
        #[serde(default)]
        link: String,
        #[serde(default)]
        id: String,
        #[serde(default, rename = "type")]
        kind: String,
    }
    let mut files = Vec::new();
    if let Some(file) = t.file_id.as_deref() {
        let mut url = http.services.pm.join("item/details").expect("relative");
        url.query_pairs_mut()
            .append_pair("id", crate::plain_id(file)?);
        let i: Item = call(http, http.get(&url).bearer_auth(key)).await?;
        files.push(RemoteFile {
            path: i.name,
            size: i.size,
            link: i.link,
        });
    } else if let Some(folder) = t.folder_id.as_deref() {
        #[derive(Deserialize)]
        struct Folder {
            #[serde(default)]
            content: Vec<Item>,
        }
        // Breadth first, a folder's path before its files'; at most 64 folders.
        let mut todo = vec![(folder.to_owned(), String::new())];
        let mut seen = 0;
        while let Some((fid, prefix)) = todo.pop() {
            seen += 1;
            if seen > 64 {
                break;
            }
            let mut url = http.services.pm.join("folder/list").expect("relative");
            url.query_pairs_mut()
                .append_pair("id", crate::plain_id(&fid)?);
            let f: Folder = call(http, http.get(&url).bearer_auth(key)).await?;
            for i in f.content {
                let path = if prefix.is_empty() {
                    i.name.clone()
                } else {
                    format!("{prefix}/{}", i.name)
                };
                if i.kind == "folder" {
                    todo.push((i.id, path));
                } else if !i.link.is_empty() {
                    files.push(RemoteFile {
                        path,
                        size: i.size,
                        link: i.link,
                    });
                }
            }
        }
    }
    Ok(Status::Ready {
        name: t.name,
        files,
    })
}

/// `POST transfer/delete` (`id`): the record only; the files stay in the user's cloud.
pub(crate) async fn transfer_delete(http: &Http, key: &str, id: &str) -> Result<(), Fault> {
    let url = http.services.pm.join("transfer/delete").expect("relative");
    call::<serde_json::Value>(
        http,
        http.post_form(&url, &[("id", crate::plain_id(id)?)])
            .bearer_auth(key),
    )
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdm_engine::ServiceFault as F;

    #[test]
    fn the_documented_answers_read() {
        let body = br#"{"status":"success","content":[{"path":"Folder/video1.mkv","size":123456789,"link":"https://x/1"}]}"#;
        let d: DirectDl = read(200, body, None).unwrap();
        assert_eq!(
            (d.content[0].path.as_str(), d.content[0].size),
            ("Folder/video1.mkv", 123456789)
        );
        let a: Account = read(200, br#"{"status":"success","customer_id":"1234567","premium_until":null,"limit_used":0.42}"#, None).unwrap();
        assert_eq!(a.premium_until, None);
        let s: Services = serde_json::from_slice(
            br#"{"status":"success","directdl":["exampleservice1","host.com"],
            "aliases":{"exampleservice1":["example1.alt"],"host.com":["host.net"]}}"#,
        )
        .unwrap();
        assert_eq!(domains_of(s), vec!["example1.alt", "host.com", "host.net"]);
        let err = br#"{"status":"error","message":"Not found","code":"not_found"}"#;
        assert_eq!(
            read::<DirectDl>(200, err, None).unwrap_err(),
            Fault::Service(F::FileUnavailable)
        );
    }

    #[test]
    fn documented_codes_mean_what_they_say() {
        assert_eq!(
            fault_of("authentication_failed", ""),
            Fault::Service(F::SignIn)
        );
        assert_eq!(fault_of("service_unsupported", ""), Fault::Unsupported);
        assert_eq!(fault_of("service_down", ""), Fault::Service(F::HostDown));
        assert_eq!(
            fault_of("account_limit_reached", ""),
            Fault::Service(F::LimitReached)
        );
        assert_eq!(
            fault_of("rate_limit_reached", ""),
            Fault::Service(F::TooManyRequests(None))
        );
        assert_eq!(
            fault_of("new_code", "Its words"),
            Fault::Service(F::Other("Its words".into()))
        );
    }
}
