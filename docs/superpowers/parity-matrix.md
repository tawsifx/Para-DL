# paraDL → Rust Framework Parity Matrix

Every row must be cleared by its owning wave before that wave is accepted. "Proving test" is the test that must pass for the row to count.
Sources: `PARALLEL_DL_DEV_CONTEXT.md` §4, `tests/**`, `rust/rapids/src/tests.rs`, `docs/rust-replaceability-report.md`, FINAL master prompt §0.

Extraction for this inventory (W0-4, READ-ONLY, BASE `0ebb09e`):
- CLI: `src/paradl/cli/parser.py:112–580` (79 `add_argument` calls), `src/paradl/cli/dispatch.py:34–226`, `src/paradl/throttle/cli.py:55–235` (24 `add_argument` calls).
- Config/env: `src/paradl/core/config.py:20–336`, `src/paradl/config.py`, `src/paradl/core/paths.py`, `src/paradl/net/accelerate.py:40`, `src/paradl/ui/display.py:28`.
- Events: every `UI.emit_event(<literal>, **kwargs)` call in `src/paradl/**` (AST-extracted, 44 distinct names) plus the one raw-dict emitter at `src/paradl/throttle/cli.py:296–308`.
- DB: `src/paradl/storage/history.py:188–343`, `src/paradl/throttle/store.py:63–112`.
- Literals: `src/paradl/core/models.py`, `src/paradl/postproc/checksum.py:19`, `src/paradl/engines/media.py:48–72`, `src/paradl/throttle/models.py:9,483–512`, `src/paradl/platform/{macos,windows}.py`, `src/paradl/cli/main.py:16–46`.
- Media protocol: `docs/superpowers/framework-charter.md` §4 (the authoritative `paradl-media/1` schema).

Legend: W0..W5 = owning wave. `—` = no Python oracle (new surface). "NEW GOLDEN" = no current Python test asserts this event/contract; the proving test must be authored in `paradl-oracle` (W0-2) and exercised by the owning wave. Seeded rows C01–C20, F01–F12, E01–E16, G01–G05, D01–D05, L01–L15, P01–P12, X01–X07 are preserved; all other rows are the W0-4 expansion.

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
| C07 | `info <target> [--json]` | W4-1 | parity non-media lane; `info` event golden |
| C08 | `hash <file> [--json]` (MD5/SHA1/SHA256/SHA512/BLAKE2b) | W2-2 | checksum tests; `hashes` event golden |
| C09 | `--from-curl [cmd\|file\|-]`, `--from-curl --clipboard` | W1-4/W4-1 | `curl_parser` oracle |
| C10 | `extract <archive> [dest_dir]` | W2-2 | archive security tests |
| C11 | `history [limit] [--json]` | W1-1 | `history.db` round-trip; `history` event golden |
| C12 | `profiles`, `profiles reset <host>` | W2-1 | `profiles`/`profiles_reset` goldens |
| C13 | `queue add/list/pause/resume/remove/run/launchd-plist` | W2-4 | queue event goldens |
| C14 | `resume --all` | W2-4 | queue crash-resume golden |
| C15 | `clean [dir]` | W4-1 | parity non-media lane; `clean` event golden |
| C16 | `config [show\|set\|reset\|reset-hosts]` | W0-3/W4-1 | `test_config.py` |
| C17 | `netprobe <url> [--duration 60] [--http3-probe]` | W4-1 | doctor/netprobe checks; `netprobe` event golden |
| C18 | `bench <url> [--watchdog v1,v2,off] [--duration 60]` | W4-3 | benchmark harness; `bench` event golden |
| C19 | `throttle sample/summarize/report/...` | W2-1 | `test_classifier.py` + report golden |
| C20 | `--version` (both `paradl`/`parallel-dl`) | W0-1 | install scripts test |
| C21 | `auth` (bare → defaults to `status`) — `parser.py` SUBCOMMANDS set | W2-4 | `auth_status` golden |
| C22 | `diagnose [--json]` (explicit alias for `doctor`) | W4-1 | `diagnose` event golden (emitted alongside `doctor`, `doctor.py:374–375`) |
| C23 | `pip install <pkg>` / `pip upgrade <pkg>` (alias `pypi`) | W2-3 | `test_pypi_transitive.py` |
| C24 | `brew install <formula>` | W2-3 | brew resolver golden |
| C25 | `brew update` | W2-3 | brew resolver golden (`NONINTERACTIVE=1 brew update`) |
| C26 | `brew upgrade [formula]` | W2-3 | brew resolver golden |
| C27 | `server [--host 127.0.0.1] [--port 8000] [--reload]` | W3-3 | `test_api_optional_pydantic.py`; `api_server_started` golden |
| C28 | `tui` | W3-2 | golden output tests |
| C29 | `throttle` (bare → defaults to `quick`) — `throttle/cli.py:55–64,73–79` | W2-1 | `test_classifier.py` |
| C30 | `throttle quick` (300 MB default budget) | W2-1 | `test_classifier.py` |
| C31 | `throttle full` (3 GB default budget, M0–M10) | W2-1 | `report.json` v1.0 golden |
| C32 | `throttle sample` (~1.2–1.5 s probe; 45-min idempotency) | W2-1 | `store.py:30` idempotency golden |
| C33 | `throttle coverage` (24h × weekday/weekend matrix) | W2-1 | coverage golden |
| C34 | `throttle launchd-plist` | W2-2 | `macos_fake.py` plist golden; `throttle_launchd_plist_generated` event |
| C35 | `throttle report [--html] [--run-id ID]` | W2-1 | `report.json` v1.0 golden |
| C36 | `throttle server` (`--host`, `--port 8765`, `--udp-port 8766`) | W2-1 | VPS server golden |
| C37 | `throttle targets [--targets FILE] [--domestic URL]` | W2-1 | target validation golden |
| C38 | `queue add <target> [-o dir] [--priority N] [--mirror URL] [--when-fast [RATE]] [--at HH:MM] [--no-require-ac]` | W2-4 | `queue_job_added` golden |
| C39 | `queue list [--json]` (priority DESC, id ASC) | W2-4 | `queue_list` golden |
| C40 | `queue pause <id>... \| --all` | W2-4 | `queue_paused` golden |
| C41 | `queue resume <id>... \| --all` | W2-4 | `queue_resumed` golden |
| C42 | `queue remove <id>... \| --all` | W2-4 | `queue_removed` golden |
| C43 | `queue run [-c N] [--when-fast [RATE]] [--at HH:MM]` | W2-4 | `queue_job_started`/`queue_job_deferred` goldens |
| C44 | `queue launchd-plist [-o path] [--at HH:MM \| --interval SEC]` | W2-4 | `launchd_plist_generated` golden |
| C45 | `config show` (bare default) | W0-3 | `test_config.py` |
| C46 | `config set <key> <value>` | W0-3 | `test_config.py`; `config_updated` golden |
| C47 | `config reset` | W0-3 | `config_reset` golden |
| C48 | `config reset-hosts` | W0-3 | `hosts_reset` golden |
| C49 | `profiles` (bare → defaults to `list`) | W2-1 | `profiles` golden |
| C50 | `auth` action set {`set`, `status`, `remove`} — `dispatch.py:170–175` | W2-4 | auth goldens |
| C51 | `<target>...` shortcut prefixes: `gh:`, `hf:`, `ipsw:`, `pip:`, `brew:`, `ytsearch1:` | W2-3/W1-4 | routing cases vs Python oracle |
| C52 | `parallel-dl` / `paradl` dual binary names | W0-1 | install scripts test; `--version` |

