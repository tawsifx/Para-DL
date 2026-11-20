# paraDL → Rust Framework Parity Matrix

Every row must be cleared by its owning wave before that wave is accepted. "Proving test" is the test that must pass for the row to count.
Sources: `PARALLEL_DL_DEV_CONTEXT.md` §4, `tests/**`, `rust/rapids/src/tests.rs`, `docs/rust-replaceability-report.md`, FINAL master prompt §0.

Legend: W0..W5 = owning wave. `—` = no Python oracle (new surface).

---

## 1. CLI subcommands

| Row | Subcommand | Owning wave | Proving test |
|---|---|---|---|
| C01 | `<target>...` (download any URL/shortcut/magnet/search) | W4-1 | parity non-media lane |
| C02 | `auth set <hf\|github>` | W2-4/W4-1 | `test_platform_windows.py` keychain goldens |
| C03 | `auth status [--json]` | W2-4 | `auth_status` event golden |
| C04 | `auth remove <hf\|github>` | W2-4 | `auth_remove` event golden |
| C05 | `doctor [--json]` (alias `diagnose`) | W4-1 | `doctor` event golden; success criterion 6 |
| C06 | `install <owner/repo> [--yes] [--sign] [--no-xattr]` | W2-2/W4-1 | `install_security_check` golden |
| C07 | `info <target> [--json]` | W4-1 | parity non-media lane |
| C08 | `hash <file> [--json]` (MD5/SHA1/SHA256/SHA512/BLAKE2b) | W2-2 | checksum tests |
| C09 | `--from-curl [cmd\|file\|-]`, `--from-curl --clipboard` | W1-4/W4-1 | `curl_parser` oracle |
| C10 | `extract <archive> [dest_dir]` | W2-2 | archive security tests |
| C11 | `history [limit] [--json]` | W1-1 | `history.db` round-trip |
| C12 | `profiles`, `profiles reset <host>` | W2-1 | `profiles`/`profiles_reset` goldens |
| C13 | `queue add/list/pause/resume/remove/run/launchd-plist` | W2-4 | queue event goldens |
| C14 | `resume --all` | W2-4 | queue crash-resume golden |
| C15 | `clean [dir]` | W4-1 | parity non-media lane |
| C16 | `config [show\|set\|reset\|reset-hosts]` | W0-3/W4-1 | `test_config.py` |
| C17 | `netprobe <url> [--duration 60] [--http3-probe]` | W4-1 | doctor/netprobe checks |
| C18 | `bench <url> [--watchdog v1,v2,off] [--duration 60]` | W4-3 | benchmark harness |
| C19 | `throttle sample/summarize/report/...` | W2-1 | `test_classifier.py` + report golden |
| C20 | `--version` (both `paradl`/`parallel-dl`) | W0-1 | install scripts test |

---

## 2. CLI flags (representative; full set enumerated by W0-4)

| Row | Flag | Owning wave | Proving test |
|---|---|---|---|
| F01 | `-o/--output-dir`, `--non-interactive`, `--json` | W3-1 | `test_bug_fixes.py` CLI cases |
| F02 | `--engine <rust\|aria2>`, `--connections N` (`MAX_CONNECTIONS=16`, `min(max(n,1),16)`) | W3-1/W1-2 | `test_perf_parity.py`, rapids tests |
| F03 | `--watchdog <v1\|v2\|off>`, `--stall-timeout SEC` | W1-3 | `test_watchdog.py` |
| F04 | `--mirror <URL>` (repeatable) | W2-4 | `mirror_rejected`/`mirror_dropped` goldens |
| F05 | `--when-fast [RATE]`, `--at HH:MM`, `--priority INT`, `--interval SEC` | W2-4 | `queue_job_deferred` golden |
| F06 | `--require-ac`, `--no-require-ac`, `--pause-on-low-power` | W2-4 | guard tests |
| F07 | `--sign`, `--no-xattr`, `--notify`, `--no-caffeinate`, `--no-net-watch` | W2-2/W2-4 | `install_security_check` golden |
| F08 | `--header`, `--cookies-from-browser`, `--cookies` | W1-4/W3-4 | cross-host credential non-leak |
| F09 | `--http3` (accepted, logged drop) | W1-4 | DoH/HTTP3 honesty note |
| F10 | `--doh-endpoint <URL>` (implement via hickory-dns) | W1-4 | DoH test |
| F11 | `--accept-ranges`, `--total` (accepted-then-discarded in PyO3; keep flags) | W3-1 | CLI help golden |
| F12 | `--audio-only`, `--format`, `--embed-metadata`, `--embed-thumbnail` (media) | W3-4 | media golden fixtures |

---

## 3. JSONL events (~48; `paradl-events` single emitter)

| Row | Event | Owning wave | Proving test |
|---|---|---|---|
| E01 | `target_classified` | W1-4 | golden shape |
| E02 | `progress` | W1-2 | golden shape |
| E03 | `watchdog_action` (incl. `tier2_pause_unpause`) | W1-3 | `test_watchdog.py` |
| E04 | `mirror_rejected`, `mirror_dropped` | W2-4 | golden shapes |
| E05 | `when_fast_probe` | W2-4 | golden shape |
| E06 | `queue_job_added`, `queue_list`, `queue_job_started`, `queue_job_deferred`, `queue_crash_recovered`, `launchd_plist_generated` | W2-4 | golden shapes |
| E07 | `profiles`, `profiles_reset` | W2-1 | golden shapes |
| E08 | `auth_status`, `auth_set`, `auth_remove` | W2-4 | golden shapes |
| E09 | `install_security_check` | W2-2 | golden shape |
| E10 | `doctor` | W4-1 | golden shape |
| E11 | `verification` (`verified=true` on success, no `error` event) | W1-2/W2-2 | golden shape |
| E12 | `result` (full field set incl. `audio_format_produced`, `exit_code`) | W4-1 | golden shape |
| E13 | `resilience_event` (WAKE/NET_CHANGE/STALL) | W2-4 | golden shape |
| E14 | `hook_*` (30s timeout, JSON stdin) | W2-4 | golden shape |
| E15 | all other oracle events enumerated by W2-4/W4-2 | W2-4 | full golden corpus |
| E16 | redaction: cross-host cookie/`Authorization`/`Proxy-Authorization` non-leak | W2-4 | redaction golden |

