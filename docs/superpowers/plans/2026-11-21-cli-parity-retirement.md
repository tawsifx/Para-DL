# CLI Parity Flag Retirement Implementation Plan
> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
**Goal:** Retire the accepted-then-discarded CLI parity flags (`--accept-ranges`, `--total`, `--http3`/`--no-http3`) and the duplicate `--install`/`--info`/`--hash` long forms, and amend the parity contract deliberately without losing the retained `--http3-probe` (F59) or the public HTTP/3 compat shim.
**Architecture:** Workstream D of the post-review hardening design; it runs last (B → A → C → D) because it deliberately edits golden help fixtures. The CLI crate stays the single owner of parsing/routing; `paradl-app` keeps the only two consumers of the removed HTTP/3 namespace (`run_engine` and `probe_live`), so this workstream is a small, compile-coupled change across `paradl-cli`, `paradl-app`, the byte-exact fixtures, the parity matrix, and its audit test. No media protocol, event schema, or engine API changes are in scope.
**Tech Stack:** Rust 2024, clap 4 (derive + `override_help`), `paradl-core::Error/ExitCode`, `paradl-events::NetprobeEvent` (macro-generated `new`), `paradl-engine::Target`, Cargo test, byte-exact `include_str!` fixtures.
**Spec:** docs/superpowers/specs/2026-11-21-post-review-hardening-design.md

---

## Global Constraints

- `rust/Cargo.toml:20`: "New crates MUST NOT depend on PyO3."
- `docs/superpowers/framework-charter.md:38`: "Default feature set compiles WITHOUT
  optional system tools."
- `docs/superpowers/framework-charter.md:172`: "single-emitter rule: `paradl-events` is
  the only JSONL emitter."
- `docs/superpowers/framework-charter.md:205`: "cargo doc --no-deps clean: no missing
  docs on public items."
- `docs/superpowers/parity-matrix.md:429+`: byte-exact parity with the deleted Python
  oracle is the acceptance contract; both Python benchmark harnesses are "retired"
  and must emit `null`+reason instead of crashing.
- Release profile is fixed: `opt-level = 3`, `lto = "thin"`, `codegen-units = 1`.

Additional decisions locked by the spec (not open questions): `--http3-probe` (F59) is retained; `rust/crates/paradl-net/src/compat.rs` is retained because it is publicly re-exported and used by `rust/crates/paradl-net/tests/routing_oracle.rs` and `rust/crates/paradl-net/examples/resolve_target.rs`; `NetprobeEvent.http3_supported` stays and is derived from `http3_probe` alone; subcommands remain canonical for install/info/hash.

## Review Focus

1. **Two hidden `effective_http3()` call sites in `paradl-app`.** `rust/crates/paradl-app/src/app.rs:679` (`run_engine`, sets `paradl_engine::Target.http3`) and `:774` (`probe_live`, sets `NetprobeEvent.http3_supported`) are the only consumers. Deleting the method or the `Cli` fields without rewiring both breaks the whole workspace; rewiring `:774` incorrectly breaks C17/F59. Pin: `cargo test -p paradl-app --test dispatch` (the new C17/F59 guard passes `--http3-probe` and asserts `"http3_supported":` is emitted) plus the new `http3_probe_flag_is_retained` CLI test below.
2. **The byte-exact usage cascade.** `HELP_TEXT` feeds `usage_block()`; changing it invalidates the hard-coded `assert_eq!(usage.len(), 1978)` at `oracle_parity.rs:441` and the prefix of all three `error_*.txt` goldens. Pin: `help_text_is_byte_exact_oracle_output`, `argparse_error_output_is_byte_exact_for_reviewed_cases`, `argparse_error_output_is_byte_exact_for_additional_choices_and_types`, and `cargo run -q -p paradl -- --help | diff -u rust/crates/paradl-cli/tests/fixtures/help.txt -`.
3. **`--hash` still has a fast path in dispatch.** `rust/crates/paradl-cli/src/dispatch.rs:312` reads `cli.hash_file` when there are no positionals, and `oracle_parity.rs::download_and_hash_fast_paths_classify` asserts `--hash local.bin` → `Command::Hash`. Removing the field without removing that branch leaves dead code or a compile error. Pin: rewritten `download_and_hash_subcommand_classify` asserting `hash local.bin` → `Command::Hash` and `--hash local.bin` → `ExitCode::Usage`; `install_info_extract_history_clean_classify` and `every_parity_subcommand_is_recognised` keep F68/F69/F70 canonical via subcommands.
4. **The audit loop enumerates F01..=F92 regardless of matrix edits.** Deleting rows from `parity-matrix.md` alone leaves `every_cli_flag_row_has_a_proving_path` asserting proofs for retired rows; the loop and `proving_path` must both be amended. Pin: new `retired_cli_flag_rows_are_not_audited` in `rust/crates/paradl-app/tests/parity.rs`.
5. **The compat shim looks unused after CLI removal and is tempting to delete.** `note_http3_drop`/`HTTP3_DROP_REASON` are still public API used by `routing_oracle.rs:25,493-495` and `examples/resolve_target.rs:19,78`; deleting them breaks those consumers and contradicts the spec. Pin: `cargo test -p paradl-net --test routing_oracle` (existing shim assertions) and the new `http3_compat_shim_is_still_public_api` guard added in Task 4.