> Note on C19: the seed text says `throttle sample/summarize/report/...`; the actual `THROTTLE_SUBCOMMANDS` tuple (`throttle/cli.py:55–64`) has no `summarize`. Authoritative list is C29–C37 above. Seed retained per "add, don't delete".

---

## 2. CLI flags (complete enumeration; `parser.py` + `throttle/cli.py`)

Seeded representative rows F01–F12 preserved. F13–F90 below enumerate **every** `add_argument` definition (short/long aliases grouped per definition) with exact option strings and `parser.py` line.

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
| F10 | `--doh-endpoint <URL>` (implement via hickory-dns) | W1-4 | DoH test |
| F12 | `--audio-only`, `--format`, `--embed-metadata`, `--embed-thumbnail` (media) | W3-4 | media golden fixtures |
| F13 | `targets` (positional, `nargs="*"`; `parser.py:112`) | W3-1 | CLI help golden |
| F14 | `-a`/`--audio` (`store_true`, default True; `:117`) | W3-4 | media golden fixtures |
| F15 | `-v`/`--video` (`store_true`; `:124`) | W3-4 | media golden fixtures |
| F16 | `-l`/`--lossless` (`store_true`, default True; `:130`) | W3-4 | media golden fixtures |
| F17 | `--no-lossless` (`store_true`; `:137`) | W3-4 | media golden fixtures |
| F18 | `--mp3` (`store_true`; `:142`) | W3-4 | media golden fixtures |
| F19 | `--audio-codec` ∈ {lossless,native,mp3,flac,opus,wav,aac} (`:147`) | W3-4 | media golden fixtures; `test_config.py` |
| F20 | `-Q`/`--quality` ∈ {best,4k,1440p,1080p,720p,480p} (`:152`) | W3-4 | media golden fixtures |
| F21 | `--playlist` (`store_true`; `:158`) | W3-4 | media golden fixtures |
| F22 | `--subs` (`store_true`; `:163`) | W3-4 | media golden fixtures |
| F23 | `--no-thumbnail` (`store_false`→`thumbnail`, default True; `:168`) | W3-4 | media golden fixtures |
| F24 | `--cookies-from-browser` (browser values safari/chrome/firefox; `:175`) | W1-4 | cross-host credential non-leak |
| F25 | `--sponsorblock` (`store_true`; `:179`) | W3-4 | media golden fixtures |
| F26 | `--ejs-remote` (`store_true`, `--remote-components ejs:github`; `:184`) | W3-4a | `doctor` + handshake |
| F27 | `-c`/`--connections` (int, default `config.connections`=16; `:190`) | W1-2/W3-1 | `test_perf_parity.py`, rapids tests |
| F28 | `--engine` (choices `sorted(VALID_ENGINES)`; `_EngineAction` sets active engine at parse time; `:197`) | W3-1 | `test_perf_parity.py` |
| F29 | `-o`/`--output` (dest `output`; seeded F01 used old `--output-dir`) | W3-1 | `test_bug_fixes.py` CLI cases |
| F30 | `-i`/`--input-file` (`:211`) | W3-1 | CLI help golden |
| F31 | `-x`/`--extract` (`store_true`; `:216`) | W2-2 | archive security tests |
| F32 | `--checksum` (`[algo:]<hash>`; `:222`) | W2-2 | checksum tests; `verification` golden |
| F33 | `--verify` (`store_true`; companion `.sha256`/`.md5`; `:226`) | W2-2 | checksum tests |
| F34 | `-f`/`--format` (yt-dlp spec; `:231`) | W3-4 | media golden fixtures |
| F35 | `-q`/`--quiet` (`store_true`; `:236`) | W3-2 | CLI help golden |
| F36 | `--json` (`store_true`; `:242`) | W2-4 | all JSONL goldens |
| F37 | `--non-interactive` (`store_true`; `:247`) | W3-1 | `test_bug_fixes.py` CLI cases |
| F38 | `-y`/`--yes` (`store_true`; `:252`) | W2-2 | `install` unsigned golden |
| F39 | `--verbose` (`store_true`; `:258`) | W3-1 | logging golden |
| F40 | `--debug` (`store_true`; `:263`) | W3-1 | logging golden |
| F41 | `--limit-rate` (e.g. `10MB/s`, `80Mbps`, `500K`; `:268`) | W1-2 | `parse_rate` golden |
| F42 | `--anti-throttle [RATE]` (`nargs="?"`, const `4MBps`; `:272`) | W1-3 | `test_watchdog.py` |
| F43 | `--no-anti-throttle` (`store_const`→None; `:279`) | W1-3 | `test_watchdog.py` |
| F44 | `--anti-throttle-grace` (int, default 5, clamped min 3 in config; `:286`) | W1-3/W0-3 | `test_config.py`, `test_watchdog.py` |
| F45 | `--watchdog` (v1/v2/off or comma list for bench; `:292`) | W1-3/W4-3 | `test_watchdog.py` |
| F46 | `--no-caffeinate` (`store_false`→`caffeinate`; `:297`) | W2-4 | guard tests |
| F47 | `--no-net-watch` (`store_false`→`net_watch`; `:304`) | W2-4 | guard tests |
| F48 | `--stall-timeout` (float, default 30.0, 0 disables; `:311`) | W2-4 | `resilience_event` golden |
| F49 | `--require-ac` (`store_true`, default None; `:317`) | W2-4 | guard tests |
| F50 | `--no-require-ac` (`store_false`, default None; `:324`) | W2-4 | guard tests |
| F51 | `--pause-on-low-power` (`store_true`; `:331`) | W2-4 | guard tests |
| F52 | `--mirror <URL>` (`append`→`mirrors`, repeatable; `:337`) | W2-4 | `mirror_rejected`/`mirror_dropped` goldens |
| F53 | `--when-fast [RATE]` (`nargs="?"`, const `DEFAULT`; `:344`) | W2-4 | `when_fast_probe`/`queue_job_deferred` goldens |
| F54 | `--at HH:MM` (dest `start_at`; `:352`) | W2-4 | `queue_job_deferred` golden |
| F55 | `--priority INT` (int, default 0; `:358`) | W2-4 | `queue_list` golden |
| F56 | `--all` (dest `all_flag`, `store_true`; `:364`) | W2-4 | `queue_resumed` golden |
| F57 | `--interval SEC` (int, default 900; `:370`) | W2-4 | `launchd_plist_generated` golden |
| F58 | `--duration SEC` (float, default 60.0; `:376`) | W4-1 | `netprobe`/`bench` goldens |
| F59 | `--http3-probe` (`store_true`; `:382`) | W1-4 | `netprobe.http3_supported` golden |
| F61 | `--proxy` / `--via-proxy` (dest `proxy`, metavar URL; `:401`) | W1-4 | cross-host credential non-leak |
| F62 | `--push` / `--adb` (dest `push_adb`, default `config.auto_adb_push`; `:408`) | W2-2 | ADB golden |
| F63 | `--asset` (pattern/extension filter; `:418`) | W2-3 | `test_github_acceleration.py` |
| F64 | `--all-assets` (`store_true`; `:422`) | W2-3 | `test_github_acceleration.py` |
| F65 | `--select` (`store_true`, TTY-only; `:427`) | W3-2 | CLI help golden |
| F66 | `--source` (`store_true`; `:432`) | W2-3 | `test_github_acceleration.py` |
| F67 | `--branch` (`:437`) | W2-3 | `test_github_acceleration.py` |
| F68 | `--install <target>` (dest `install_target`; `:443`) | W2-2 | `install_security_check` golden |
| F69 | `--info <target>` (dest `info_target`; `:448`) | W4-1 | `info` golden |
| F70 | `--hash <file>` (dest `hash_file`; `:453`) | W2-2 | `hashes` golden |
| F71 | `--history` (`store_true`; `:458`) | W1-1 | `history` golden |
| F72 | `--history-limit INT` (default 15; `:463`) | W1-1 | `history` golden |
| F73 | `--search` (`:469`) | W1-1 | `history` golden |
| F74 | `--type` ∈ {direct,stream,search,github,huggingface,ipsw,pypi,brew,torrent} (dest `history_type`; `:473`) | W1-1 | `history` golden |
| F75 | `--success` / `--failed` (dest `history_success`; `:489`,`:496`) | W1-1 | `history` golden |
| F76 | `--clear-history` (`store_true`; `:502`) | W1-1 | `history` golden |
| F77 | `--from-history ID` (int; `:507`) | W1-1 | `history` golden |
| F78 | `--clean` (`store_true`; `:513`) | W4-1 | `clean` golden |
| F79 | `--diagnose` / `--doctor` (`store_true`; `:518`,`:523`) | W4-1 | `doctor`/`diagnose` goldens |
| F80 | `--sign` (`store_true`; `codesign --force --sign -`; `:528`) | W2-2 | `macos_fake.py` golden |
| F81 | `--no-xattr` (`store_true`; `:533`) | W2-2 | `macos_fake.py` golden |
| F82 | `--notify` (`store_true`; `:538`) | W2-4 | hook golden |
| F83 | `--from-curl [cmd\|file\|-]` (`nargs="?"`, const `-`; `:543`) | W1-4 | `curl_parser` oracle |
| F84 | `--clipboard` (`store_true`, pbpaste; `:551`) | W1-4 | `curl_parser` oracle |
| F85 | `--version` (`action="version"`, `parallel-dl (paradl) v{VERSION}`; `:556`) | W0-1 | install scripts test |
| F86 | `--host` (default `127.0.0.1`; `:563`) | W3-3 | `api_server_started` golden |
| F87 | `--port` (int, default 8000; `:568`) | W3-3 | `api_server_started` golden |
| F88 | `--reload` (`store_true`; `:574`) | W3-3 | CLI help golden |
| F89 | `throttle` parser: `--targets`, `--domestic` (append), `--plan-mbps`, `--isp`, `--via-proxy`, `--interface` (`cli.py:85–127`) | W2-1 | target validation golden |
| F90 | `throttle` parser: `--ipv4`/`--ipv6` (mutually exclusive), `--data-budget`, `-o`/`--output`, `--html` (default True), `--run-id`, `--force` (bypass 45-min window), `--allow-32`, `--seed` (default 42) (`cli.py:128–188`) | W2-1 | `test_classifier.py`, report golden |
| F91 | `throttle` parser: `--host` (default 127.0.0.1), `--port` (default 8765), `--udp-port` (default 8766), `--duration`, `--json`, `-y`/`--yes`, `--non-interactive` (`cli.py:189–234`) | W2-1/W2-4 | VPS server golden |
| F92 | `throttle` parser positionals: `subcommand` (`nargs="?"`, choices = C29–C37, default `quick`), `extra_args` (`nargs="*"`) — `throttle/cli.py:73–84` | W2-1 | CLI help golden |

