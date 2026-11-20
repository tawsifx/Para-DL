//! Authentication token lookup and keychain access.
//!
//! Port of `src/paradl/auth/manager.py`. The lookup order is strict:
//! **environment → platform keychain → none**. Tokens are never placed in
//! process argv, command logs, or JSONL events. Keychain writes use
//! `security -i` with the secret on **stdin**; reads use `security
//! find-generic-password ... -w` whose argv carries only non-secret labels.
//! Any secret read or written is registered with `paradl_events` so it is
//! redacted from every subsequent event line.
//!
//! ```
//! use std::collections::HashMap;
//! use paradl_resilience::auth::{EnvLookup, MapEnv, InMemorySecretStore, Service, TokenSource, lookup_token};
//! let env = MapEnv::new([("GITHUB_TOKEN".to_string(), "ghp_example_1234".to_string())]);
//! let found = lookup_token(Service::Github, &env, &InMemorySecretStore::new()).unwrap();
//! assert_eq!(found.source, TokenSource::Env);
//! ```

use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use paradl_core::{Error, is_windows};
use paradl_events::{AuthStatusEvent, Event};
use serde_json::{Map, Value, json};

use crate::{Emitted, Result, now};

/// Supported credential services.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Service {
    /// Hugging Face Hub.
    Hf,
    /// GitHub API.
    Github,
}

impl Service {
    /// Canonical lowercase service name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hf => "hf",
            Self::Github => "github",
        }
    }

    /// Human-readable label used in status output.
    pub fn label(self) -> &'static str {
        match self {
            Self::Hf => "Hugging Face Hub",
            Self::Github => "GitHub API",
        }
    }

    /// Environment variables consulted in priority order.
    pub fn env_keys(self) -> &'static [&'static str] {
        match self {
            Self::Hf => &["HF_TOKEN", "HUGGING_FACE_HUB_TOKEN"],
            Self::Github => &["GITHUB_TOKEN", "GH_TOKEN"],
        }
    }

    /// Parse a CLI service name, case-insensitively.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "hf" => Some(Self::Hf),
            "github" => Some(Self::Github),
            _ => None,
        }
    }
}

/// Where a token came from. Never carries the token itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenSource {
    /// Explicit environment variable.
    Env,
    /// macOS Login Keychain / Windows Credential Store.
    Keychain,
    /// Hugging Face cache file.
    CacheFile,
    /// GitHub CLI credential.
    GhCli,
    /// No credential configured.
    None,
}

impl TokenSource {
    /// Platform-specific keychain source name used in status output.
    pub fn platform_name() -> &'static str {
        if is_windows() {
            "credential_manager"
        } else {
            "keychain"
        }
    }
}

/// Environment lookup abstraction, injectable for tests.
pub trait EnvLookup {
    /// Return the value of `key`, if set.
    fn get(&self, key: &str) -> Option<String>;
}

/// Production environment lookup.
#[derive(Debug, Default, Clone, Copy)]
pub struct OsEnv;

impl EnvLookup for OsEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// Map-backed environment lookup for tests and replay.
#[derive(Debug, Default, Clone)]
pub struct MapEnv(HashMap<String, String>);

impl MapEnv {
    /// Construct from key/value pairs.
    pub fn new<I: IntoIterator<Item = (String, String)>>(entries: I) -> Self {
        Self(entries.into_iter().collect())
    }
}

impl EnvLookup for MapEnv {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
}

/// Platform secret store abstraction.
pub trait SecretStore {
    /// Load a secret, if present.
    fn load(&self, service: Service, account: &str) -> Result<Option<String>>;
    /// Store a secret. Returns `true` on success.
    fn store(&self, service: Service, account: &str, token: &str) -> Result<bool>;
    /// Delete a secret. Returns `true` when something was removed.
    fn delete(&self, service: Service, account: &str) -> Result<bool>;
}

/// In-memory secret store for tests and non-platform builds.
#[derive(Debug, Default)]
pub struct InMemorySecretStore {
    secrets: Mutex<HashMap<(Service, String), String>>,
}

impl InMemorySecretStore {
    /// Construct an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a token directly (test helper).
    pub fn put(&self, service: Service, account: &str, token: &str) -> Result<()> {
        self.secrets
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert((service, account.to_string()), token.to_string());
        Ok(())
    }
}

impl SecretStore for InMemorySecretStore {
    fn load(&self, service: Service, account: &str) -> Result<Option<String>> {
        Ok(self
            .secrets
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&(service, account.to_string()))
            .cloned())
    }

    fn store(&self, service: Service, account: &str, token: &str) -> Result<bool> {
        self.put(service, account, token)?;
        Ok(true)
    }

    fn delete(&self, service: Service, account: &str) -> Result<bool> {
        Ok(self
            .secrets
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&(service, account.to_string()))
            .is_some())
    }
}

/// A resolved token together with its source (never logged directly).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRecord {
    /// The secret value; callers must register it before emitting events.
    pub token: String,
    /// Where the token was found.
    pub source: TokenSource,
}

