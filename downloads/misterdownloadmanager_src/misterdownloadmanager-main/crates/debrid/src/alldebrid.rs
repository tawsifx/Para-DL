//! AllDebrid's API v4 / v4.1, as documented at <https://docs.alldebrid.com/> (read 2026-11-05).
//! Only what MDM uses: the account, the supported domains, unlocking a link (and its delayed
//! flow), and sign-in by PIN.
//!
//! - Every call carries `Authorization: Bearer <apikey>`; no `agent` parameter (the changelog of
//!   15/01/2025 removed it). POST bodies are form fields.
//! - Every answer is an envelope: `{"status":"success","data":{…}}` or
//!   `{"status":"error","error":{"code":"…","message":"…"}}`, decided by `status`, not by the
//!   HTTP code (the docs say errors may come with 200; a 429 or 503 is too many requests).
//! - Demo keys (`staticDemoApikeyPrem`…) get the documentation's static answers (`"demo"`).

use serde::Deserialize;
use url::Url;

use crate::{Fault, Http, ServiceKind};

/// Where AllDebrid answers: `https://api.alldebrid.com/` (tests: the lab server's `/ad/`).
pub const BASE: &str = "https://api.alldebrid.com/";

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    status: String,
    data: Option<T>,
    error: Option<ErrorBody>,
}

#[derive(Debug, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    code: String,
    #[serde(default)]
    message: String,
}

/// What an error code means for MDM (the documentation's list of codes).
pub(crate) fn fault_of(code: &str, message: &str) -> Fault {
    use mdm_engine::ServiceFault as F;
    Fault::Service(match code {
        "AUTH_MISSING_APIKEY" | "AUTH_BAD_APIKEY" | "AUTH_MISSING_AGENT" | "AUTH_BAD_AGENT" => {
            F::SignIn
        }
        "AUTH_BLOCKED" => F::NeedsApproval,
        "AUTH_USER_BANNED" | "ACCOUNT_INVALID" => F::AccountBlocked,
        "MUST_BE_PREMIUM" | "FREE_TRIAL_LIMIT_REACHED" => F::NotPremium,
        "LINK_HOST_NOT_SUPPORTED" | "LINK_NOT_SUPPORTED" => return Fault::Unsupported,
        "LINK_HOST_UNAVAILABLE" | "LINK_TEMPORARY_UNAVAILABLE" | "LINK_HOST_FULL" => F::HostDown,
        "LINK_HOST_LIMIT_REACHED" => F::LimitReached,
        "LINK_TOO_MANY_DOWNLOADS" => F::TooManyActive,
        "LINK_DOWN" => F::FileUnavailable,
        "LINK_PASS_PROTECTED" => F::PasswordNeeded,
        "MAGNET_MUST_BE_PREMIUM" => F::NotPremium,
        "MAGNET_NO_SERVER" => F::IpRefused,
        "MAGNET_TOO_MANY_ACTIVE" => F::TooManyActive,
        "MAGNET_TOO_MANY" => F::LimitReached,
        "MAGNET_INVALID_ID" => F::FileUnavailable,
        "NO_SERVER" => F::IpRefused,
        "MAINTENANCE" | "GENERIC" | "DELAYED_INVALID_ID" | "LINK_ERROR" => F::ServiceDown,
        _ if !message.is_empty() => F::Other(message.to_owned()),
        _ => F::Other(code.to_owned()),
    })
}

/// Read an answer: the envelope's data, or what its error means.
fn read<T: serde::de::DeserializeOwned>(
    status: u16,
    body: &[u8],
    retry: Option<u64>,
) -> Result<T, Fault> {
    use mdm_engine::ServiceFault as F;
    if status == 429 || status == 503 {
        return Err(Fault::Service(F::TooManyRequests(
            retry.map(std::time::Duration::from_secs),
        )));
    }
    let Ok(e) = serde_json::from_slice::<Envelope<T>>(body) else {
        return Err(Fault::Service(if status >= 500 {
            F::ServiceDown
        } else {
            F::Other("an answer MDM can't read".into())
        }));
    };
    match (e.status.as_str(), e.data, e.error) {
        ("success", Some(d), _) => Ok(d),
        (_, _, Some(err)) => Err(fault_of(&err.code, &err.message)),
        _ => Err(Fault::Service(F::ServiceDown)),
    }
}

async fn call<T: serde::de::DeserializeOwned>(
    http: &Http,
    req: reqwest::RequestBuilder,
) -> Result<T, Fault> {
    let (status, body, retry) = http.send(ServiceKind::AllDebrid, req).await?;
    read(status, &body, retry)
}