### Retired CLI flag rows (W5-3)

| Row | Retired flag(s) | Reason |
|---|---|---|
| F09 | `--http3` (accepted, logged drop) | retired at W5-3; Python oracle deleted |
| F11 | `--accept-ranges`, `--total` | retired at W5-3; Python oracle deleted |
| F60 | `--http3` / `--no-http3` | retired at W5-3; Python oracle deleted |

> Seeded F01/F08/F10/F12 describe the *target Rust surface* (aliases `--output-dir`, `--header`, `--cookies`, `--doh-endpoint`, `--audio-only`, `--embed-metadata`, `--embed-thumbnail`) that do not all exist verbatim in the current Python parser; they remain as acceptance rows for W3-1/W3-4. F13–F91 are the byte-exact current Python parser surface. F09, F11 and F60 are retired at W5-3; Python oracle deleted — see the retired-rows table above.

---

## 3. JSONL events (44 distinct names; single `paradl-events` emitter)

`UI.emit_event()` writes `{ "event": <name>, "timestamp": round(time.time(),3), ...redacted_payload }` on stdout only when `--json` (`ui/display.py:142–152`). Field sets below are the exact kwargs at each call site (union where a name has variants). Seeded E01–E16 preserved; E17–E60 enumerate every distinct name. There is **no** current Python test asserting most of these events — most proving tests are NEW GOLDENs to be authored by W0-2 and exercised by the owning wave; this is stated honestly per row.

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
| E17 | `api_server_started` {`host`, `port`} — `api/server.py:904` | W3-3 | NEW GOLDEN |
| E18 | `auth_remove` {`service`, `success`} — `auth/manager.py:208` | W2-4 | NEW GOLDEN |
| E19 | `auth_set` {`service`, `success`} — `auth/manager.py:189` | W2-4 | NEW GOLDEN |
| E20 | `auth_status` {`services`} — `auth/manager.py:148` | W2-4 | NEW GOLDEN |
| E21 | `bench` {`url`, `duration_limit_sec`, `results`} — `diagnostics/bench.py:113` | W4-3 | NEW GOLDEN |
| E22 | `clean` {`directory`, `removed`} — `app/commands.py:82,96` | W4-1 | NEW GOLDEN |
| E23 | `config` {`config`} — `app/commands.py:109` | W0-3 | NEW GOLDEN |
| E24 | `config_reset` {`config`} — `app/commands.py:151` | W0-3 | NEW GOLDEN |
| E25 | `config_updated` {`key`, `value`} — `app/commands.py:139` | W0-3 | NEW GOLDEN |
| E26 | `deferred` {`reason`, `start_at`, `wait_seconds`} — `app/commands.py:724` | W2-4 | NEW GOLDEN |
| E27 | `diagnose` {same payload as `doctor` via `**payload`} — `diagnostics/doctor.py:375` | W4-1 | NEW GOLDEN |
| E28 | `doctor` {`version`, `ytdlp`, `ytdlp_ejs`, `auth`, `macos_utilities`, `core_tools`, `fixes`} — `diagnostics/doctor.py:374` | W4-1 | NEW GOLDEN (doctor contract) |
| E29 | `error` {`message`} / {`reason`,`message`} / {`reason`,`status_code`,`message`,`target`} / {`reason`,`measured_bps`,`threshold_bps`,`message`} — 22 call sites | W2-4 | `test_rust_pipeline_e2e.py:99` + NEW GOLDEN variants |
| E30 | `hashes` {`file`, `filename`, `size`, `duration`, `hashes`} — `postproc/checksum.py:217` | W2-2 | NEW GOLDEN |
| E31 | `history` {`entries`, `count`, `search`, `target_type`, `success`} — `storage/history.py:581` | W1-1 | NEW GOLDEN |
| E32 | `hook_error` {`hook`, `error`} — `resilience/hooks.py:104` | W2-4 | NEW GOLDEN |
| E33 | `hook_executed` {`hook`, `success`} — `resilience/hooks.py:85` | W2-4 | NEW GOLDEN |
| E34 | `hook_failed` {`hook`, `exit_code`, `stderr`} — `resilience/hooks.py:78` | W2-4 | NEW GOLDEN |
| E35 | `hook_timeout` {`hook`, `timeout_seconds`} — `resilience/hooks.py:100` | W2-4 | NEW GOLDEN (30s timeout) |
| E36 | `hosts_reset` {`cleared`} — `app/commands.py:160` | W0-3 | NEW GOLDEN |
| E37 | `info` {`target`, `properties`} — `app/inspector.py:132` | W4-1 | NEW GOLDEN |
| E38 | `install_security_check` {`file`, `codesign_valid`, `codesign_detail`, `gatekeeper_accepted`, `gatekeeper_detail`, `verified_by_pdl`} — `engines/github.py:999` | W2-2 | NEW GOLDEN |
| E39 | `interrupted` {`signal`, `exit_code`} — `cli/main.py:24` | W0-3/W4-1 | NEW GOLDEN (SIGINT/SIGTERM→130) |
| E40 | `launchd_plist_generated` {`path`, `label`, `logs_dir`, `lint`} — `resilience/queue.py:807` | W2-4 | NEW GOLDEN |
| E41 | `mirror_dropped` {`mirror_url`, `failures`, `reason`} — `storage/mirrors.py:155` | W2-4 | NEW GOLDEN |
| E42 | `mirror_rejected` {`mirror_url`, `reason`, `expected_length`, `mirror_length`} — `storage/mirrors.py:361` | W2-4 | NEW GOLDEN |
| E43 | `netprobe` {`url`, `host`, `supports_range`, `ipv6_DNS`, `throttle_class`, `recommended_connections`, `recommended_action`, `verdict`, `http3_supported`, `http3_detail`, `passes`} — `diagnostics/netprobe.py:340` | W4-1 | NEW GOLDEN |
| E44 | `profiles` {`profiles`, `count`} — `storage/history.py:899` | W2-1 | NEW GOLDEN |
| E45 | `profiles_reset` {`host`, `cleared`} — `app/commands.py:189` | W2-1 | NEW GOLDEN |
| E46 | `progress` {`title`, `downloaded_bytes`, `total_bytes`, `percent`, `speed_bps`, `eta_seconds`, `connections`, `peer_count`, `watchdog_state`, `file_progress`, `connection_speeds`} — `ui/display.py:380` | W1-2 | NEW GOLDEN (note 11 fields vs 9 in DEV_CONTEXT §4) |
| E47 | `queue_crash_recovered` {`recovered_count`} — `resilience/queue.py:452` | W2-4 | NEW GOLDEN |
| E48 | `queue_job_added` {`job_id`, `target`, `priority`, `require_ac`, `when_fast_bps`, `start_at`} + {`job_id`, `target`, `from_history`} — `app/commands.py:295,450` | W2-4 | NEW GOLDEN |
| E49 | `queue_job_deferred` {`job_id`, `reason`, `start_at`} + {`job_id`, `reason`, `measured_bps`, `threshold_bps`} — `resilience/queue.py:601,617` | W2-4 | NEW GOLDEN |
| E50 | `queue_job_started` {`job_id`, `target`, `priority`, `connections`} — `resilience/queue.py:632` | W2-4 | NEW GOLDEN |
| E51 | `queue_list` {`jobs`} — `resilience/queue.py:461` | W2-4 | NEW GOLDEN |
| E52 | `queue_paused` {`paused`} — `app/commands.py:343` | W2-4 | NEW GOLDEN |
| E53 | `queue_removed` {`removed`} — `app/commands.py:323` | W2-4 | NEW GOLDEN |
| E54 | `queue_resumed` {`resumed`} + {`resumed`, `all`} — `app/commands.py:360,471` | W2-4 | NEW GOLDEN |
| E55 | `result` variant A {`subcommand`, `target`, `success`, `file_path`, `checksum_verified`, `error`, `exit_code`} — `app/commands.py:605` | W4-1 | NEW GOLDEN |
| E56 | `result` variant B {`target`, `target_type`, `success`, `file_path`, `bytes_downloaded`, `duration`, `avg_speed_bps`, `recycle_count`, `audio_format_produced`, `checksum_verified`, `checksum_algo`, `checksum_digest`, `adb_synced`, `error`, `exit_code`} — `app/commands.py:1095` | W4-1 | NEW GOLDEN |
| E57 | `target_classified` {`target`, `target_type`, `clean_url`, `filename_hint`, `content_length`, `accept_ranges`} — `app/commands.py:837`, `inspector.py:33` | W1-4 | NEW GOLDEN |
| E58 | `throttle_launchd_plist_generated` {`path`, `label`, `lint`} — RAW `json.dumps` at `throttle/cli.py:296–308` (no `timestamp`/redaction) | W2-1/W2-4 | NEW GOLDEN; **fix to single emitter** |
| E59 | `verification` {`file`, `algorithm`, `expected`, `computed`, `verified`, `source`} + variant {`file`, `verified`, `source`, `error`} — 10 call sites | W1-2/W2-2 | `test_rust_pipeline_e2e.py:96` |
| E60 | `watchdog_action` {`action`,`reason`} / {`action`,`reason`,`supports_range`} / full {`cycle`, `mode`, `action`, `resilience_event`, `reason`, `speed_bps`, `threshold_bps`, `rolling_p90_bps`, `connections`} — `engines/aria2.py:987,1387,1451`, `engines/rust_engine.py:675` | W1-3 | `test_watchdog.py`, `tests/e2e/test_slow_guards.py:190` |
| E61 | `when_fast_probe` {`url`, `attempt`, `measured_bps`, `threshold_bps`, `passed`} — `resilience/queue.py:147` | W2-4 | NEW GOLDEN |