/// Strict env → keychain → none lookup.
pub fn lookup_token(service: Service, env: &dyn EnvLookup, store: &dyn SecretStore) -> Option<TokenRecord> {
    for key in service.env_keys() {
        if let Some(value) = env.get(key)
            && value.len() >= 4
        {
            paradl_events::register_secret(value.clone());
            return Some(TokenRecord {
                token: value,
                source: TokenSource::Env,
            });
        }
    }
    let token = store.load(service, service.as_str()).ok().flatten()?;
    if token.len() < 4 {
        return None;
    }
    paradl_events::register_secret(token.clone());
    Some(TokenRecord {
        token,
        source: TokenSource::Keychain,
    })
}

/// Command specification for a keychain write; the secret lives only on stdin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeychainCommandSpec {
    /// argv passed to `Command`; contains no secret.
    pub argv: Vec<String>,
    /// Bytes written to stdin; contains the secret.
    pub stdin_payload: String,
}

/// Build the `security -i` write spec (no token in argv).
pub fn keychain_store_spec(service: &str, account: &str, token: &str) -> KeychainCommandSpec {
    KeychainCommandSpec {
        argv: vec!["security".to_string(), "-i".to_string()],
        stdin_payload: format!(
            "add-generic-password -a {} -s {} -U -w {}\n",
            account, service, token
        ),
    }
}

/// Build the `security find-generic-password` argv (no secret in argv).
pub fn keychain_load_spec(service: &str, account: &str) -> Vec<String> {
    vec![
        "security".to_string(),
        "find-generic-password".to_string(),
        "-a".to_string(),
        account.to_string(),
        "-s".to_string(),
        service.to_string(),
        "-w".to_string(),
    ]
}

/// Build the `security delete-generic-password` argv (no secret in argv).
pub fn keychain_delete_spec(service: &str, account: &str) -> Vec<String> {
    vec![
        "security".to_string(),
        "delete-generic-password".to_string(),
        "-a".to_string(),
        account.to_string(),
        "-s".to_string(),
        service.to_string(),
    ]
}

/// macOS/Windows keychain implementation using stdin for secret writes.
#[derive(Debug, Default, Clone, Copy)]
pub struct KeychainSecretStore;

impl SecretStore for KeychainSecretStore {
    fn load(&self, service: Service, account: &str) -> Result<Option<String>> {
        let argv = keychain_load_spec("pdl", account);
        let output = Command::new(&argv[0])
            .args(&argv[1..])
            .stderr(Stdio::null())
            .output()
            .map_err(Error::from)?;
        if !output.status.success() {
            return Ok(None);
        }
        let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if token.is_empty() {
            return Ok(None);
        }
        paradl_events::register_secret(token.clone());
        let _ = service;
        Ok(Some(token))
    }

    fn store(&self, service: Service, account: &str, token: &str) -> Result<bool> {
        let spec = keychain_store_spec("pdl", account, token);
        let mut child = Command::new(&spec.argv[0])
            .args(&spec.argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(Error::from)?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(spec.stdin_payload.as_bytes()).map_err(Error::from)?;
        }
        let status = child.wait().map_err(Error::from)?;
        paradl_events::register_secret(token);
        let _ = service;
        Ok(status.success())
    }

    fn delete(&self, service: Service, account: &str) -> Result<bool> {
        let argv = keychain_delete_spec("pdl", account);
        let status = Command::new(&argv[0])
            .args(&argv[1..])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(Error::from)?;
        let _ = service;
        Ok(status.success())
    }
}

/// Build `auth_status` metadata and its typed event (never includes tokens).
pub fn auth_status(env: &dyn EnvLookup, store: &dyn SecretStore) -> Result<(Value, Emitted)> {
    let mut services = Map::new();
    for service in [Service::Hf, Service::Github] {
        // Re-derive the source label without retaining the secret value.
        let source_name = if service
            .env_keys()
            .iter()
            .any(|key| env.get(key).is_some_and(|value| value.len() >= 4))
        {
            "env".to_string()
        } else if store.load(service, service.as_str()).ok().flatten().is_some() {
            TokenSource::platform_name().to_string()
        } else {
            "none".to_string()
        };
        let mut entry = Map::new();
        entry.insert("service".into(), json!(service.as_str()));
        entry.insert("label".into(), json!(service.label()));
        entry.insert("configured".into(), json!(source_name != "none"));
        entry.insert("source".into(), json!(source_name));
        services.insert(service.as_str().to_string(), Value::Object(entry));
    }
    let value = Value::Object(services);
    let emitted = Emitted::new(Event::AuthStatus(AuthStatusEvent::new(
        now(),
        value.clone(),
    )));
    Ok((value, emitted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_parsing_and_env_keys() {
        assert_eq!(Service::parse(" HF "), Some(Service::Hf));
        assert_eq!(Service::parse("github"), Some(Service::Github));
        assert_eq!(Service::parse("other"), None);
        assert_eq!(Service::Hf.env_keys()[0], "HF_TOKEN");
        assert_eq!(Service::Github.env_keys()[0], "GITHUB_TOKEN");
    }
}