#[derive(Debug, Deserialize)]
struct UserData {
    user: User,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct User {
    pub username: String,
    #[serde(default)]
    pub is_premium: bool,
    /// Unix time; 0 when not premium.
    #[serde(default)]
    pub premium_until: i64,
}

/// `GET v4/user`.
pub(crate) async fn user(http: &Http, key: &str) -> Result<User, Fault> {
    let url = http.services.ad.join("v4/user").expect("relative");
    call::<UserData>(http, http.get(&url).bearer_auth(key))
        .await
        .map(|d| d.user)
}

#[derive(Debug, Deserialize)]
struct Domains {
    #[serde(default)]
    hosts: Vec<String>,
}

/// `GET v4/hosts/domains` (public): the file hosts' domains, their other names included.
pub(crate) async fn domains(http: &Http) -> Result<Vec<String>, Fault> {
    let url = http.services.ad.join("v4/hosts/domains").expect("relative");
    call::<Domains>(http, http.get(&url)).await.map(|d| d.hosts)
}

/// `POST v4/link/unlock`.
#[derive(Debug, Deserialize)]
pub(crate) struct Unlock {
    #[serde(default)]
    pub link: String,
    pub filename: Option<String>,
    #[serde(default)]
    pub filesize: i64,
    /// Set when the link takes time to generate: poll `link/delayed` with it.
    pub delayed: Option<i64>,
}

pub(crate) async fn unlock(http: &Http, key: &str, link: &Url) -> Result<Unlock, Fault> {
    let url = http.services.ad.join("v4/link/unlock").expect("relative");
    call(
        http,
        http.post_form(&url, &[("link", link.as_str())])
            .bearer_auth(key),
    )
    .await
}

/// `POST v4/link/delayed`: 1 still processing, 2 ready (`link`), 3 failed.
#[derive(Debug, Deserialize)]
pub(crate) struct Delayed {
    pub status: i64,
    #[serde(default)]
    pub time_left: i64,
    pub link: Option<String>,
}

pub(crate) async fn delayed(http: &Http, key: &str, id: i64) -> Result<Delayed, Fault> {
    let url = http.services.ad.join("v4/link/delayed").expect("relative");
    let id = id.to_string();
    call(
        http,
        http.post_form(&url, &[("id", id.as_str())])
            .bearer_auth(key),
    )
    .await
}

/// `GET v4.1/pin/get`: the PIN to type at `user_url`, and the check token.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct Pin {
    pub pin: String,
    pub check: String,
    pub expires_in: u64,
    pub user_url: String,
}

pub(crate) async fn pin(http: &Http) -> Result<Pin, Fault> {
    let url = http.services.ad.join("v4.1/pin/get").expect("relative");
    call(http, http.get(&url)).await
}

#[derive(Debug, Deserialize)]
struct PinCheck {
    #[serde(default)]
    activated: bool,
    apikey: Option<String>,
}

/// `POST v4/pin/check`: the apikey once the user typed the PIN, None while waiting.
pub(crate) async fn pin_check(http: &Http, pin: &Pin) -> Result<Option<String>, Fault> {
    let url = http.services.ad.join("v4/pin/check").expect("relative");
    let form = [("check", pin.check.as_str()), ("pin", pin.pin.as_str())];
    let c: PinCheck = call(http, http.post_form(&url, &form)).await?;
    Ok(c.apikey.filter(|k| c.activated && !k.is_empty()))
}

/// `POST v4/magnet/upload` (`magnets[]`): the magnet's id.
pub(crate) async fn add_magnet(http: &Http, key: &str, magnet: &str) -> Result<String, Fault> {
    #[derive(Deserialize)]
    struct Uploaded {
        #[serde(default)]
        magnets: Vec<One>,
    }
    #[derive(Deserialize)]
    struct One {
        id: Option<i64>,
        error: Option<ErrorBody>,
    }
    let url = http.services.ad.join("v4/magnet/upload").expect("relative");
    let u: Uploaded = call(
        http,
        http.post_form(&url, &[("magnets[]", magnet)])
            .bearer_auth(key),
    )
    .await?;
    match u.magnets.into_iter().next() {
        Some(One { id: Some(id), .. }) => Ok(id.to_string()),
        Some(One { error: Some(e), .. }) => Err(fault_of(&e.code, &e.message)),
        _ => Err(Fault::Service(mdm_engine::ServiceFault::ServiceDown)),
    }
}