---

### Task 1: Remove the retired flags from the parser and its Rust consumers

**Files:**
- Modify: `rust/crates/paradl-cli/src/parser.rs`
- Modify: `rust/crates/paradl-cli/src/dispatch.rs`
- Modify: `rust/crates/paradl-app/src/app.rs`
- Test: `rust/crates/paradl-cli/tests/oracle_parity.rs`
- Test: `rust/crates/paradl-app/tests/dispatch.rs`

**Interfaces:**
- Consumes: `Cli::parse_command_line(argv: Vec<String>) -> Result<ParseOutcome, Error>`; `Command::from_cli(cli: &Cli) -> Result<Command, Error>`; `App::handle(&mut self, command: &Command, cli: &Cli) -> ExitCode`; `NetprobeEvent::new(timestamp, url, host, supports_range, ipv6_dns, throttle_class, recommended_connections, recommended_action, verdict, http3_supported, http3_detail, passes)`.
- Produces: `Cli` without fields `http3`, `no_http3`, `accept_ranges`, `total`, `install_target`, `info_target`, `hash_file` and without method `effective_http3(&self) -> bool`; `Cli.http3_probe: bool` retained; `Command::from_cli` with no `--hash` fast path; `probe_live` deriving the 10th `NetprobeEvent::new` argument from `cli.http3_probe` alone; `paradl_engine::Target.http3` left at its `Default` (`false`) on the CLI path.

- [ ] Write the failing tests in `rust/crates/paradl-cli/tests/oracle_parity.rs` (add both, and rewrite `download_and_hash_fast_paths_classify` as below). Do not touch source yet.
  ```rust
  #[test]
  fn retired_flags_are_rejected() {
      for flag in ["--http3", "--no-http3", "--accept-ranges", "--total",
                   "--install", "--info", "--hash"] {
          let argv = vec![flag.to_string(), "x".to_string()];
          let err = Cli::parse_command_line(argv).expect_err("retired flag must not parse");
          assert_eq!(err.exit_code(), ExitCode::Usage, "argv: {flag}");
          assert!(err.to_string().contains("unrecognized arguments:"), "{flag}: {err}");
      }
  }

  #[test]
  fn http3_probe_flag_is_retained() {
      let cli = parse(&["netprobe", "https://example.com/f", "--http3-probe"]);
      assert!(cli.http3_probe, "F59 flag must survive");
      assert!(!parse(&["https://example.com/f"]).http3_probe);
  }

  #[test]
  fn download_and_hash_subcommand_classify() {
      let cli = parse(&["https://example.com/f.zip"]);
      assert!(matches!(Command::from_cli(&cli).unwrap(), Command::Download(_)));
      let cli = parse(&["hash", "local.bin"]);
      assert!(matches!(Command::from_cli(&cli).unwrap(), Command::Hash(h) if h.file == "local.bin"));
  }
  ```