> Reconciliation of "~48": source has **44 distinct event names** (43 `emit_event` literals + 1 raw dict). The doc figure ~48 counts 4 additional *variants* (`result` A/B, `verification` std/error, `watchdog_action` 3 shapes → +4 beyond the name). `resilience_event` (seeded E13) is a **field value** inside `watchdog_action`, not a standalone event; the WAKE/NET_CHANGE/STALL enum lives at `resilience/events.py:167,189,212`.

---

## 4. Config fields / env vars

24 `PDLConfig` dataclass fields (`core/config.py:56–81`), plus env vars. Seeded G01–G05 preserved.

| Row | Field / env | Owning wave | Proving test |
|---|---|---|---|
| G01 | `config.json` fields (full set enumerated by W0-4) | W0-3 | `test_config.py` |
| G02 | `PARADL_HOME` isolation | W0-3 | `test_config.py` |
| G03 | `PARADL_MEDIA_PY` (media python override) | W3-4b | handshake tests |
| G04 | `HF_TOKEN` / `GITHUB_TOKEN` env precedence | W2-4 | auth tests |
| G05 | `PARADL_ASSERT_NO_PYTHON=1` (parity guard) | W4-2 | non-media lane |
| G06 | `schema_version` (int, =2 `CONFIG_SCHEMA_VERSION`; forced on load/save) | W0-3 | `test_config.py` |
| G07 | `connections` (int, 1–16; clamped `min(n,16)` on validate) | W1-2/W0-3 | `test_config.py`, rapids tests |
| G08 | `output_dir` (str, default `"."`) | W0-3 | `test_config.py` |
| G09 | `audio_codec` (∈ `VALID_AUDIO_CODECS`, default `"aac"`) | W3-4/W0-3 | `test_config.py`, media golden |
| G10 | `video_quality` (∈ `VALID_VIDEO_QUALITIES`, default `"best"`) | W3-4/W0-3 | `test_config.py`, media golden |
| G11 | `auto_extract` (bool, default false) | W2-2/W0-3 | `test_config.py` |
| G12 | `auto_adb_push` (bool, default false) | W2-2/W0-3 | `test_config.py` |
| G13 | `bin_dir` (str, default `~/.local/bin` or `%LOCALAPPDATA%/Programs/pdl/bin`) | W0-1/W0-3 | `test_config.py` |
| G14 | `adb_music_dir` (str, default `/storage/emulated/0/Music`) | W2-2/W0-3 | `test_config.py` |
| G15 | `adb_download_dir` (str, default `/storage/emulated/0/Download`) | W2-2/W0-3 | `test_config.py` |
| G16 | `max_download_limit` (rate str\|null; `parse_rate`) | W1-2/W0-3 | `test_config.py` |
| G17 | `history_limit` (int ≥1; dataclass default 50, CLI `--history-limit` default 15) | W1-1/W0-3 | `test_config.py` |
| G18 | `anti_throttle` (rate str\|null, default `"4MBps"`) | W1-3/W0-3 | `test_config.py`, `test_watchdog.py` |
| G19 | `anti_throttle_grace` (int, clamped `max(n,3)`; default 5) | W1-3/W0-3 | `test_config.py` |
| G20 | `watchdog` (∈ {v1,v2,off}, default `"v2"`) | W1-3/W0-3 | `test_watchdog.py` |
| G21 | `ytdlp_player_client` (str\|null) | W3-4/W0-3 | `test_config.py` |
| G22 | `caffeinate` (bool, default true) | W2-4/W0-3 | `test_config.py` |
| G23 | `net_watch` (bool, default true) | W2-4/W0-3 | `test_config.py` |
| G24 | `stall_timeout` (float ≥0, default 30.0; 0 disables) | W2-4/W0-3 | `test_config.py` |
| G25 | `require_ac` (bool, default false) | W2-4/W0-3 | `test_config.py` |
| G26 | `pause_on_low_power` (bool, default false) | W2-4/W0-3 | `test_config.py` |
| G27 | `isp` (str, default `"auto"`; legacy `"Duronto Internet"` migrated to `"auto"`) | W2-1/W0-3 | `test_config.py` |
| G28 | `doh_endpoint` (http/https URL\|null; implement via hickory-dns) | W1-4/W0-3 | DoH test |
| G29 | `engine` (∈ {rust,aria2}, default `"rust"`; also `_active_engine` global) | W3-1/W0-3 | `test_perf_parity.py` |
| G30 | `HF_TOKEN` / `HUGGING_FACE_HUB_TOKEN` (HF auth precedence: env → keychain → none) | W2-4 | auth tests; `ui/display.py:28` redaction keys |
| G31 | `GITHUB_TOKEN` / `GH_TOKEN` (GitHub auth precedence; `engines/github.py:82`) | W2-4 | auth tests; `test_platform_windows.py` |
| G32 | `LOCALAPPDATA` (Windows `bin_dir` root; `core/config.py:50`) | W0-1 | `test_platform_windows.py` |
| G33 | `PARADL_GITHUB_ARCHIVE_MIRROR` (archive mirror prefix, default `gh-proxy.com`; `net/accelerate.py:40`) | W2-3 | `test_github_acceleration.py` |
| G34 | `VIRTUAL_ENV` (PyPI install target; `engines/pypi.py`) | W2-3 | `test_pypi_transitive.py` |
| G35 | `NONINTERACTIVE` (brew install/update; `cli/dispatch.py:105,122`) | W0-1 | install scripts test |
| G36 | `HOMEBREW_NO_AUTO_UPDATE` (brew pour; install scripts) | W0-1 | install scripts test |
| G37 | `PYO3_PYTHON` (build-time only, legacy `paradl-rs`; new crates must NOT depend on PyO3) | W0-1 | `cargo test --all` with PYO3_PYTHON set |

