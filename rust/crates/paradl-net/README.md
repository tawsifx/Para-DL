# paradl-net

HTTP client, HEAD/Range probing, RFC 6266 filenames, ETag digests, DNS/DoH,
proxy + IP-family selection, pure routing rules, and site-resolver plumbing for
the paraDL framework.

## Public traits

- **`TargetResolver`** (charter §2) — `scheme()` + async `resolve(&str, &ResolveContext) -> Result<Vec<ResolvedAsset>>`.
  Implementations: `HttpResolver`, `SearchResolver`, `MagnetResolver`.
- **`ExtractorClaims`** — injectable check for yt-dlp extractor claims. Rust
  never loads yt-dlp; the W3-4 media bridge supplies the live-registry version
  via the `paradl-media/1` `claims` op. `PureClaimsChecker` is the default.

## Routing split (ruling 3)

Rust replicates only the **pure** rules from the `paradl-core` single-source
constants (`MEDIA_DOMAINS`, `IMAGE_DOMAINS`, `DIRECT_EXTENSIONS`, re-exported
here so there is no drift): hostname-suffix media matching, extension suffix
matching, `ytsearch1:` search, and HEAD/Range `text/html → STREAM`. Anything
requiring yt-dlp's live extractor registry, full metadata, or direct-vs-managed
handoff is returned as `RouteDecision::DelegateToWorker`; `claims=false`
becomes `MediaClaim::DirectFallback`.

## Feature flags

- `doh` (opt-in) — DNS-over-HTTPS via the hickory-dns project's
  `hickory-resolver` client library, used by `--doh-endpoint`. Without it the
  system resolver is used and the endpoint is accepted but ignored.
- No HTTP/3 feature exists. The legacy `--http3` flag is kept compatible by
  `note_http3_drop()`, which logs `HTTP3_DROP_REASON`.

## ISP handoff (W0-3)

`paradl_core::PDLConfig::get_effective_isp()` returns `"auto"` when dynamic
detection is requested. This crate provides `detect_isp()` and
`resolve_effective_isp(configured)`: explicit names pass through, `"auto"` is
resolved by three detection tiers with a five-minute cache. Wiring happens at
integration (W4-1); `paradl-core` is not modified.

## Example

```bash
cargo run -p paradl-net --example resolve_target
cargo run -p paradl-net --example resolve_target --features doh
```

The example is network-free (probing disabled) and prints classifications,
RFC 6266/ETag helpers, ISP passthrough, and DoH endpoint parsing.

## Verification

```bash
cargo test -p paradl-net
cargo test -p paradl-net --features doh
cargo clippy -p paradl-net -- -D warnings
cargo doc -p paradl-net --no-deps
```
