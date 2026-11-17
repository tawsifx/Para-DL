# Third-Party Notices — `mdm-engine` (Mister Download Manager)

## Source

- **Project:** Mister Download Manager (`anefzaoui/misterdownloadmanager`)
- **Source archive:** `https://github.com/anefzaoui/misterdownloadmanager/archive/refs/heads/main.zip`
- **Version metadata:** workspace `version = "0.5.2"`, `rust-version = "1.98"`,
  `edition = "2024"` (from the archive's root `Cargo.toml`)
- **Author / copyright holder:** Ahmed Nefzaoui
- **License:** **GPL-3.0-or-later**
- **Archive handling:** downloaded and extracted with `paradl` itself into
  `/tmp/mdm-ref/src`; the archive has no `.git` directory, so no commit SHA is
  available. The `main` branch tip at retrieval time (2026-11-17) is the reference.

## Files ported / adapted from Mister Download Manager

All ported Rust files retain an SPDX `GPL-3.0-or-later` header and a provenance
comment naming the original file.

| paradl file | MDM original | Modifications made |
|---|---|---|
| `rust/mdm-engine/src/planner.rs` | `crates/engine/src/planner.rs` | Inline tests moved to `src/tests.rs`; no logic changes. |
| `rust/mdm-engine/src/state.rs` | `crates/engine/src/state.rs` | `craft_atomic::atomic_write` replaced with std write-temp-then-rename; legacy `.idmnext` migration and inline tests removed. |
| `rust/mdm-engine/src/limiter.rs` | `crates/engine/src/limiter.rs` | Inline tests moved to `src/tests.rs`; no logic changes. |
| `rust/mdm-engine/src/verify.rs` | `crates/engine/src/verify.rs` | Inline tests moved to `src/tests.rs`; no logic changes. |
| `rust/mdm-engine/src/naming.rs` | `crates/engine/src/naming.rs` | Only the `.mdm.part` naming helper retained. |

New files written for paradl (not copied from MDM, but designed against its
planner/state/limiter/verify interfaces): `rust/mdm-engine/src/download.rs`,
`probe.rs`, `error.rs`, `lib.rs`, `tests.rs`, and the PyO3 crate
`rust/paradl-rs/src/lib.rs`.

## Deliberate exclusions

The MDM workspace pins git forks (`storytold/craft-libs`, `anefzaoui/egui`,
`anefzaoui/rqbit`) and optional native-dependency features. None were vendored:

- `wreq` / BoringSSL (requires CMake, C/C++ compiler, libclang, and NASM on Windows)
- `rquickjs` / PAC scripts (C build; libclang on 32-bit Windows)
- FTP/FTPS, BitTorrent, site-specific resolvers, app/UI crates

The trimmed crate uses crates.io dependencies only, with `reqwest` + `rustls`.

## ⚠️ LICENSE CONFLICT — ACTION REQUIRED BY MAINTAINER

`misterdownloadmanager` and every ported file above are **GPL-3.0-or-later**.
`parallel-dl` declares `license = { text = "MIT" }` in `pyproject.toml`.

Under GPL-3.0, distributing a combined work that incorporates GPL-covered code
generally requires that the **whole combined work be licensed under
GPL-3.0-or-later**. Keeping the project's MIT declaration while shipping this
Rust extension would be inconsistent.

**This migration does not silently change the project license.** The maintainer
must choose one of:

1. Re-license `parallel-dl` (and this combined distribution) under
   GPL-3.0-or-later, updating `pyproject.toml` and all notices; or
2. Keep the GPL `mdm-engine` extension as a **separately distributed, optional
   plugin** that is never bundled into the MIT core, with clear per-artifact
   licensing; or
3. Replace the ported GPL code with an independently written, permissively
   licensed implementation (a clean-room rewrite).

Until a decision is made, the vendored Rust code remains
**GPL-3.0-or-later** as required by its original license.