- [ ] Verify FAILS: `cd rust && cargo test -p paradl-cli retired_flags_are_rejected -- --exact` (currently the flags parse, so `expect_err` panics). Also confirm `cargo test -p paradl-cli http3_probe_flag_is_retained` passes — it is a guard, not the RED.
- [ ] Implement minimal code:
  - `parser.rs`: delete the `#[arg(long, ...)]` fields `http3` (lines 290-292), `no_http3` (293-295), `install_target` (319-321), `info_target` (322-324), `hash_file` (325-327), `accept_ranges` (391-393), `total` (394-396), and the `effective_http3` method (518-521). Keep `http3_probe` (287-289), `doh_endpoint`, `headers`, `cookies`, and the `--install/--info/--hash` *subcommand* spellings in `SUBCOMMANDS`. Update the module doc (lines 8-12) to drop the `--http3`/`--no-http3` example, and change the "Dropped/legacy flags kept compatible" comment so it no longer names the two removed flags.
  - `dispatch.rs`: in `Command::from_cli` replace the `let Some(first) = targets.first() else { if let Some(file) = &cli.hash_file { ... } return Ok(Self::Download(cli.clone())); };` block with `let Some(first) = targets.first() else { return Ok(Self::Download(cli.clone())); };`. Leave `SUBCOMMANDS`, `USAGE_HASH`, and the `"hash"` match arm unchanged.
  - `app.rs`: delete the `http3: cli.effective_http3(),` line from `run_engine` (the `..EngineTarget::default()` already supplies `http3: false`); change `probe_live` line 774 from `cli.http3_probe && cli.effective_http3()` to `cli.http3_probe`. Keep `NetprobeEvent.http3_supported` and every other `NetprobeEvent::new` argument byte-identical.
  - `paradl-app/tests/dispatch.rs`: add this guard next to the C17 case (it already passes before the change; it pins the field's survival during the refactor):
    ```rust
    #[test]
    fn netprobe_http3_probe_flag_is_retained_in_event() {
        let (code, output) = dispatch(&[
            "netprobe", "https://example.com/f", "--http3-probe", "--json",
        ]);
        assert_eq!(code, ExitCode::Ok);
        assert!(output.contains("\"http3_supported\":"), "F59 field must survive: {output}");
    }
    ```
  - `oracle_parity.rs`: in `defaults_match_pdlconfig_oracle` replace `assert!(cli.http3);` with `assert!(!cli.http3_probe);`. In `dropped_and_alias_flags_are_accepted` remove `--accept-ranges`, `--total`, `--http3` and their three assertions, and in its second `parse` remove `--no-http3` and `assert!(!cli.effective_http3());`; rename the test `alias_flags_are_accepted`. Leave the `invalid` list in `usage_errors_map_to_exit_code_two` unchanged; the dedicated rejection test above already covers the retired flags.
- [ ] Verify PASS: `cd rust && cargo test -p paradl-cli && cargo test -p paradl-app --test dispatch` (all tests, including the three new/rewritten ones and the C17/F59 guard).
- [ ] Run suite: `cd rust && cargo test -p paradl-app && cargo test --all -- --skip full_non_media_gate_is_clean`.
- [ ] Commit: `git add rust/crates/paradl-cli/src/parser.rs rust/crates/paradl-cli/src/dispatch.rs rust/crates/paradl-app/src/app.rs rust/crates/paradl-cli/tests/oracle_parity.rs rust/crates/paradl-app/tests/dispatch.rs && git commit -m "cli: retire accepted-then-discarded parity flags"`.

---

### Task 2: Update the byte-exact help and error goldens

**Files:**
- Modify: `rust/crates/paradl-cli/tests/fixtures/help.txt`
- Modify: `rust/crates/paradl-cli/tests/fixtures/error_unknown_arg.txt`
- Modify: `rust/crates/paradl-cli/tests/fixtures/error_engine_bogus.txt`
- Modify: `rust/crates/paradl-cli/tests/fixtures/error_connections_abc.txt`
- Test: `rust/crates/paradl-cli/tests/oracle_parity.rs`
- Verify only (expected no diff): `rust/crates/paradl-cli/tests/fixtures/throttle_help.txt` — it contains none of the retired flags.

**Interfaces:**
- Consumes: `HELP_TEXT: &str` (`include_str!("fixtures/help.txt")`), `usage_block() -> &'static str`, `argparse_error(err: &clap::Error, argv: &[String]) -> String`.
- Produces: new `HELP_TEXT` bytes with retired usage tokens/option lines removed, new `usage_block()` byte length, and three regenerated `error_*.txt` goldens equal to `new_usage_block + unchanged_error_suffix`; `throttle_help.txt` byte-identical.

- [ ] Write the failing test in `rust/crates/paradl-cli/tests/oracle_parity.rs` (source/fixtures untouched):
  ```rust
  #[test]
  fn help_does_not_advertise_retired_flags() {
      let usage = usage_block();
      for token in ["[--http3]", "[--no-http3]", "[--install ", "[--info ", "[--hash "] {
          assert!(!usage.contains(token), "usage block still lists {token}");
      }
      assert!(HELP_TEXT.contains("--http3-probe"), "F59 flag must stay documented");
      for line in HELP_TEXT.lines().map(str::trim_start) {
          for stale in ["--http3 ", "--no-http3 ", "--install ", "--info ", "--hash "] {
              assert!(!line.starts_with(stale), "option line still lists {stale}: {line}");
          }
      }
  }
  ```
- [ ] Verify FAILS: `cd rust && cargo test -p paradl-cli help_does_not_advertise_retired_flags -- --exact`.
- [ ] Implement minimal code:
  - Edit `help.txt`: remove `[--http3] [--no-http3]` from the usage lines (currently lines 18-19) and `[--install INSTALL_TARGET] [--info INFO_TARGET] [--hash HASH_FILE]` (lines 21-22), reflowing the remaining tokens with the same 19-space continuation indent and preserving token order. Delete the option-description entries for `--http3`, `--no-http3` (131-133) and `--install`, `--info`, `--hash` (148-153). Keep `--http3-probe` (129-130), all recipes (192-208), and the trailing recipe whitespace line. `--accept-ranges`/`--total` never appeared in this fixture — do not add or remove anything for them.
  - Recompute the usage-block length literal. First capture the old prefix, then from `rust/` run:
    ```bash
    python3 - <<'PY'
    from pathlib import Path
    fx = Path("crates/paradl-cli/tests/fixtures")
    new_usage = (fx / "help.txt").read_text()
    new_usage = new_usage[:new_usage.find("\n\n") + 1]
    print("new usage length:", len(new_usage))
    PY
    ```
    Replace `assert_eq!(usage.len(), 1978);` at `oracle_parity.rs:441` with the printed length.
  - Regenerate the three error goldens with the captured old prefix (run from `rust/`):
    ```bash
    python3 - <<'PY'
    import subprocess
    from pathlib import Path
    fx = Path("crates/paradl-cli/tests/fixtures")
    old_help = subprocess.check_output(
        ["git", "show", "HEAD:rust/crates/paradl-cli/tests/fixtures/help.txt"], text=True)
    old_usage = old_help[:old_help.find("\n\n") + 1]
    new_help = (fx / "help.txt").read_text()
    new_usage = new_help[:new_help.find("\n\n") + 1]
    for name in ["error_unknown_arg.txt", "error_engine_bogus.txt", "error_connections_abc.txt"]:
        p = fx / name
        body = p.read_text()
        assert body.startswith(old_usage), name
        p.write_text(new_usage + body[len(old_usage):])
    PY
    ```
    The `parallel-dl: error: ...` suffix of each golden must remain unchanged.
- [ ] Verify PASS: `cd rust && cargo test -p paradl-cli`, then `cd rust && cargo run -q -p paradl -- --help > "$TMPDIR/paradl_help.txt" && diff -u crates/paradl-cli/tests/fixtures/help.txt "$TMPDIR/paradl_help.txt"` (empty diff required).
- [ ] Run suite: `cd rust && cargo test --all -- --skip full_non_media_gate_is_clean`.
- [ ] Commit: `git add rust/crates/paradl-cli/tests && git commit -m "cli: refresh help and error goldens after flag retirement"`.

---

### Task 3: Retire parity rows F09/F11/F60 and skip them in the audit

**Files:**
- Modify: `docs/superpowers/parity-matrix.md`
- Test: `rust/crates/paradl-app/tests/parity.rs`

**Interfaces:**
- Consumes: `proving_path(row: &str) -> Option<&'static str>`, `assert_rows(prefix: char, end: u32)`.
- Produces: `const RETIRED_CLI_FLAG_ROWS: &[&str] = &["F09", "F11", "F60"];`, `fn assert_rows_excluding(prefix: char, end: u32, retired: &[&str])`, an explicit `proving_path` arm returning `None` for retired rows, and `every_cli_flag_row_has_a_proving_path` calling `assert_rows_excluding('F', 92, RETIRED_CLI_FLAG_ROWS)`.

- [ ] Write the failing test in `rust/crates/paradl-app/tests/parity.rs` (do not edit the matrix yet):
  ```rust
  #[test]
  fn retired_cli_flag_rows_are_not_audited() {
      for row in ["F09", "F11", "F60"] {
          assert_eq!(proving_path(row), None, "retired row {row} still claims a proving path");
      }
  }
  ```
- [ ] Verify FAILS: `cd rust && cargo test -p paradl-app --test parity retired_cli_flag_rows_are_not_audited -- --exact` (the `_ if row.starts_with('F')` catch-all currently returns `Some`).
- [ ] Implement minimal code:
  - `parity.rs`: add `const RETIRED_CLI_FLAG_ROWS: &[&str] = &["F09", "F11", "F60"];` near the top; add `"F09" | "F11" | "F60" => None,` as the first match arm in `proving_path` (before the `'F'` catch-all); add `assert_rows_excluding` that loops `1..=end`, `continue`s when `retired.contains(&row.as_str())`, and otherwise asserts `proving_path(&row).is_some()`; change `every_cli_flag_row_has_a_proving_path` to call it with `('F', 92, RETIRED_CLI_FLAG_ROWS)`. Leave the other `assert_rows` callers (`C`, `E`, `G`, `D`, `L`, `P`, `X`) unchanged.
  - `parity-matrix.md` §2: delete rows F09 (line 93), F11 (95), and F60 (144) from the main table. Add a new subsection immediately after that table, before the note, titled `### Retired CLI flag rows (W5-3)` with columns `| Row | Retired flag(s) | Reason |` and exactly three rows: `F09 | --http3 (accepted, logged drop) | retired at W5-3; Python oracle deleted`, `F11 | --accept-ranges, --total | retired at W5-3; Python oracle deleted`, `F60 | --http3 / --no-http3 | retired at W5-3; Python oracle deleted`.
  - Amend the note at `parity-matrix.md:178`: remove the stale sentence's retired aliases (`--accept-ranges`, `--total`, and the F09/F60 HTTP/3 references). Replacement text: `> Seeded F01/F08/F10/F12 describe the *target Rust surface* (aliases \`--output-dir\`, \`--header\`, \`--cookies\`, \`--doh-endpoint\`, \`--audio-only\`, \`--embed-metadata\`, \`--embed-thumbnail\`) that do not all exist verbatim in the current Python parser; they remain as acceptance rows for W3-1/W3-4. F13–F91 are the byte-exact current Python parser surface. F09, F11 and F60 are retired at W5-3; Python oracle deleted — see the retired-rows table above.` Do **not** retire F68/F69/F70: their proving tests exercise the canonical `install`/`info`/`hash` subcommands, which survive. Leave C17/F59 untouched (`netprobe ... [--http3-probe]`, `netprobe.http3_supported` golden).
- [ ] Verify PASS: `cd rust && cargo test -p paradl-app --test parity`.
- [ ] Run suite: `cd rust && cargo test --all -- --skip full_non_media_gate_is_clean`.
- [ ] Commit: `git add docs/superpowers/parity-matrix.md rust/crates/paradl-app/tests/parity.rs && git commit -m "parity: retire F09/F11/F60 and skip them in the flag audit"`.

---

### Task 4: Keep the HTTP/3 compat shim and correct the README docs

**Files:**
- Modify: `rust/crates/paradl-cli/README.md`
- Modify: `rust/crates/paradl-net/README.md`
- Test: `rust/crates/paradl-cli/tests/oracle_parity.rs`
- Test: `rust/crates/paradl-net/tests/routing_oracle.rs`
- Verify only: `README.md` (root) has no retired-flag documentation; do not edit it (the PyO3 contributor note at lines 283-295 belongs to Workstream B).
- Verify only: `rust/crates/paradl-net/src/compat.rs` and its `pub use compat::{HTTP3_DROP_REASON, note_http3_drop};` in `rust/crates/paradl-net/src/lib.rs` — no edits.

**Interfaces:**
- Consumes: `paradl_net::note_http3_drop() -> ()`, `paradl_net::HTTP3_DROP_REASON: &'static str`, and the literal README text currently at `paradl-cli/README.md:10-12` and `paradl-net/README.md:51-52`.
- Produces: docs that no longer claim the canonical CLI accepts `--http3`/`--accept-ranges`/`--total`, still document `--doh-endpoint` and `--http3-probe`, and still describe `note_http3_drop` as a public downstream adapter shim; compile-time guards proving the shim remains public.

- [ ] Write the failing docs test in `rust/crates/paradl-cli/tests/oracle_parity.rs` (leave README files untouched):
  ```rust
  #[test]
  fn docs_do_not_advertise_retired_flags() {
      let cli_readme = include_str!("../README.md");
      let net_readme = include_str!("../../paradl-net/README.md");
      for stale in [
          "the dropped adapter flags `--http3`/`--doh-endpoint`/`--accept-ranges`/`--total`",
          "The legacy `--http3` flag is kept compatible by",
      ] {
          assert!(!cli_readme.contains(stale) && !net_readme.contains(stale), "stale docs: {stale}");
      }
      assert!(cli_readme.contains("--http3-probe"), "F59 must stay documented");
      assert!(net_readme.contains("note_http3_drop"), "public compat shim must stay documented");
  }
  ```
- [ ] Verify FAILS: `cd rust && cargo test -p paradl-cli docs_do_not_advertise_retired_flags -- --exact`.
- [ ] Implement minimal code:
  - `paradl-cli/README.md:10-12`: change the `Cli` bullet to describe the retained surface — all ~60 main flags, `--doh-endpoint`, the retained `--http3-probe` (F59), the target-surface aliases, and throttle flags — and state that `--http3`/`--no-http3`, `--accept-ranges`, `--total` and the `--install`/`--info`/`--hash` long forms are retired at W5-3 (subcommands are canonical).
  - `paradl-net/README.md:51-52`: replace the claim that the canonical CLI accepts `--http3` with: no HTTP/3 feature exists; `note_http3_drop()`/`HTTP3_DROP_REASON` remain a public compatibility shim for downstream adapters (still exercised by `routing_oracle.rs` and `examples/resolve_target.rs`), while the canonical CLI no longer accepts `--http3`.
  - Add this guard (not RED) to `rust/crates/paradl-net/tests/routing_oracle.rs` next to the existing shim assertions at lines 493-495:
    ```rust
    #[test]
    fn http3_compat_shim_is_still_public_api() {
        let _: fn() = paradl_net::note_http3_drop;
        let _: &str = paradl_net::HTTP3_DROP_REASON;
    }
    ```
  - Do not modify `compat.rs`, its re-export, `routing_oracle.rs`'s existing assertions, or the example.
- [ ] Verify PASS: `cd rust && cargo test -p paradl-cli docs_do_not_advertise_retired_flags -- --exact && cargo test -p paradl-net --test routing_oracle`.
- [ ] Run suite: `cd rust && cargo test --all -- --skip full_non_media_gate_is_clean` and `cargo doc --no-deps -p paradl-cli -p paradl-net` (charter: no missing docs on public items).
- [ ] Commit: `git add rust/crates/paradl-cli/README.md rust/crates/paradl-net/README.md rust/crates/paradl-cli/tests/oracle_parity.rs rust/crates/paradl-net/tests/routing_oracle.rs && git commit -m "cli: document retired flags; keep HTTP/3 compat shim"`.

---

## Final verification gate

- [ ] `cd rust && cargo test -p paradl-cli`
- [ ] `cd rust && cargo test --all -- --skip full_non_media_gate_is_clean`
- [ ] `cd rust && cargo run -q -p paradl -- --help > "$TMPDIR/paradl_help.txt" && diff -u crates/paradl-cli/tests/fixtures/help.txt "$TMPDIR/paradl_help.txt"` — must be empty.
- [ ] `cd rust && cargo run -q -p paradl -- --http3 >/dev/null 2>"$TMPDIR/http3.err"; test $? -eq 2 && grep -q "unrecognized arguments: --http3" "$TMPDIR/http3.err"` — confirms the retired flag is a usage error at the real binary boundary.
- [ ] `cd rust && cargo run -q -p paradl -- netprobe https://example.com/f --http3-probe --json` still emits an `http3_supported` field (C17/F59 preserved).
- [ ] `git status --short` shows only the files listed in Tasks 1-4.
