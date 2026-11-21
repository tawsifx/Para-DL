# paradl (reference binary)

The thin native binary that owns the `paradl` and `parallel-dl` command names.
All application logic lives in [`paradl-app`](../paradl-app/README.md); parsing
and the byte-exact help/version/error text live in
[`paradl-cli`](../paradl-cli/README.md).

## Byte-exact surfaces

- `paradl --version` prints `parallel-dl (paradl) v1.0.0` (oracle
  `action="version"` bytes, including the trailing newline).
- `paradl --help` prints the oracle argparse help verbatim.
- `paradl doctor` reports the binary version, the `paradl-media/1` protocol,
  the Python media-package version, yt-dlp version and JS-runtime status.
  Non-media paths never spawn Python.

## Feature flags

`api`, `tui`, `media`, `platform-macos`, `platform-windows` are all opt-in and
forwarded to `paradl-app`. The default build needs no optional system tools and
no Python interpreter. Media stays a one-shot `python -m paradl_media` worker
over `paradl-media/1`; it is never linked into this binary.

## Exit codes

`0` success, `1` failure, `2` usage, `3` verification failure, `4` missing
dependency (including media handshake failures), `130` interruption — all
owned by `paradl_core::Error`/`ExitCode`.
