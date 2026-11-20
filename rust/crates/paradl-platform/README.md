# paradl-platform

Byte-exact macOS and Windows origin metadata plus keychain-backed secrets for
the paraDL framework. External tools (`xattr`, `security`, `codesign`, `spctl`,
`osascript`) stay external processes; no Python is ever spawned.

## Public API

- `Platform` trait (charter section 2):
  - `set_origin_metadata(&Path, &[String]) -> Result<()>`
  - `store_secret(&str, &str, &str) -> Result<()>`
  - `load_secret(&str, &str) -> Result<Option<String>>`
- `NoopPlatform` — safe fallback; compiles and runs with no system tools.
- Feature `platform-macos` → `macos` module with `MacOSPlatform` plus pure
  byte-exact helpers (`binary_plist_string_array`, `where_froms_xattr_args`,
  `keychain_add_command`, `keychain_find_command`, codesign arg builders).
- Feature `platform-windows` → `windows` module with `WindowsPlatform` plus pure
  helpers (`format_zone_identifier_text`, `parse_zone_identifier_text`,
  `unquarantine_zone_identifier_text`, `ads_path`).

## Feature flags

| Flag | Default | Effect |
|---|---|---|
| `platform-macos` | off | Compile the macOS `xattr`/`security` implementation. |
| `platform-windows` | off | Compile the Windows `Zone.Identifier`/credential implementation. |

The default build (`cargo build -p paradl-platform`) needs **no** system tools.

## Byte-exact contracts

- **L09** — `com.apple.metadata:kMDItemWhereFroms` is a CPython-compatible binary
  plist written via `xattr -w -x <attr> <hex> <path>`. The serializer matches
  `plistlib.dumps(urls, fmt=FMT_BINARY)` byte-for-byte (empty, ASCII, extended
  length, UTF-16, and offset-table widths).
- **L10** — `Zone.Identifier` is a CRLF-delimited INI:
  `[ZoneTransfer]\r\n[ZoneId=<n>\r\n][HostUrl=<url>\r\n]`.
- **L11** — keychain secrets are piped to `security -i` on **stdin**; argv is
  only `["-i"]`, so the token never leaks into the process list.

All goldens are exercised by `tests/goldens.rs`, captured from
`tests/support/macos_fake.py` and `tests/unit/test_platform_windows.py`.

## Example

```sh
cargo run -p paradl-platform --example metadata
cargo run -p paradl-platform --features platform-macos,platform-windows --example metadata
```

## Tests

```sh
cargo test -p paradl-platform
cargo test -p paradl-platform --features platform-macos,platform-windows
cargo doc -p paradl-platform --no-deps
```