---

## 5. DB tables / schema

All tables live in the single `$PARADL_HOME/history.db` (WAL, `synchronous=NORMAL`, `HISTORY_SCHEMA_VERSION=4` stored in `schema_meta`). **10 physical tables** = 7 "history" (incl. shared `queue_jobs`) + 3 throttle-owned. Seeded D01–D05 preserved.

| Row | Table / property | Owning wave | Proving test |
|---|---|---|---|
| D01 | `history.db` schema v4, WAL | W1-1 | `history.db` opens/migrates/round-trips |
| D02 | 7 history tables | W1-1 | W1-1 tests |
| D03 | 3 throttle tables (shared file) | W1-1/W2-1 | W1-1/W2-1 tests |
| D04 | `queue_jobs` (shared) | W1-1/W2-4 | queue round-trip |
| D05 | device+inode guard | W1-1 | `test_sqlite_guard.py` |
| D06 | `schema_meta` (`key TEXT PK`, `value TEXT NOT NULL`); stores `schema_version='4'` and `json_migrated` — `history.py:191–196,339–356` | W1-1 | `test_sqlite_guard.py` |
| D07 | `downloads` (`id INTEGER PK AUTOINCREMENT`, `timestamp`, `target`, `type`, `filename`, `path`, `size`, `duration`, `success`, `adb`, `sha256`) — `history.py:199–211` | W1-1 | `history` round-trip |
| D08 | `host_strategies` (`host TEXT PK`, `best_action`, `connections`, `ip_family`, `speed_before`, `speed_after`, `supports_range`, `throttle_class`, `updated_at`, `avg_rtt_ms`, `packet_loss_pct`) — `history.py:216–228` | W1-1 | W1-1 tests |
| D09 | `learned_profiles` (PK `(host,hour_bucket,network_id)`; `connections`, `ip_family`, `recovery_action`, `speed_before`, `speed_after`, `supports_range`, `throttle_class`, `updated_at`, `updated_epoch`, `source`; migrated `avg_rtt_ms`, `packet_loss_pct`) — `history.py:248–284` | W1-1/W2-1 | W1-1/W2-1 tests |
| D10 | `http_cache` (`url TEXT PK`, `etag`, `body_json`, `updated_at`) — `history.py:287–293` | W2-3 | GitHub/HF/IPSW ETag golden |
| D11 | `host_caps` (`host TEXT PK`, `cap INTEGER`, `seen INTEGER`, `updated_epoch REAL`) — `history.py:297–303` | W1-1 | W1-1 tests |
| D12 | `queue_jobs` (22 cols: `id`, `target`, `target_type`, `out_dir`, `filename`, `mirrors_json`, `priority`, `connections`, `status`, `require_ac` [default 1], `pause_on_low_power`, `when_fast_bps`, `start_at`, `expected_length`, `expected_digest`, `category`, `bytes_downloaded`, `error`, `options_json`, `created_at`, `updated_at`) — `history.py:307–336` | W2-4 | queue round-trip |
| D13 | `throttle_samples` (`id`, `timestamp`, `epoch`, `hour`, `is_weekend`, `interface` [default `en0`], `ssid`, `network_id`, `source`, `target_group`, `host`, `download_bps`, `upload_bps`, `rtt_ms`, `bytes_used`, `series_json`) — `store.py:66–83` | W2-1 | `test_classifier.py` |
| D14 | `throttle_runs` (`id`, `run_id TEXT UNIQUE`, `mode`, `isp`, `created_at`, `created_epoch`, `budget_bytes`, `bytes_used`, `report_json`) — `store.py:88–99` | W2-1 | `report.json` golden |
| D15 | `asn_cache` (`ip TEXT PK`, `asn`, `country`, `org`, `updated_epoch`; 7-day max age) — `store.py:103–109` | W2-1 | M10 evidence golden |
| D16 | WAL pragmas on every new connection (`journal_mode=WAL`, `synchronous=NORMAL`) — `history.py:175–176` | W1-1 | `test_sqlite_guard.py` |
| D17 | Once-per-process schema guard keyed by `(resolved path, st_dev, st_ino)` for both `HistoryManager` and `ThrottleStore` — `history.py:131–143`, `store.py:36–60` | W1-1 | `test_sqlite_guard.py` (CREATE 85→10 assertion) |
| D18 | Corrupt DB rotation to `history.db.bad` + removal of `-wal`/`-shm` sidecars — `history.py:153–167` | W1-1 | W1-1 tests |
| D19 | Read-only legacy `history.json` → `downloads` one-time migration (`json_migrated` flag); never deletes `history.json` — `history.py:347+` | W1-1 | W1-1 tests |
| D20 | `ALTER TABLE` migrations: `host_strategies` +avg_rtt_ms/+packet_loss_pct; `learned_profiles` +source/+avg_rtt_ms/+packet_loss_pct; `queue_jobs` +category — `history.py:231–245,266–284,332–338` | W1-1 | `test_sqlite_guard.py` |

