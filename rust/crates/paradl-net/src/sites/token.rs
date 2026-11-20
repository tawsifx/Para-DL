//! Token flows for site engines (env → keychain → none).
//!
//! No secret is ever placed in a command line. The [`Keychain`] trait mirrors
//! `paradl-platform::Platform::load_secret`; a W2-2/W2-4 implementation feeds
//! the secret over stdin (`security -i`), so argv stays clean (charter row L11).
//! [`EnvSecretProvider::register_secret`] is the hook the W2-4 redaction layer
//! uses to register tokens before any JSONL/event emission.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// The two token-bearing site services.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SiteService {
    /// GitHub (`gh:` releases, trees, archives).
    Github,
    /// Hugging Face (`hf:` models and datasets).
    Hf,
}

impl SiteService {
    /// Environment variables consulted in priority order.
    pub const fn env_vars(self) -> &'static [&'static str] {
        match self {
            Self::Github => &["GITHUB_TOKEN", "GH_TOKEN"],
            Self::Hf => &["HF_TOKEN", "HUGGING_FACE_HUB_TOKEN"],
        }
    }

    /// Keychain account name (`github` / `hf`).
    pub const fn keychain_account(self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Hf => "hf",
        }
    }

    /// Keychain service name (always `pdl`, charter row L35).
    pub const fn keychain_service(self) -> &'static str {
        "pdl"
    }

    /// Site-specific User-Agent, byte-identical to the Python engines.
    pub const fn user_agent(self) -> &'static str {
        match self {
            Self::Github => "pdl-accelerator/1.0.0",
            Self::Hf => "pdl-hf-accelerator/1.0.0",
        }
    }
}

/// A read-only keychain abstraction for site tokens.
pub trait Keychain: Send + Sync {
    /// Load a secret for `(service, account)`, returning `None` when absent.
    fn load_secret(&self, service: &str, account: &str) -> Option<String>;
}

/// Supplies a bearer token for a site service.
pub trait SecretProvider: Send + Sync {
    /// Return the token for `service`, registering it for redaction.
    fn token(&self, service: SiteService) -> Option<String>;

    /// Register a secret with the redaction layer (no-op by default for custom
    /// providers; [`EnvSecretProvider`] records it for verification).
    fn register_secret(&self, secret: &str);
}

/// Env-first, keychain-second [`SecretProvider`] matching the Python engines.
///
/// Precedence: `GITHUB_TOKEN` → `GH_TOKEN` → keychain for GitHub, and
/// `HF_TOKEN` → `HUGGING_FACE_HUB_TOKEN` → keychain for Hugging Face.
#[derive(Default)]
pub struct EnvSecretProvider {
    env: HashMap<String, String>,
    keychain: Option<Arc<dyn Keychain>>,
    registered: Mutex<Vec<String>>,
}

impl EnvSecretProvider {
    /// Snapshot the current process environment (no keychain attached).
    pub fn from_process() -> Self {
        Self {
            env: std::env::vars().collect(),
            keychain: None,
            registered: Mutex::new(Vec::new()),
        }
    }

    /// Build from an explicit environment map, for deterministic tests and
    /// embedders that do not want to read process-global state.
    pub fn with_env(
        env: HashMap<String, String>,
        keychain: Option<Arc<dyn Keychain>>,
    ) -> Self {
        Self {
            env,
            keychain,
            registered: Mutex::new(Vec::new()),
        }
    }

    /// Attach a keychain provider (builder style).
    pub fn with_keychain(mut self, keychain: Arc<dyn Keychain>) -> Self {
        self.keychain = Some(keychain);
        self
    }

    /// Secrets registered for redaction so far, in first-seen order.
    pub fn registered_secrets(&self) -> Vec<String> {
        self.registered
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl SecretProvider for EnvSecretProvider {
    fn token(&self, service: SiteService) -> Option<String> {
        let mut token = None;
        for name in service.env_vars() {
            if let Some(value) = self.env.get(*name) {
                let trimmed = value.trim();
                if !trimmed.is_empty() {
                    token = Some(trimmed.to_owned());
                    break;
                }
            }
        }
        if token.is_none()
            && let Some(keychain) = &self.keychain
        {
            token = keychain
                .load_secret(service.keychain_service(), service.keychain_account())
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty());
        }
        if let Some(found) = &token {
            self.register_secret(found);
        }
        token
    }

    fn register_secret(&self, secret: &str) {
        let mut registered = self.registered.lock().unwrap_or_else(|e| e.into_inner());
        if !registered.iter().any(|value| value == secret) {
            registered.push(secret.to_owned());
        }
    }
}

/// Header construction for site REST calls and asset downloads.
pub struct SiteHeaders;

impl SiteHeaders {
    /// Headers for a site's JSON REST API (includes `Accept` on GitHub).
    pub fn rest(service: SiteService, secrets: &dyn SecretProvider) -> HashMap<String, String> {
        let mut headers = Self::download(service, secrets);
        if service == SiteService::Github {
            headers.insert(
                "Accept".to_owned(),
                "application/vnd.github.v3+json".to_owned(),
            );
        }
        headers
    }

    /// Headers for downloading a resolved asset (User-Agent + optional Bearer).
    pub fn download(service: SiteService, secrets: &dyn SecretProvider) -> HashMap<String, String> {
        let mut headers = HashMap::new();
        headers.insert("User-Agent".to_owned(), service.user_agent().to_owned());
        if let Some(token) = secrets.token(service) {
            headers.insert("Authorization".to_owned(), format!("Bearer {token}"));
        }
        headers
    }
}
