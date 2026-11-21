//! # paradl-platform
//!
//! Byte-exact macOS and Windows origin metadata plus keychain-backed secrets.
//!
//! The public [`Platform`] trait is the authoritative interface (framework charter
//! section 2). Concrete implementations are **feature-gated** so the default build
//! needs no system tools:
//!
//! - `platform-macos` → `MacOSPlatform` (`xattr`, `security -i`, `codesign`)
//! - `platform-windows` → `WindowsPlatform` (`Zone.Identifier` ADS, WinCred/file store)
//!
//! All external tools stay external processes; no Python is ever spawned.
//!
//! ```
//! use std::path::Path;
//! use paradl_platform::{NoopPlatform, Platform};
//!
//! let platform = NoopPlatform;
//! platform.set_origin_metadata(Path::new("/tmp/file"), &["https://example.com".into()]).unwrap();
//! assert_eq!(platform.load_secret("pdl", "hf").unwrap(), None);
//! ```

#![warn(missing_docs)]

use std::path::Path;

use paradl_core::Error;

/// Convenience result alias for all platform operations.
pub type Result<T> = std::result::Result<T, Error>;

/// OS origin-metadata and secret-storage operations (charter section 2).
///
/// Implementations must be safe no-ops when the host OS does not match or the
/// required external tool is unavailable, mirroring the Python oracle's
/// `is_effective_macos()` / `is_effective_windows()` fallbacks.
pub trait Platform: Send + Sync {
    /// Record the download origin URLs on `path`.
    ///
    /// macOS writes a binary plist to `com.apple.metadata:kMDItemWhereFroms`;
    /// Windows writes a `Zone.Identifier` alternate data stream.
    fn set_origin_metadata(&self, path: &Path, urls: &[String]) -> Result<()>;

    /// Store `secret` under `service`/`account`.
    ///
    /// Secrets are never placed in process argv. macOS pipes the command to
    /// `security -i` over stdin; Windows uses CredWriteW or a user-scoped store.
    fn store_secret(&self, service: &str, account: &str, secret: &str) -> Result<()>;

    /// Load a previously stored secret, or `None` when absent/unsupported.
    fn load_secret(&self, service: &str, account: &str) -> Result<Option<String>>;
}

/// A safe no-op [`Platform`] used when no platform feature is enabled.
///
/// It never touches the filesystem or spawns a process.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopPlatform;

impl Platform for NoopPlatform {
    fn set_origin_metadata(&self, _path: &Path, _urls: &[String]) -> Result<()> {
        Ok(())
    }

    fn store_secret(&self, _service: &str, _account: &str, _secret: &str) -> Result<()> {
        Ok(())
    }

    fn load_secret(&self, _service: &str, _account: &str) -> Result<Option<String>> {
        Ok(None)
    }
}

#[cfg(feature = "platform-macos")]
pub mod macos;
#[cfg(feature = "platform-macos")]
pub use macos::MacOSPlatform;

#[cfg(feature = "platform-windows")]
pub mod windows;
#[cfg(feature = "platform-windows")]
pub use windows::WindowsPlatform;

/// Select the active [`Platform`] for the host OS and enabled feature.
///
/// This is the one deliberate trait-object seam in the framework: choosing an
/// implementation at runtime from `#[cfg(all(feature, target_os))]` cannot be
/// expressed with static dispatch without duplicating every call site into
/// `cfg` blocks. All other charter traits stay zero-cost static-dispatch bounds.
///
/// The arms are mutually exclusive: `platform-macos` on macOS yields
/// `MacOSPlatform`, `platform-windows` on Windows yields `WindowsPlatform`,
/// and a disabled feature or a non-matching OS yields the safe [`NoopPlatform`].
#[cfg(all(feature = "platform-macos", target_os = "macos"))]
pub fn current() -> &'static dyn Platform {
    static P: MacOSPlatform = MacOSPlatform { keychain: None };
    &P
}

/// Select the active [`Platform`] for the host OS and enabled feature.
///
/// This is the one deliberate trait-object seam in the framework: choosing an
/// implementation at runtime from `#[cfg(all(feature, target_os))]` cannot be
/// expressed with static dispatch without duplicating every call site into
/// `cfg` blocks. All other charter traits stay zero-cost static-dispatch bounds.
///
/// The arms are mutually exclusive: `platform-macos` on macOS yields
/// `MacOSPlatform`, `platform-windows` on Windows yields `WindowsPlatform`,
/// and a disabled feature or a non-matching OS yields the safe [`NoopPlatform`].
#[cfg(all(feature = "platform-windows", target_os = "windows"))]
pub fn current() -> &'static dyn Platform {
    static P: WindowsPlatform = WindowsPlatform { store_path: None };
    &P
}

/// Select the active [`Platform`] for the host OS and enabled feature.
///
/// This is the one deliberate trait-object seam in the framework: choosing an
/// implementation at runtime from `#[cfg(all(feature, target_os))]` cannot be
/// expressed with static dispatch without duplicating every call site into
/// `cfg` blocks. All other charter traits stay zero-cost static-dispatch bounds.
///
/// The arms are mutually exclusive: `platform-macos` on macOS yields
/// `MacOSPlatform`, `platform-windows` on Windows yields `WindowsPlatform`,
/// and a disabled feature or a non-matching OS yields the safe [`NoopPlatform`].
#[cfg(not(any(
    all(feature = "platform-macos", target_os = "macos"),
    all(feature = "platform-windows", target_os = "windows")
)))]
pub fn current() -> &'static dyn Platform {
    &NoopPlatform
}
