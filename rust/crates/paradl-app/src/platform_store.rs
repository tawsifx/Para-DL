//! Thin adapters connecting [`paradl_platform`] to the resilience auth stack.
//!
//! This module is the only place where the two secret-store abstractions meet:
//! [`PlatformSecretStore`] implements the resilience [`SecretStore`] trait over
//! a [`Platform`], and [`apply_origin_metadata`] is the best-effort seam used by
//! the transfer path. Keeping both here makes them easy to move during the C2
//! `app.rs` split — callers only reference these symbols by name.

use std::path::Path;

use paradl_platform::Platform;
use paradl_resilience::Result;
use paradl_resilience::auth::{SecretStore, Service};

/// Adapts the host [`Platform`] to the resilience [`SecretStore`] interface.
///
/// The resilience lookup keeps its strict env → keychain → none chain and its
/// `source` metadata; this adapter only supplies the keychain leg. It uses the
/// same `"pdl"` service label as the former direct platform call and takes the
/// account from the `Service::as_str()` value that `lookup_token` passes in.
pub(crate) struct PlatformSecretStore(&'static dyn Platform);

impl PlatformSecretStore {
    /// Wrap the selected platform for use as a resilience [`SecretStore`].
    pub(crate) fn new(platform: &'static dyn Platform) -> Self {
        Self(platform)
    }
}

impl SecretStore for PlatformSecretStore {
    fn load(&self, _service: Service, account: &str) -> Result<Option<String>> {
        self.0.load_secret("pdl", account)
    }

    fn store(&self, _service: Service, account: &str, token: &str) -> Result<bool> {
        self.0.store_secret("pdl", account, token).map(|()| true)
    }

    fn delete(&self, _service: Service, account: &str) -> Result<bool> {
        // Preserve the pre-existing Remove semantics — an empty write, with no
        // new trait method and no dependence on platform delete support.
        self.0.store_secret("pdl", account, "").map(|()| true)
    }
}

/// Best-effort origin-metadata write after a successful transfer.
///
/// Production passes [`paradl_platform::current()`] and discards the result, so
/// this seam emits no event and can never change a command's exit code.
pub(crate) fn apply_origin_metadata(
    platform: &dyn Platform,
    dest: &Path,
    urls: &[String],
) -> paradl_platform::Result<()> {
    platform.set_origin_metadata(dest, urls)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use paradl_platform::Platform;
    use paradl_resilience::auth::{MapEnv, SecretStore, Service, TokenSource, lookup_token};

    use super::{PlatformSecretStore, apply_origin_metadata};

    /// Recording [`Platform`] used to observe the adapter's exact calls.
    #[derive(Clone, Default)]
    struct SpyPlatform {
        origin_calls: Arc<Mutex<Vec<(PathBuf, Vec<String>)>>>,
        secrets: Arc<Mutex<HashMap<(String, String), String>>>,
    }

    impl Platform for SpyPlatform {
        fn set_origin_metadata(&self, path: &Path, urls: &[String]) -> paradl_platform::Result<()> {
            self.origin_calls
                .lock()
                .unwrap()
                .push((path.to_path_buf(), urls.to_vec()));
            Ok(())
        }

        fn store_secret(
            &self,
            service: &str,
            account: &str,
            secret: &str,
        ) -> paradl_platform::Result<()> {
            self.secrets.lock().unwrap().insert(
                (service.to_string(), account.to_string()),
                secret.to_string(),
            );
            Ok(())
        }

        fn load_secret(
            &self,
            service: &str,
            account: &str,
        ) -> paradl_platform::Result<Option<String>> {
            Ok(self
                .secrets
                .lock()
                .unwrap()
                .get(&(service.to_string(), account.to_string()))
                .cloned())
        }
    }

    /// Platform whose metadata write fails, to prove the seam propagates errors
    /// so the production call site can ignore them without changing exit codes.
    #[derive(Default)]
    struct FailingPlatform;

    impl Platform for FailingPlatform {
        fn set_origin_metadata(
            &self,
            _path: &Path,
            _urls: &[String],
        ) -> paradl_platform::Result<()> {
            Err(paradl_core::Error::failure("synthetic metadata failure"))
        }

        fn store_secret(
            &self,
            _service: &str,
            _account: &str,
            _secret: &str,
        ) -> paradl_platform::Result<()> {
            Ok(())
        }

        fn load_secret(
            &self,
            _service: &str,
            _account: &str,
        ) -> paradl_platform::Result<Option<String>> {
            Ok(None)
        }
    }

    fn leaked_platform(spy: &SpyPlatform) -> &'static dyn Platform {
        Box::leak(Box::new(spy.clone()))
    }

    #[test]
    fn adapter_maps_service_account_to_pdl_platform_labels() {
        let spy = SpyPlatform::default();
        let store = PlatformSecretStore(leaked_platform(&spy));

        assert!(store.store(Service::Hf, "hf", "token-value-1234").unwrap());
        assert_eq!(
            spy.secrets
                .lock()
                .unwrap()
                .get(&("pdl".to_string(), "hf".to_string()))
                .map(String::as_str),
            Some("token-value-1234")
        );
        assert_eq!(
            store.load(Service::Hf, "hf").unwrap().as_deref(),
            Some("token-value-1234")
        );

        // Remove keeps the pre-existing empty-write semantics: no new trait method.
        assert!(store.delete(Service::Hf, "hf").unwrap());
        assert_eq!(
            spy.secrets
                .lock()
                .unwrap()
                .get(&("pdl".to_string(), "hf".to_string()))
                .map(String::as_str),
            Some("")
        );
    }

    #[test]
    fn env_precedence_over_platform_keychain_is_preserved() {
        let spy = SpyPlatform::default();
        let store = PlatformSecretStore(leaked_platform(&spy));
        store
            .store(Service::Hf, "hf", "keychain-token-1234")
            .unwrap();

        let with_env = MapEnv::new([("HF_TOKEN".to_string(), "env-token-1234".to_string())]);
        let record = lookup_token(Service::Hf, &with_env, &store).expect("env token wins");
        assert_eq!(record.source, TokenSource::Env);
        assert_eq!(record.token, "env-token-1234");

        let without_env = MapEnv::new([]);
        let record = lookup_token(Service::Hf, &without_env, &store).expect("keychain fallback");
        assert_eq!(record.source, TokenSource::Keychain);
        assert_eq!(record.token, "keychain-token-1234");

        let empty = PlatformSecretStore(leaked_platform(&SpyPlatform::default()));
        assert!(lookup_token(Service::Hf, &without_env, &empty).is_none());
    }

    #[test]
    fn apply_origin_metadata_records_destination_and_target_once() {
        let spy = SpyPlatform::default();
        let dest = Path::new("/tmp/c1-destination/file.zip");
        let urls = ["https://example.com/f.zip".to_string()];

        apply_origin_metadata(leaked_platform(&spy), dest, &urls).unwrap();

        let calls = spy.origin_calls.lock().unwrap();
        assert_eq!(calls.len(), 1, "exactly one metadata write");
        assert_eq!(calls[0].0, dest);
        assert_eq!(calls[0].1, urls);
    }

    #[test]
    fn apply_origin_metadata_propagates_platform_errors() {
        let failing: &'static dyn Platform = Box::leak(Box::new(FailingPlatform));
        let error = apply_origin_metadata(failing, Path::new("/tmp/file.zip"), &[]).unwrap_err();
        assert!(error.to_string().contains("synthetic metadata failure"));
    }
}