---

## 6. Contract literals (byte-exact)

Seeded L01–L15 preserved; L16+ enumerate the remaining exact strings/numbers/formats/domain lists.

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
| L16 | `ExitCode` names/values (IntEnum, `models.py:39–45`): `OK=0`, `FAILURE=1`, `USAGE=2`, `VERIFICATION_FAILED=3`, `MISSING_DEPENDENCY=4`, `INTERRUPTED=130` | W0-3 | `test_models.py`, full mapping test |
| L17 | `TargetType` strings (StrEnum, `models.py:21–31`): `torrent`, `direct`, `stream`, `search`, `github`, `huggingface`, `ipsw`, `pypi`, `brew` | W1-4/W2-3 | routing oracle |
| L18 | `WatchdogMode` strings: `v1`, `v2`, `off` (`models.py:33–36`) | W1-3 | `test_watchdog.py` |
| L19 | `VALID_ENGINES = {"rust","aria2"}`; default engine `"rust"` (`config.py:25,31,81`) | W3-1 | `test_perf_parity.py` |
| L20 | `VALID_AUDIO_CODECS = {lossless,native,mp3,flac,opus,wav,aac}`; `VALID_VIDEO_QUALITIES = {best,4k,1440p,1080p,720p,480p}` (`config.py:22–23`) | W3-4/W0-3 | `test_config.py` |
| L21 | `parse_rate` regex `^\s*([0-9]+(?:\.[0-9]+)?)\s*([kKmMgGtT])?(i)?(B\|b\|byte\|bytes\|bit\|bits)?(?:\s*(?:ps\|/s(?:ec)?))?\s*$`; binary multipliers (1 K = 1024) — `models.py:174–184` | W1-2/W0-3 | `test_models.py` |
| L22 | `SUPPORTED_ALGOS = ("md5","sha1","sha256","sha512","blake2b")` (`checksum.py:19`); companion suffixes `.sha256`, `.sha256sum`, `.sha512`, `.md5` (`checksum.py:168–170`) | W2-2 | checksum tests |
| L23 | `REPORT_SCHEMA_VERSION = "1.0"` (`throttle/models.py:9`); module keys `m0_sanity`, `m1_baseline`, `m2_single_flow`, `m3_flow_scaling`, `m4_destination_independence`, `m5_burst_bucket`, `m6_policing_shaping`, `m7_protocol_port`, `m8_time_volume_upload`, `m9_ground_truth`, `m10_local_vs_intl` (`models.py:492–504`) | W2-1 | `report.json` golden |
| L24 | 12 seeded deterministic cases `@pytest.mark.parametrize("seed", list(range(12)))` (`tests/unit/test_classifier.py:60`) | W2-1 | `test_classifier.py` |
| L25 | `classify_audio_production` exact strings (`media.py:60–72`): `"passthrough AAC (from {src} source, no re-encode)"`, `"true lossless ALAC (from lossless {src} source)"`, `"ALAC-wrapped lossy (transcoded from lossy {src} source)"`, `"true lossless FLAC (from {src} source)"`, `"FLAC (transcoded from lossy {src} source)"`, `"uncompressed WAV PCM (from {src} source)"`, `"{CODEC} (from {src} source)"` | W3-4 | media golden fixture |
| L26 | `sanitize_filename`: `name.strip()[:200]`, Windows reserved stem check (`models.py:14,248–255`) | W0-3/W3-4 | `test_models.py` golden |
| L27 | `MEDIA_DOMAINS` (14, hostname-suffix match, `models.py:111–126`): youtube.com, youtu.be, soundcloud.com, vimeo.com, twitch.tv, bandcamp.com, bilibili.com, dailymotion.com, mixcloud.com, facebook.com, instagram.com, tiktok.com, twitter.com, x.com | W1-4/W3-4 | routing oracle |
| L28 | `IMAGE_DOMAINS` (5, `models.py:129–135`): image.pollinations.ai, images.unsplash.com, i.imgur.com, i.redd.it, cdn.discordapp.com + pollinations.ai heuristic (`"image" in host or "/prompt/" in url`) | W1-4 | routing oracle |
| L29 | `DIRECT_EXTENSIONS` (~59 suffixes, `models.py:49–108`): .zip .tar .gz .tgz .bz2 .tbz2 .xz .txz .7z .rar .iso .img .dmg .pkg .deb .rpm .apk .ipa .ipsw .bin .exe .msi .appimage .safetensors .gguf .pt .pth .onnx .ckpt .parquet .csv .jsonl .mp4 .mkv .avi .mov .webm .flv .mp3 .m4a .flac .wav .opus .aac .ogg .pdf .epub .doc .docx .png .jpg .jpeg .webp .gif .svg .whl .meta4 .metalink | W1-4/W2-3 | routing oracle |
| L30 | Search clean URL prefix `ytsearch1:{target}` (`net/client.py:853`) | W1-4 | routing oracle |
| L31 | HEAD/Range `text/html` → `TargetType.STREAM` (`net/client.py:801–802,827–828`) | W1-4 | routing oracle |
| L32 | `WHERE_FROMS_ATTR = "com.apple.metadata:kMDItemWhereFroms"` (binary plist via `xattr -w -x`); `QUARANTINE_ATTR = "com.apple.quarantine"` (`platform/macos.py:799,867`) | W2-2 | `macos_fake.py` golden |
| L33 | `codesign --verify --verbose`, `spctl --assess --type execute --verbose`, `codesign --force --sign -` (ad-hoc only with `--sign`) (`platform/macos.py:870–949`) | W2-2 | `macos_fake.py` golden |
| L34 | `Zone.Identifier` INI: `[ZoneTransfer]` + `ZoneId=<int>` + `HostUrl=<url>`, joined with `"\r\n"` and trailing `"\r\n"` (`platform/windows.py:376–396`); ADS path `<filepath>:Zone.Identifier` (`:399–400`) | W2-2 | `test_platform_windows.py` |
| L35 | Keychain service `"pdl"`, accounts `"hf"\|"github"`; `security -i` over stdin `add-generic-password -a <service> -s pdl -U -w <token>` (zero argv leak) (`auth/manager.py`, `engines/{github,hf}.py`) | W2-4 | keychain golden |
| L36 | `clean` orphan patterns: `.aria2`, `.part`, `.ytdl`, `.pdl_raw_*` (`commands.py` clean handlers; DEV_CONTEXT §4) | W4-1 | parity non-media lane |
| L37 | Default paths: `PARADL_HOME` → `~/.config/paradl` (or `%LOCALAPPDATA%/paradl`); files `config.json`, `mirrors.json`, `history.db`, `logs/`, `hooks/{on_complete,on_fail}`, `throttle/` (`core/paths.py`) | W0-3/W2-4 | `test_config.py` |
| L38 | Guard constants: `STALE_PROFILE_DAYS=30.0` (2,592,000 s), `HOST_CAP_DECAY_SEC=600.0`, `HOST_CAP_MIN_OBS=2`, `SAMPLE_IDEMPOTENCY_WINDOW_SEC=45*60` (`history.py:24–27`, `store.py:30`) | W1-1/W2-1 | W1-1/W2-1 tests |
| L39 | power guard: free disk `>= remaining + margin`, `margin = max(5% remaining, min(10% remaining, 64 MiB), 1 KiB)` → `ExitCode.FAILURE(1)`; `--require-ac` on battery → 1 (`DEV_CONTEXT §3`) | W2-4 | guard tests |
| L40 | Watchdog v2 constants: `threshold = max(user_floor, 0.5*rolling_p90)` at ≥3 samples; `grace_period = 5s` (min 2); 5 low samples / 5s periodic; 2s min cooldown; 4-hour hour buckets `00-03..20-23`; staleness 70% of `speed_after` (`DEV_CONTEXT §3`) | W1-3 | `test_watchdog.py` |