/// `POST v4.1/magnet/status` (`id`), then, once ready, `POST v4/magnet/files` (`id[]`).
pub(crate) async fn magnet_status(
    http: &Http,
    key: &str,
    id: &str,
) -> Result<crate::torrents::Status, Fault> {
    use crate::torrents::{AdMagnet, AdNode, Status};
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        Many(Vec<AdMagnet>),
        One(AdMagnet),
    }
    #[derive(Deserialize)]
    struct Magnets {
        magnets: OneOrMany,
    }
    let id = crate::plain_id(id)?;
    let url = http
        .services
        .ad
        .join("v4.1/magnet/status")
        .expect("relative");
    let m: Magnets = call(http, http.post_form(&url, &[("id", id)]).bearer_auth(key)).await?;
    let m = match m.magnets {
        OneOrMany::One(m) => m,
        OneOrMany::Many(v) => v
            .into_iter()
            .next()
            .ok_or(Fault::Service(mdm_engine::ServiceFault::FileUnavailable))?,
    };
    if let Some(s) = crate::torrents::ad_status(&m) {
        return Ok(s);
    }
    #[derive(Deserialize)]
    struct Files {
        #[serde(default)]
        magnets: Vec<FilesOf>,
    }
    #[derive(Deserialize)]
    struct FilesOf {
        #[serde(default)]
        files: Vec<AdNode>,
        error: Option<ErrorBody>,
    }
    let url = http.services.ad.join("v4/magnet/files").expect("relative");
    let f: Files = call(http, http.post_form(&url, &[("id[]", id)]).bearer_auth(key)).await?;
    let Some(of) = f.magnets.into_iter().next() else {
        return Err(Fault::Service(mdm_engine::ServiceFault::FileUnavailable));
    };
    if let Some(e) = of.error {
        return Err(fault_of(&e.code, &e.message));
    }
    let mut files = Vec::new();
    crate::torrents::ad_flatten(&of.files, "", &mut files);
    Ok(Status::Ready {
        name: m.filename,
        files,
    })
}

/// `POST v4/magnet/delete` (`id`).
pub(crate) async fn magnet_delete(http: &Http, key: &str, id: &str) -> Result<(), Fault> {
    let id = crate::plain_id(id)?;
    let url = http.services.ad.join("v4/magnet/delete").expect("relative");
    call::<serde_json::Value>(http, http.post_form(&url, &[("id", id)]).bearer_auth(key))
        .await
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mdm_engine::ServiceFault as F;

    #[test]
    fn the_documented_envelopes_read() {
        // The documentation's examples.
        let ok = br#"{"status":"success","data":{"link":"http://ombfyx.debrid.it/dl/abcdefgh12/somefile.txt",
            "host":"example","filename":"somefile.txt","paws":false,"filesize":699400192,"streams":[],
            "id":"abcdefgh12","hostDomain":"example.com"}}"#;
        let u: Unlock = read(200, ok, None).unwrap();
        assert_eq!(
            (u.filename.as_deref(), u.filesize, u.delayed),
            (Some("somefile.txt"), 699400192, None)
        );
        let d: Delayed = read(
            200,
            br#"{"status":"success","data":{"status":1,"time_left":45}}"#,
            None,
        )
        .unwrap();
        assert_eq!((d.status, d.time_left, d.link), (1, 45, None));
        let err = br#"{"status":"error","error":{"code":"LINK_DOWN","message":"This link is not available"}}"#;
        assert_eq!(
            read::<Unlock>(200, err, None).unwrap_err(),
            Fault::Service(F::FileUnavailable)
        );
    }

    #[test]
    fn documented_codes_mean_what_they_say() {
        assert_eq!(fault_of("AUTH_BAD_APIKEY", ""), Fault::Service(F::SignIn));
        assert_eq!(
            fault_of("AUTH_BLOCKED", ""),
            Fault::Service(F::NeedsApproval)
        );
        assert_eq!(fault_of("LINK_HOST_NOT_SUPPORTED", ""), Fault::Unsupported);
        assert_eq!(
            fault_of("LINK_HOST_LIMIT_REACHED", ""),
            Fault::Service(F::LimitReached)
        );
        assert_eq!(
            fault_of("LINK_PASS_PROTECTED", ""),
            Fault::Service(F::PasswordNeeded)
        );
        assert_eq!(fault_of("NO_SERVER", ""), Fault::Service(F::IpRefused));
        assert_eq!(
            fault_of("MUST_BE_PREMIUM", ""),
            Fault::Service(F::NotPremium)
        );
        assert_eq!(
            fault_of("SOMETHING_NEW", "Its words"),
            Fault::Service(F::Other("Its words".into()))
        );
        assert!(matches!(
            read::<Unlock>(429, b"", Some(10)).unwrap_err(),
            Fault::Service(F::TooManyRequests(Some(_)))
        ));
    }
}
