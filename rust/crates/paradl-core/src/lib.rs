//! # paradl-core
//!
//! Shared foundation for the paraDL framework: the single public [`Error`] type
//! and its [`ExitCode`] mapping, byte-exact model helpers, configuration,
//! state paths, and a clock abstraction.
//!
//! All helpers are direct ports of the Python oracle (`src/paradl/core/*.py`)
//! and are verified against its golden values. Domain lists and direct-download
//! extensions are each defined once and reused by the classification helpers.
//!
//! ```
//! use paradl_core::{Error, ExitCode, parse_rate, sanitize_filename};
//!
//! assert_eq!(parse_rate("1M").unwrap(), 1024.0 * 1024.0);
//! assert_eq!(sanitize_filename("CON"), "_CON");
//! assert_eq!(Error::usage("bad flag").exit_code(), ExitCode::Usage);
//! ```
//!
//! [`Clock`]: clock::Clock

#![warn(missing_docs)]

pub mod clock;
pub mod config;
pub mod error;
pub mod models;
pub mod paths;

pub use clock::{Clock, FakeClock, SystemClock};
pub use config::{CONFIG_SCHEMA_VERSION, PDLConfig, get_active_engine, set_active_engine};
pub use error::{Error, ExitCode};
pub use models::{
    DIRECT_EXTENSIONS, DownloadItem, IMAGE_DOMAINS, MEDIA_DOMAINS, RateInput, TargetInfo,
    TargetType, TransferResult, WatchdogMode, direct_extension, format_rate_for_aria2, format_size,
    format_speed, has_direct_extension, is_image_domain, is_media_domain, parse_rate,
    sanitize_filename,
};
pub use paths::{
    config_file, default_bin_dir, get_paradl_home, history_db_file, is_windows,
    legacy_history_file, logs_dir, mirrors_file, targets_file, throttle_dir,
};