---

## 4. Config fields / env vars

| Row | Field / env | Owning wave | Proving test |
|---|---|---|---|
| G01 | `config.json` fields (full set enumerated by W0-4) | W0-3 | `test_config.py` |
| G02 | `PARADL_HOME` isolation | W0-3 | `test_config.py` |
| G03 | `PARADL_MEDIA_PY` (media python override) | W3-4b | handshake tests |
| G04 | `HF_TOKEN` / `GITHUB_TOKEN` env precedence | W2-4 | auth tests |
| G05 | `PARADL_ASSERT_NO_PYTHON=1` (parity guard) | W4-2 | non-media lane |

---

## 5. DB tables / schema

| Row | Table / property | Owning wave | Proving test |
|---|---|---|---|
| D01 | `history.db` schema v4, WAL | W1-1 | `history.db` opens/migrates/round-trips |
| D02 | 7 history tables | W1-1 | W1-1 tests |
| D03 | 3 throttle tables (shared file) | W1-1/W2-1 | W1-1/W2-1 tests |
| D04 | `queue_jobs` (shared) | W1-1/W2-4 | queue round-trip |
| D05 | device+inode guard | W1-1 | `test_sqlite_guard.py` |

---

## 6. Contract literals (byte-exact)

| Row | Literal | Owning wave | Proving test |
|---|---|---|---|
| L01 | `http_headers` values→str, keys unchanged | W3-4a/b | media protocol test |
| L02 | `filename` via sanitize_filename (200-char cap, Windows reserved) | W0-3/W3-4 | `test_models.py` golden |
| L03 | `audio_format_produced` exact strings (`media.py:48–72`) | W3-4 | media golden fixture |
| L04 | JSONL `{event, timestamp: round(time.time(),3), ...redacted}` | W2-4 | event goldens |
| L05 | timestamp rounding: Python bankers `round()` vs Rust `round` at .5 | W2-4 | numeric parity test |
| L06 | `MIN/MAX connections`: `min(max(n,1),16)` | W1-2 | rapids test |
| L07 | `WatchdogActionType.tier2_pause_unpause` | W1-3 | `test_watchdog.py` |
| L08 | close-delimited 200 → no false 100% | W1-2 | `rapids/src/tests.rs` |
| L09 | plist xattr hex (binary plist) | W2-2 | `macos_fake.py` golden |
| L10 | `Zone.Identifier` CRLF INI | W2-2 | `test_platform_windows.py` |
| L11 | keychain secrets-over-stdin (no argv leak) | W2-4 | keychain golden |
| L12 | `report.json` schema `1.0` with `modules.m0..m10` | W2-1 | report golden |
| L13 | 12 seeded `test_classifier.py` cases byte-for-byte | W2-1 | `test_classifier.py` |
| L14 | `.pdl.part` + `.pdl.json` sidecars: byte-exact on resume, persist on interrupt | W1-2 | `rapids/src/tests.rs` |
| L15 | exit codes `0/1/2/3/4/130` | W0-3/W3-4b | full mapping test |

---

## 7. Media protocol `paradl-media/1` fields

| Row | Field | Owning wave | Proving test |
|---|---|---|---|
| P01 | `protocol: "paradl-media/1"` on request + all frames | W3-4 | handshake contract |
| P02 | `request_id` correlation | W3-4b | boundary lane |
| P03 | `op`: resolve/download_managed/finalize/claims | W3-4a/b | media golden replay |
| P04 | `frame`: progress/result/error | W3-4b | boundary lane |
| P05 | versions: worker/yt_dlp/mutagen/ffmpeg + caps | W3-4a/b | `doctor` + handshake |
| P06 | major-version rejection → exit 4 + diagnostic | W3-4b | boundary lane version-mismatch |
| P07 | minor additions tolerated (`1.1` ↔ `1.0`) | W3-4b | version-compat matrix |
| P08 | missing/unparseable version → fail closed exit 4 | W3-4b | boundary lane malformed |
| P09 | `error.kind` → ExitCode mapping (no worker `3`) | W3-4b | boundary lane error cases |
| P10 | stdout protocol-only; humans→stderr | W3-4a/b | boundary lane malformed |
| P11 | SIGINT→130, preserve `.pdl.part`/`.pdl.json` | W3-4a/b | media lane |
| P12 | yt-dlp pip-installed in binary-managed venv, never frozen | W0-1 | install scripts test |

---

## 8. Distribution / process policy

| Row | Requirement | Owning wave | Proving test |
|---|---|---|---|
| X01 | no committed `.so` | W0-1 | `find . -name '*.so'` |
| X02 | CI builds Rust natively (macos-14/13, windows) | W0-1 | CI YAML |
| X03 | media package has no console script | W0-1/W3-4a | `media/pyproject.toml` |
| X04 | `parallel-dl.py` dev-only | W5-1 | cutover audit |
| X05 | non-media paths spawn no Python (`PARADL_ASSERT_NO_PYTHON=1`) | W4-2 | non-media lane |
| X06 | Python spawned only via `python -m paradl_media` for media | W4-2 | process guard |
| X07 | Homebrew formula + install scripts work clean | W0-1/W5-2 | fresh-machine test |
