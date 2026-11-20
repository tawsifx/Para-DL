//! # paradl-postproc
//!
//! Post-processing for paraDL: cryptographic checksums, **security-validated**
//! ZIP/tar extraction, and ADB hand-off. 7z/DMG/hdiutil stay external processes.
//!
//! The public [`PostProcessor`] trait is the authoritative interface (framework
//! charter section 2). A verification mismatch maps to
//! [`ExitCode::VerificationFailed`] (`3`) via [`paradl_core::Error`].
//!
//! ```
//! use paradl_postproc::{Algorithm, DefaultPostProcessor, Digest, PostProcessor};
//!
//! let dir = std::env::temp_dir().join(format!("paradl-postproc-doc-{}", std::process::id()));
//! std::fs::create_dir_all(&dir).unwrap();
//! let path = dir.join("abc.bin");
//! std::fs::write(&path, b"abc").unwrap();
//! let processor = DefaultPostProcessor;
//! let bad = Digest { algorithm: Algorithm::Sha256, hex: "00".repeat(32) };
//! let err = processor.verify(&path, Some(&bad)).unwrap_err();
//! assert_eq!(err.exit_code(), paradl_core::ExitCode::VerificationFailed);
//! std::fs::remove_dir_all(&dir).unwrap();
//! ```
//!
//! [`ExitCode::VerificationFailed`]: paradl_core::ExitCode::VerificationFailed

#![warn(missing_docs)]

pub mod adb;
pub mod archive;
pub mod checksum;

use std::path::{Path, PathBuf};

use paradl_core::Error;

pub use checksum::{Algorithm, Digest};

/// Convenience result alias for all post-processing operations.
pub type Result<T> = std::result::Result<T, Error>;

/// Where an expected checksum came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumSource {
    /// Caller supplied the digest explicitly (`--checksum`).
    Explicit,
    /// Discovered from a companion file (`--verify`).
    Companion,
    /// No checksum was available.
    None,
}

/// Outcome of a checksum verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verification {
    /// Whether the computed digest matched the expected one.
    pub verified: bool,
    /// Algorithm used for the comparison.
    pub algorithm: Algorithm,
    /// Expected hex digest, when one was available.
    pub expected: Option<String>,
    /// Computed hex digest.
    pub computed: String,
    /// Provenance of the expected digest.
    pub source: ChecksumSource,
}

/// Inputs to [`PostProcessor::postprocess`].
#[derive(Debug, Clone, Default)]
pub struct PostContext {
    /// Path to the downloaded artifact.
    pub path: PathBuf,
    /// Destination directory for archive extraction.
    pub dest_dir: Option<PathBuf>,
    /// Extract `path` when it is a supported archive.
    pub extract: bool,
    /// Remove the archive after a successful extraction.
    pub remove_archive: bool,
    /// Push the file to an attached Android device via ADB.
    pub adb_push: bool,
    /// ADB destination directory (defaults to `/storage/emulated/0/Music`).
    pub adb_target_dir: Option<String>,
    /// Optional explicit expected digest.
    pub expected_digest: Option<Digest>,
}

impl PostContext {
    /// Build a context for `path` with all optional actions disabled.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            ..Self::default()
        }
    }
}

/// Result of [`PostProcessor::postprocess`].
#[derive(Debug, Clone, Default)]
pub struct PostResult {
    /// Checksum verification, when one was performed.
    pub verification: Option<Verification>,
    /// Whether an archive was extracted.
    pub extracted: bool,
    /// Files produced by archive extraction.
    pub extracted_files: Vec<PathBuf>,
    /// Whether the file was pushed to an ADB device.
    pub adb_synced: bool,
}

/// Byte-exact post-processing operations (charter section 2).
pub trait PostProcessor: Send + Sync {
    /// Verify `path` against `expected`, or a discovered companion checksum.
    ///
    /// Returns [`Error::VerificationFailed`] (exit code `3`) on mismatch.
    fn verify(&self, path: &Path, expected: Option<&Digest>) -> Result<Verification>;

    /// Run the requested checksum / extraction / ADB steps.
    fn postprocess(&self, ctx: &PostContext) -> Result<PostResult>;
}

/// Default [`PostProcessor`] implementation using the crate's own helpers.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultPostProcessor;

impl PostProcessor for DefaultPostProcessor {
    fn verify(&self, path: &Path, expected: Option<&Digest>) -> Result<Verification> {
        let verification = checksum::verify_file(path, expected)?;
        if verification.verified {
            Ok(verification)
        } else {
            Err(Error::verification_failed(format!(
                "checksum mismatch for {}: expected {:?}, computed {:?}",
                path.display(),
                verification.expected,
                verification.computed,
            )))
        }
    }

    fn postprocess(&self, ctx: &PostContext) -> Result<PostResult> {
        let mut result = PostResult::default();
        if ctx.expected_digest.is_some() {
            result.verification = Some(checksum::verify_file(
                &ctx.path,
                ctx.expected_digest.as_ref(),
            )?);
        }
        if ctx.extract {
            let dest = ctx
                .dest_dir
                .clone()
                .unwrap_or_else(|| ctx.path.parent().unwrap_or(Path::new(".")).to_path_buf());
            let (ok, files) = archive::extract(&ctx.path, &dest, ctx.remove_archive)?;
            result.extracted = ok;
            result.extracted_files = files;
        }
        if ctx.adb_push {
            result.adb_synced = adb::push_file(
                &ctx.path,
                ctx.adb_target_dir
                    .as_deref()
                    .unwrap_or(adb::DEFAULT_MUSIC_DIR),
            );
        }
        Ok(result)
    }
}
