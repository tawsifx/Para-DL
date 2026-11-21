# paradl-parity — three-lane differential harness

Development-only crate. It never ships in the `paradl` binary and never
contains application logic: it only runs observable behavior on both sides of
the migration and diffs it byte-for-byte. Any unexpected difference makes the
harness exit nonzero.

```bash
(cd rust && PYO3_PYTHON="$PWD/../.venv/bin/python" cargo test -p paradl-parity)
(cd rust && PYO3_PYTHON="$PWD/../.venv/bin/python" cargo clippy -p paradl-parity -- -D warnings)
(cd rust && PYO3_PYTHON="$PWD/../.venv/bin/python" cargo doc -p paradl-parity --no-deps)

# CI's protocol-mismatch negative test (must print 4):
cargo run --quiet --manifest-path rust/Cargo.toml -p paradl-parity -- \
  boundary --case version-mismatch --worker .venv-media/bin/python

# Main gate and golden-replay lanes:
cargo run --quiet --manifest-path rust/Cargo.toml -p paradl-parity -- non-media
cargo run --quiet --manifest-path rust/Cargo.toml -p paradl-parity -- media
```

TDD note: the contract tests in `tests/parity.rs` and the fixtures were written
first; the initial run was RED (8 failing lane assertions against stub
outcomes) and the implemented lanes are GREEN (12 tests including doc-tests).

## Standing ruling

The Python application becomes Rust **except media**, which remains the
standalone `python -m paradl_media` worker on `paradl-media/1`. "No Python
anywhere" is void for media and boundary paths; it is **not** void for the
non-media path (see X05/X06 below).

## Lane 1 — non-media (main cutover gate)

`non-media.rs` starts one deterministic loopback HTTP server (stable body,
`Content-Length`, `Accept-Ranges`, `ETag`) and runs the intact Python CLI
(`parallel-dl.py`) and the Rust `paradl` binary with the same arguments against
the same URL. It captures and compares, byte-for-byte:

- stdout (including every JSONL line),
- stderr,
- produced files and `.pdl.part` / `.pdl.json` sidecars (hex snapshot),
- exit codes.

The raw stdout bytes are compared byte-for-byte **except** that the numeric
value of each JSON `timestamp` field is replaced by the stable token `"<TS>"`
(`normalize_stdout_timestamps`). This is a textual substitution: field order,
spacing and every other byte remain exact, and non-JSONL output is untouched.
JSONL is additionally compared structurally with `timestamp` removed. No other
field is normalized.

**Process policy (X05/X06).** The Rust side runs with
`PARADL_ASSERT_NO_PYTHON=1` *and* a `PATH` whose `python`/`python3` are poison
shims that write a marker and exit 97. A marker makes the case fail with
`process:python_spawn`. This is the proof that no Python is spawned on a
non-media path; it is tested independently by `no_python_guard_detects_a_spawn`.

**Representative corpus and row coverage.** The full surface is enumerated in
`docs/superpowers/parity-matrix.md` (C01–C52, F01–F92, E01–E61). The executable
corpus is deliberately small and spans the main categories:

| Case | Rows | Channel exercised |
|---|---|---|
| `--version` | C20, F85 | stdout, exit code |
| `--help` | F13, F29, F36, F37 | stdout (usage/help text) |
| `doctor --json` | C05, C22, E10, E27, E28 | JSONL stdout |
| `info <loopback> --json` | C07, E37 | loopback routing, JSONL, exit code |
| `download <loopback> -o {out} --json --non-interactive` | C01, E46, E56, E59, L14 | **gated**: bytes + `.pdl.*` sidecars + stdout/stderr/JSONL/exit |

With W4-1's engine-backed binary landed, the non-media lane is now the **full
criterion-2 gate**, not a mechanism demo. The download case drives a real
transfer on both sides against one loopback server (with HEAD/Range support);
success must produce byte-identical files with both sidecars cleaned. A second,
deterministic interrupt/resume gate (`run_interrupt_resume_gate`) drives the
online `App::with_config` engine with a cooperative cancel and asserts exit
`130` plus byte-exact `.pdl.part` / `.pdl.json` preservation and a valid
`ResumeState` — no offline shim, no Python spawn.

## Lane 2 — media (golden replay, no live network)

A recorded yt-dlp `-J` fixture (`fixtures/ytdlp_j_golden.json`), the real
`MediaEngine.resolve()` from `media/`, and the recorded 32-byte artifact are
replayed by `fixtures/media_golden_replay.py`. yt-dlp is replaced in-process by
a fake that returns the recorded metadata; there is **no** DNS, HTTP, or
yt-dlp download. The **identical** protocol frames are then fed through the
real Rust bridge (`WorkerMediaResolver`) via a stub worker. The lane diffs:

- protocol response (`filename`, coerced `http_headers`,
  `audio_format_produced`, `paths`),
- synthesized JSONL (Rust `paradl-events` progress lines, timestamp-normalized),
- output bytes (SHA-256 of the replayed artifact),
- tags (`title`, `artist`, `album`, `year`) carried in the terminal frame.

The vote is unanimous only when all four agree with the golden.

## Lane 3 — boundary (protocol frames)

A stub Python worker emits canned frames through the real Rust bridge:
`ok`, `error` (`download`→1), `unsupported` (unknown kind fails closed→1),
`malformed` (chatty/non-JSON stdout must not corrupt `--json`→1), and
`version-mismatch` (unknown major → **4**). The version conformance matrix N,
N−1, N+1, unknown, missing, and unparseable is asserted through
`paradl_media::check_protocol`. The CLI exits `1` whenever any difference is
recorded (a harness must never print `DIFF` and still succeed); only a **clean**
`version-mismatch` returns `4`, which is exactly what the CI negative test
asserts. All other clean cases return `0`. See
[`boundary_process_exit_code`] for the decision table.

## Honesty caveats (no warm-cache/drift victories claimed)

- **Current non-media result:** `--version`, `--help`, the gated `download`
  file/sidecar channels, and the interrupt/resume gate are CLEAN. The lane
  still reports **real** parity differences in `doctor --json` (Python emits
  an extra `diagnose` event and richer tool/auth fields; Rust emits one
  `doctor` event) and `info --json` / `download` JSONL (event sets and field
  shapes differ, e.g. Python's `target_classified`/`progress` vs Rust's
  `result`-only). These are genuine W4-1 event-contract gaps, reported
  verbatim; nothing is normalized away or hidden to force green.
- **No-Python proof is scoped:** the poison-`PATH` canary proves the Rust
  process does not spawn `python`/`python3`; it does not trace arbitrary
  `execve` of a renamed interpreter.
- **Media startup:** only the Python worker's spawn overhead is measured
  (`startup_ms`), and only on this lane. A warm interpreter is not a
  throughput win; no cold/warm number here supports a performance claim.
- **Timestamps and ports:** JSONL `timestamp` is removed in the structural
  comparison and replaced by `<TS>` in the raw-stdout comparison; no other
  field is normalized. The loopback port is equal for both sides within a case
  but not across runs.
- **Bytes/tags are Python-owned:** Rust deliberately has no extractor. The
  media lane proves the bridge preserves the golden protocol payload, JSONL,
  bytes, and tags; it does not claim Rust produced them.
