# paradl-postproc

Checksums (BLAKE2b, MD5/SHA1/SHA256/SHA512), GNU/BSD checksum-text parsing,
companion-file discovery, **security-validated** ZIP/tar extraction, and ADB
hand-off for the paraDL framework. 7z/DMG/hdiutil stay external processes — no
Python is ever spawned.

## Public API

- `PostProcessor` trait: `verify(&Path, Option<&Digest>) -> Result<Verification>` and
  `postprocess(&PostContext) -> Result<PostResult>`.
- `DefaultPostProcessor` — production implementation using the crate helpers.
- `checksum` module: `Algorithm`, `Digest`, `compute_hashes`, `verify_file`,
  `parse_checksum_text`, `find_companion_checksum`, `SUPPORTED_ALGOS`.
- `archive` module: `is_archive`, `validate_member_path`, `validate_link_target`,
  `validate_zip`, `validate_tar`, `extract`, `sanitize_dmg_item`,
  `ArchiveSecurityError`.
- `adb` module: `Device`, `parse_devices`, `connected_devices() -> Result<Vec<Device>>`,
  `push_file(...) -> Result<bool>`, `ADB_TIMEOUT`, `DEFAULT_MUSIC_DIR`,
  `DEFAULT_DOWNLOAD_DIR`.
- Types: `Verification`, `ChecksumSource`, `PostContext`, `PostResult`.

## Feature flags

None. All functionality is available in the default build. External tools
(`7z`, `hdiutil`, `adb`) are optional and detected at runtime.

## Exit codes

A checksum mismatch, unsupported algorithm, or archive security violation maps
to `paradl_core::Error::VerificationFailed` → exit code `3`. Missing files and
I/O failures map to exit code `1`. Every external `adb` call is bounded to 5 s
(oracle `adb.py:25`); a timeout kills the child and returns a failure (exit `1`)
rather than silently reporting "no devices".

## Security contract

Before a single byte is written, every archive member is checked for absolute
paths, Windows drive prefixes, `..` traversal, canonicalized path escape,
symlink/hardlink targets escaping the destination, and tar device/FIFO entries.
All such violations fail closed with exit code `3`.

## Example

```sh
cargo run -p paradl-postproc --example hash_and_verify
```

## Tests

```sh
cargo test -p paradl-postproc
cargo doc -p paradl-postproc --no-deps
```

Golden values are captured from `src/paradl/postproc/*.py`.
