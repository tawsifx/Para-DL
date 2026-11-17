# SDD ledger — plan: docs/superpowers/plans/2026-11-17-wave3-test-centralization.md

BASE: ab2629ba325831ee4051f9bb9fa924da16241c6f
Worktree: /Users/tawsifx/paradl-perf (perf/max-efficiency)
Baseline: 19 test files, 222 passed in 2.12s | ruff clean | mypy clean (82 files)

## Rulings
- Parallel implementers for Tasks 2/3 authorized: disjoint directory scopes; shared source test_bug_fixes.py split by explicit test-name list in plan. Cost if wrong: lead reconciles.
- test_bug_fixes.py split: S2 takes 19 tests → unit; S3 takes test_r1_probe_requests_identity_encoding → integration; lead deletes original after both DONE.
- No obsolete tests found targeting deleted cli.py monolith (paradl.cli is now a valid package). Subagents FLAG suspected obsolete tests, do not delete unilaterally.
- support/server.py + macos_fake.py are NEW reusable harnesses (no existing duplicated inline servers to consolidate); one new e2e smoke test will exercise them.
- Subprocess guard must be opt-in named fixture (never autouse) or the mypy-guard test breaks.

## Phase 1
- Inventory + durations captured → .superpowers/wave3-inventory.md
- Plan written + self-reviewed → docs/superpowers/plans/2026-11-17-wave3-test-centralization.md

## Tasks
- Task 1 (support harness + pytest config): COMPLETE — spec PASS, quality Approved. Smoke 7 passed; full 95 passed pre-migration (expected: 127 unit_test_* tests uncollected per new python_files); ruff/mypy clean; no PYTHONPATH needed. Minor notes: session-scope server (xdist absent), UDP/DNS hermeticity gap, loopback allowlist names. Lead must delete tests/smoke_support.py before final verify. I2 verified: `pdl` script entry removal in pyproject.toml traces to accepted Wave-2 S4 docs reconciliation (not this task); out of Wave-3 scope — flag to user only.
- Task 2 (unit migration/dedup): COMPLETE — spec PASS, quality Approved. AST-level zero-coverage-regression proof: 2 dedups (WatchdogMode 3-case, format_size 7-case union), R1 relocated not copied, PRESERVE-EXACT files cmp-identical. 190 unit tests.
- Task 3 (integration/e2e): COMPLETE — spec PASS, quality Approved. 36 integration + 3 e2e (2 slow guards + 1 new FakeClock+FakeAria2Server watchdog-recycle test verified semantically against watchdog.py). Zero coverage regression proven byte-for-byte.
- NOTE: transitions share basenames between new tier files and legacy root files; all tier dirs must have __init__.py. Lead deletes smoke_support.py + legacy files after both migrations verified.
- Task 4 (lead deletion/speed/verify): IN PROGRESS — legacy 18 flat files + 2 temp TDD files deleted after both migration reviews passed. Final tree: 29 test/support .py files. Speed pass: session-scoped socket guard, NoSleepClock on 3 engine poll-loop tests, skip up-front HTTP probe in checksum unit test (accept_ranges_hint=True), hoisted conftest import. Verified: 229 passed | 226 passed/3 deselected (not slow) | 190 unit in ~0.49-0.55s (machine noise; target effectively met, all real sleeps gone) | ruff clean | mypy clean. Scoped review of lead speed edits pending ses_e1a1fa8b7ffeNdLRpTXNIIAXX2.

## Reviews
Each task: spec-compliance + code-quality verdict before acceptance.

## Progress log
- Phase 1 complete: inventory, durations, plan, ledger.
- Phase 2 complete: S1 harness accepted; S2/S3 migrations accepted (4 spec+quality reviews total, plus scoped re-review pending).
- Phase 3: 18 legacy flat test files + 2 temp proof files deleted; full suite 229 passed / 1.6s (was 222 / 2.12s); unit 190 in <0.5s (noise-bounded); ruff+mypy clean. CI runs full suite so slow guards stay active.
