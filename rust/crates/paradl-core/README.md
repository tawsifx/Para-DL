# paradl-core

Shared foundation for the paraDL framework: the single public `Error` → `ExitCode`
model, byte-exact ports of the Python oracle's helpers/config/paths, and a clock
abstraction. All numeric and string contracts are verified against
`tests/unit/test_models.py` and `tests/unit/test_config.py` golden values.

## Public surface

- `Error` — the one public error type reused by every crate; `exit_code()` maps
  to the six-code model (`0/1/2/3/4/130`). `Error::from_media_kind` maps a media
  worker `error.kind` without ever producing code `3`.
- `ExitCode` — `Ok=0`, `Failure=1`, `Usage=2`, `VerificationFailed=3`,
  `MissingDependency=4`, `Interrupted=130`.
- `clock::Clock` — the only public trait: `fn now(&self) -> SystemTime`,
  implemented by `SystemClock` and `FakeClock`.
- `models` — `parse_rate`, `format_rate_for_aria2`, `format_size`, `format_speed`,
  `sanitize_filename`, `is_media_domain`, `is_image_domain`,
  `has_direct_extension`/`direct_extension`, `TargetType`, `WatchdogMode`,
  `TargetInfo`, `DownloadItem`, `TransferResult`, and the single-source
  `DIRECT_EXTENSIONS` / `MEDIA_DOMAINS` / `IMAGE_DOMAINS` constants.
- `config` — `PDLConfig` with validation, coercion (`set`), atomic `save`,
  `load` with corrupt-file backup, schema migration, and `PARADL_HOME` isolation.
- `paths` — centralized state paths honoring `PARADL_HOME`.

## Feature flags

None. `paradl-core` builds with no optional system tools and depends only on
`thiserror`, `serde`, `serde_json`, and `url`.

## Example

```bash
cargo run -p paradl-core --example basics
```

See [`examples/basics.rs`](examples/basics.rs) for a runnable tour of the public
API. Golden contract tests live in [`tests/golden.rs`](tests/golden.rs).
