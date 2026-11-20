# paradl-engine

Reusable download engine for paraDL, promoting the `rapids` core into a public,
documented crate. It owns planning, resume state, rate limiting, checksum
verification, the Range probe and a bounded Tokio worker pool.

## Public interface

- `DownloadEngine` — the authoritative trait from the framework charter §2:
  `plan(&Target) -> Plan`, `download(Plan, &mut dyn ProgressSink) -> TransferResult`,
  `capabilities() -> EngineCapabilities`.
- `NdEngine` — the stateless native implementation (clone it freely).
- Supporting types: `Target`, `Plan`, `ProgressSink`, `NoopProgressSink`,
  `EngineCapabilities`, `Snapshot`, `Outcome`, `Download`, `StartOptions`,
  `UriPool`, `State`.
- Pure components re-exported for reuse and testing: `Planner`, `SavedSegment`,
  `Assignment`, `Claim`, `ResumeState`, `Validators`, `Limiter`, `Hash`, `Algo`,
  `ProbeInfo`, `part_path`, `state_path`, `MAX_CONNECTIONS`, `clamp_connections`.

Errors are always `paradl_core::Error`, so exit codes are owned by `paradl-core`:
transport failures → `1`, verification failures → `3`, cancellation → `130`.

## Byte-exact contracts

- `MAX_CONNECTIONS = 16`, requested counts clamped with `min(max(n, 1), 16)`.
- A close-delimited 200 is streamed once; `total` stays `None` until EOF, with
  no false 100% and no quadratic re-fetch of the committed prefix.
- `<dest>.pdl.part` plus a pretty-printed `<dest>.pdl.json` sidecar are written
  atomically, survive interrupt, and resume byte-exactly. The part file is
  verified **before** it is renamed into place.
- Redirected URIs are captured once and preserved across connection recycle;
  host-sensitive headers and cookies never cross to a mirror or a cross-host
  redirect. IPv6 pinning and SOCKS proxies are supported.

## Feature flags

| Flag | Default | Behavior |
|---|---|---|
| `doh` | off | DNS-over-HTTPS via `hickory-resolver`. Off: the OS resolver is used and a supplied endpoint is logged-dropped with a reason. |

The DoH endpoint must be an http(s) URL such as `https://cloudflare-dns.com/dns-query`.
The endpoint **hostname is resolved once through the operating-system resolver to
bootstrap the connection** (a known limitation, matching `paradl-net`'s design);
the resolved IP is used only as the socket address. The hostname itself is stored
as the hickory-dns `tls_dns_name`, so SNI and certificate validation use the
correct name. Every subsequent transfer-host lookup goes over the encrypted
HTTPS transport.

HTTP/3 is accepted (`Target::http3`) and logged-dropped with a reason. There is
deliberately **no** HTTP/3 Cargo feature. Cooperative cancellation is available
through `Target::cancel`: setting the flag during a transfer cancels workers and
returns exit code 130 while preserving `.pdl.part` / `.pdl.json` for resume.

## Example

```bash
cd rust
cargo run -p paradl-engine --example download
```

The example starts a loopback Range server, plans a transfer, prints progress
through a `ProgressSink`, downloads to a temp file and asserts byte equality.
No external network is touched.

## Tests

`src/tests.rs` ports all 21 `rapids` fault-injection tests (mid-body closes,
5xx mirror failover, stalls, cross-host redirects, resume, checksum mismatch,
bounded probe/timeouts) and adds byte-exact sidecar and connection-cap checks.

```bash
cargo test -p paradl-engine
cargo clippy -p paradl-engine -- -D warnings
cargo doc -p paradl-engine --no-deps
cargo test -p paradl-engine --features doh
```