> F08/F09/F10/F11 in §2 similarly record target-Rust flags absent from today's Python parser; they stay as acceptance rows, not as claims about current source.

---

## 7. Media protocol `paradl-media/1` fields (authoritative: charter §4)

Seeded P01–P12 preserved; P13+ enumerate every field of all four frame types plus handshake rules.

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
| P13 | Request `target` (`<url-or-search>`), `out_dir` (`<absolute path>`) | W3-4 | handshake contract |
| P14 | Request `options`: `{audio_only: bool, format: null\|str, cookies_from_browser: null\|str}` | W3-4 | handshake contract |
| P15 | Progress frame fields: `downloaded_bytes` (int), `total_bytes` (null\|int), `speed_bps` (int), `eta_seconds` (null\|float) | W3-4b | boundary lane |
| P16 | Terminal result `ok: true`; `data`: `{filename, http_headers, audio_format_produced, paths}` | W3-4b | media golden replay |
| P17 | Terminal error `ok: false`; `error`: `{kind: metadata\|download\|conversion\|dependency\|usage\|interrupted, message}` | W3-4b | boundary lane error cases |
| P18 | Exact versions keys: `worker_version`, `yt_dlp_version`, `mutagen_version`, `ffmpeg_version` (e.g. `0.1.0`, `2026.10.26`, `1.48.1`, `7.0`) | W3-4a/b | `doctor` + handshake |
| P19 | `caps` array values: `ejs`, `mutagen`, `ffmpeg` | W3-4a/b | handshake contract |
| P20 | `error.kind` → ExitCode: `metadata\|download\|conversion`→1, `dependency`→4, `usage`→2, `interrupted`→130; worker NEVER emits 3 | W3-4b | boundary lane error cases |
| P21 | Handshake: both request and every response frame carry `protocol` + `request_id` | W3-4 | handshake contract |
| P22 | Handshake: `versions` present + parseable in terminal frames; missing/unparseable → exit 4 | W3-4b | boundary lane malformed |
| P23 | Handshake: yt-dlp version is DATA not protocol; only schema changes bump protocol version | W3-4a/b | version-compat matrix |
| P24 | Contract test matrix N, N−1, N+1, unknown, missing | W3-4b/W4-2 | boundary lane version-compat |
| P25 | Transport: one-shot worker `<python> -m paradl_media` per op (NO sidecar, NO PyO3) | W3-4b | process guard |
| P26 | Single-emitter rule: worker progress frames re-emitted by Rust `paradl-events` with redaction | W2-4/W3-4b | single-emitter integrity test |

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

