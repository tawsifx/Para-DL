//! Compatibility shims for dropped legacy flags.
//!
//! `--http3` stays accepted at the CLI layer for backwards compatibility, but
//! HTTP/3 is not implemented in this crate. This module provides the single
//! log-drop helper and its reason string; no Cargo feature is exposed for an
//! unimplemented capability (framework charter §5).

/// Explanation emitted when the legacy `--http3` flag is seen.
pub const HTTP3_DROP_REASON: &str = "HTTP/3 is not implemented in paradl-net 0.1.0; --http3 is accepted for CLI compatibility and the transport falls back to HTTP/1.1 over rustls TCP.";

/// Acknowledge `--http3` by logging why it is dropped.
///
/// Call this from the CLI when the flag is parsed. It is infallible and never
/// changes transport behavior.
pub fn note_http3_drop() {
    tracing::warn!(
        reason = HTTP3_DROP_REASON,
        "--http3 requested but unsupported; dropping flag"
    );
}
