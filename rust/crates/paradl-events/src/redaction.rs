//! Secret redaction, a faithful port of `src/paradl/ui/display.py:22-54`.
//!
//! Two layers mirror the Python oracle exactly:
//!
//! 1. [`Redactor::redact_text`] rewrites free text: `Bearer <token>`,
//!    `token=`/`token:` values, cookies, and any registered/environment secret
//!    of length >= 4.
//! 2. [`Redactor::is_sensitive_key`] replaces the *whole value* of any mapping
//!    key containing `secret`, `token`, `authorization`, `password`, or
//!    `cookie` with `***REDACTED***`, recursively through nested objects and
//!    arrays (Python `_redact_obj`).
//!
//! The serializer in [`crate::json`] applies both layers at emit time, so no
//! caller can accidentally bypass redaction by logging a raw struct.

use std::sync::OnceLock;

use regex::Regex;

/// Replacement written in place of every detected secret.
pub const REDACTED: &str = "***REDACTED***";

static BEARER_RE: OnceLock<Regex> = OnceLock::new();
static TOKEN_PARAM_RE: OnceLock<Regex> = OnceLock::new();
static COOKIE_RE: OnceLock<Regex> = OnceLock::new();

fn bearer_re() -> &'static Regex {
    BEARER_RE.get_or_init(|| {
        Regex::new(r"(?i)(Bearer\s+)[A-Za-z0-9._\-+/=]+").expect("bearer regex compiles")
    })
}

fn token_param_re() -> &'static Regex {
    TOKEN_PARAM_RE.get_or_init(|| {
        Regex::new(r"(?i)(token[:=]\s*)[A-Za-z0-9._\-+/=]+").expect("token regex compiles")
    })
}

fn cookie_re() -> &'static Regex {
    COOKIE_RE.get_or_init(|| {
        Regex::new(r"(?i)((?:set-)?cookie\s*[:=]\s*)[^\r\n]+").expect("cookie regex compiles")
    })
}

/// Environment variables whose values are always treated as secrets.
///
/// Mirrors `env_keys` in `redact_secrets` (`ui/display.py:28`).
pub const SECRET_ENV_KEYS: [&str; 4] = [
    "HF_TOKEN",
    "HUGGING_FACE_HUB_TOKEN",
    "GITHUB_TOKEN",
    "GH_TOKEN",
];

/// Configurable redactor used by the single JSONL serializer.
///
/// ```
/// use paradl_events::Redactor;
///
/// let r = Redactor::with_secrets(&["my-secret-123".to_string()]);
/// assert_eq!(r.redact_text("Bearer abc123.XYZ"), "Bearer ***REDACTED***");
/// assert_eq!(r.redact_text("token=supersecret"), "token=***REDACTED***");
/// assert_eq!(r.redact_text("my-secret-123"), "***REDACTED***");
/// assert!(Redactor::is_sensitive_key("Proxy-Authorization"));
/// ```
#[derive(Debug, Clone)]
pub struct Redactor {
    env_secrets: Vec<String>,
    extra_secrets: Vec<String>,
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new()
    }
}

impl Redactor {
    /// Build a redactor with no explicitly registered secrets.
    pub fn new() -> Self {
        Self {
            env_secrets: Vec::new(),
            extra_secrets: Vec::new(),
        }
    }

    /// Build a redactor with explicit (registered) secrets in addition to the
    /// process environment secrets.
    pub fn with_secrets(secrets: &[String]) -> Self {
        Self {
            env_secrets: Vec::new(),
            extra_secrets: secrets.to_vec(),
        }
    }

    /// Build a redactor that reads the known credential environment variables.
    pub fn from_env() -> Self {
        let env_secrets = SECRET_ENV_KEYS
            .iter()
            .filter_map(|key| std::env::var(key).ok())
            .filter(|value| value.len() >= 4)
            .collect();
        Self {
            env_secrets,
            extra_secrets: Vec::new(),
        }
    }

    /// Combine environment secrets with a set of registered secrets.
    pub fn from_env_and(secrets: &[String]) -> Self {
        let mut redactor = Self::from_env();
        redactor.extra_secrets.extend_from_slice(secrets);
        redactor
    }

    /// Redact all token-like patterns and known secrets from free text.
    pub fn redact_text(&self, text: &str) -> String {
        if text.is_empty() {
            return String::new();
        }
        let mut out = bearer_re()
            .replace_all(text, "${1}".to_string() + REDACTED)
            .into_owned();
        out = token_param_re()
            .replace_all(&out, "${1}".to_string() + REDACTED)
            .into_owned();
        out = cookie_re()
            .replace_all(&out, "${1}".to_string() + REDACTED)
            .into_owned();
        for secret in self.env_secrets.iter().chain(self.extra_secrets.iter()) {
            if secret.len() >= 4 && out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), REDACTED);
            }
        }
        out
    }

    /// `true` when a mapping key marks its value as secret (case-insensitive).
    ///
    /// `Proxy-Authorization` is covered because it contains `authorization`.
    pub fn is_sensitive_key(key: &str) -> bool {
        let lower = key.to_ascii_lowercase();
        [
            "secret",
            "token",
            "authorization",
            "password",
            "cookie",
            "api_key",
            "apikey",
            "credential",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_bearer_and_token_forms() {
        let r = Redactor::new();
        assert_eq!(r.redact_text("Bearer abc123SECRETtoken.XYZ-_+/=="), "Bearer ***REDACTED***");
        assert_eq!(r.redact_text("token=supersecretvalue123"), "token=***REDACTED***");
        assert_eq!(r.redact_text("token: abcdefghijkl"), "token: ***REDACTED***");
        assert_eq!(r.redact_text("plain text no secrets"), "plain text no secrets");
        assert_eq!(r.redact_text(""), "");
    }

    #[test]
    fn redacts_cookie_headers_and_proxy_authorization() {
        let r = Redactor::new();
        assert_eq!(r.redact_text("Cookie: session=abc123; a=b"), "Cookie: ***REDACTED***");
        assert!(Redactor::is_sensitive_key("Proxy-Authorization"));
        assert!(Redactor::is_sensitive_key("Set-Cookie"));
        assert!(!Redactor::is_sensitive_key("safe"));
    }
}