---

## 9. W5-2 post-cutover provability note (informational — lead owns final sign-off)

The W5-1 cutover deleted the Python oracle (`src/paradl/**`, bulk `tests/**`,
`parallel-dl.py`). Row provability after cutover:

- **Non-media differential rows** (C01–C52 where "parity non-media lane" is the
  proving test, plus X05 `PARADL_ASSERT_NO_PYTHON=1`) are proven by the final
  GREEN run captured at commit `c4e271a`: exit `0`, 6/6 cases CLEAN. The harness
  test `full_non_media_gate_is_clean` is now `#[ignore]`d because it invokes the
  deleted oracle; it is **retired historical evidence**, not a re-runnable gate.
- **Boundary lane rows** P02, P04, P06, P08, P09, P10, P15, P17, P20, P22 stay
  fully live and oracle-free; `cargo run -p paradl-parity -- boundary --case
  version-mismatch` exits `4`. Protocol matrix N / N−1 / N+1 / unknown / missing
  is covered by `protocol_version_matrix_matches_oracle` and the boundary tests.
- **Media golden-replay rows** P03, P16 stay live because `media/` was retained:
  `PYTHONPATH=media .venv/bin/python -m pytest media/tests -q`.
- **X04 / C20** (`parallel-dl.py`, dual binary names): `parallel-dl.py` is
  deleted; the native `paradl` binary owns both `paradl` and `parallel-dl`.
- **C18 / E21** (`bench` subcommand / bench event): the subcommand remains
  implemented, but the two *Python benchmark harnesses* that compared against
  the deleted oracle are retired — they emit `{"value": null, "reason": "..."}`
  with all honesty/drift caveats preserved instead of crashing on stale imports.
- **X01 / X03 / X06 / X07** remain provable from the tree, `media/pyproject.toml`,
  the process guard, and the Formula/install scripts respectively.
