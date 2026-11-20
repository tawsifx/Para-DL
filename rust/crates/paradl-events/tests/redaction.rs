//! Golden redaction tests derived from `redaction.json`.
//!
//! The emitter must never leak Bearer tokens, `token=`/`token:` values,
//! environment credentials, `Authorization` / `Proxy-Authorization` headers,
//! cookies, passwords, or nested secrets — including nested objects and lists.

use std::collections::BTreeMap;

use paradl_events::{Redactor, emit_jsonl, emit_jsonl_with_secrets};
use serde::Serialize;
use serde_json::Value;

const REDACTION_FIXTURE: &str = include_str!("../../paradl-oracle/fixtures/redaction.json");

#[derive(Debug, serde::Deserialize)]
struct Fixture {
    redact_secrets: BTreeMap<String, String>,
    redact_obj: Value,
    redact_env_token: String,
    redact_event_line: String,
}

fn fixture() -> Fixture {
    serde_json::from_str(REDACTION_FIXTURE).expect("redaction fixture parses")
}

#[test]
fn string_redaction_matches_golden_map() {
    let golden = fixture();
    // The golden injects a registered secret for the env-token cases. We pass
    // the same literal explicitly so the test is deterministic and env-independent.
    let secrets: Vec<String> = vec!["abcdefghijklmnop".to_string(), "envtok123456".to_string()];
    let redactor = Redactor::with_secrets(&secrets);
    for (input, expected) in &golden.redact_secrets {
        assert_eq!(&redactor.redact_text(input), expected, "input: {input}");
    }
}

#[test]
fn env_token_redaction_matches_golden() {
    let golden = fixture();
    let token = "envtok123456";
    let input = golden.redact_env_token.replace("***REDACTED***", token);
    let redactor = Redactor::with_secrets(&[token.to_string()]);
    assert_eq!(redactor.redact_text(&input), golden.redact_env_token);
}

#[derive(Debug, Serialize)]
struct RedactObjFixture<'a> {
    event: &'a str,
    hf_token: &'a str,
    #[serde(rename = "Authorization")]
    authorization: &'a str,
    password: &'a str,
    safe: &'a str,
    nested: Nested<'a>,
    list: Vec<ListItem<'a>>,
}

#[derive(Debug, Serialize)]
struct Nested<'a> {
    github_token: &'a str,
    url: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ListItem<'a> {
    Obj { secret: &'a str },
    Text(&'a str),
}

#[test]
fn object_redaction_matches_golden_deep_and_byte_exact() {
    let golden = fixture();
    let fixture_obj = RedactObjFixture {
        event: "auth_status",
        hf_token: "hf_secret_value",
        authorization: "Bearer abc123SECRETtoken",
        password: "hunter2secret",
        safe: "hello",
        nested: Nested {
            github_token: "ghp_supersecret",
            url: "https://example.com",
        },
        list: vec![
            ListItem::Obj {
                secret: "nested_secret",
            },
            ListItem::Text("ok"),
        ],
    };

    let line = emit_jsonl(&fixture_obj);
    let expected_line = "{\"event\": \"auth_status\", \"hf_token\": \"***REDACTED***\", \"Authorization\": \"***REDACTED***\", \"password\": \"***REDACTED***\", \"safe\": \"hello\", \"nested\": {\"github_token\": \"***REDACTED***\", \"url\": \"https://example.com\"}, \"list\": [{\"secret\": \"***REDACTED***\"}, \"ok\"]}\n";
    assert_eq!(line, expected_line);

    let emitted: Value = serde_json::from_str(line.trim_end()).unwrap();
    assert_eq!(emitted, golden.redact_obj);
}

#[derive(Debug, Serialize)]
struct ResultLineFixture<'a> {
    event: &'a str,
    timestamp: f64,
    target: &'a str,
    authorization: &'a str,
}

#[test]
fn event_line_redaction_is_byte_exact() {
    let golden = fixture();
    let fixture_line = ResultLineFixture {
        event: "result",
        timestamp: 1763376000.123,
        target: "https://example.com/f",
        authorization: "Bearer abc123SECRETtoken.XYZ-_+/==",
    };
    assert_eq!(emit_jsonl(&fixture_line), golden.redact_event_line);
}

#[test]
fn registered_secrets_are_redacted_from_every_line() {
    paradl_events::register_secret("registered-secret-999");
    #[derive(Serialize)]
    struct Line<'a> {
        event: &'a str,
        detail: &'a str,
    }
    let line = emit_jsonl(&Line {
        event: "progress",
        detail: "using registered-secret-999 now",
    });
    assert!(!line.contains("registered-secret-999"));
    assert!(line.contains("***REDACTED***"));

    // Explicit secrets work without touching the global registry.
    let line = emit_jsonl_with_secrets(
        &Line {
            event: "progress",
            detail: "using explicit-secret-888 now",
        },
        &["explicit-secret-888".to_string()],
    );
    assert!(!line.contains("explicit-secret-888"));
}

#[test]
fn cross_host_cookies_and_proxy_authorization_never_leak() {
    #[derive(Serialize)]
    struct Headers<'a> {
        event: &'a str,
        #[serde(rename = "Cookie")]
        cookie: &'a str,
        #[serde(rename = "Proxy-Authorization")]
        proxy_authorization: &'a str,
        nested: HeadersNested<'a>,
    }
    #[derive(Serialize)]
    struct HeadersNested<'a> {
        #[serde(rename = "Set-Cookie")]
        set_cookie: &'a str,
        host: &'a str,
    }
    let line = emit_jsonl(&Headers {
        event: "info",
        cookie: "session=abc123def456; host=other.example",
        proxy_authorization: "Bearer proxysecretvalue123",
        nested: HeadersNested {
            set_cookie: "sid=supersecretcookie; Path=/",
            host: "safe.example",
        },
    });
    assert!(!line.contains("abc123def456"));
    assert!(!line.contains("proxysecretvalue123"));
    assert!(!line.contains("supersecretcookie"));
    assert!(line.contains("safe.example"));
    assert_eq!(line.matches("***REDACTED***").count(), 3);
}
