# perf/max-efficiency — Implementation Plan

Baseline (worktree, 57 tests): `pytest -q` → **57 passed**. Benchmark harness:
`benchmarks/bench_baseline.py --tag baseline` → `benchmarks/results/baseline.json`.

Scope discipline: no network benchmarks (no internet), no GPU claims (this is a
download CLI), no framework rewrites (PyO3/Rust/Polars would break output parity
for a stdlib-only tool). Only measured, offline, output-preserving changes.

## Task 1 — Remove verified dead code
- [ ] Delete `src/paradl/cli.py` (813 lines). Evidence: package `src/paradl/cli/`
      shadows it; entry point `paradl.cli:main` resolves to `cli/main.py`; zero
      file-path or module references. `auth.py` is also shadowed but kept (no win).
- [ ] Guard test: entry point + `build_parser` still resolve to the package.
- [ ] Verify: `pytest -q`, `ruff check src`, benchmark unchanged.

## Task 2 — `core.models.sanitize_filename` hot path
- [ ] Add benchmark case over 100k tricky filenames.
- [ ] Golden parity test (exact outputs, incl. reserved names, traversal, NUL).
- [ ] Hoist the 27-element Windows-reserved-name set to a module-level frozenset;
      replace 10 chained `str.replace` passes with one `str.translate`.
- [ ] Verify: parity test green, full suite green, benchmark speedup measured.
      Revert if no measurable gain.

## Task 3 — `storage.mirrors` I/O + quadratic dedup
- [ ] Benchmark already shows 2,066 ms/call at 2,000 rules and ~2.0 s at 20k
      rules (stress) — config is re-read and re-parsed per call.
- [ ] Parity test: `match_configured_mirrors` returns identical ordered list.
- [ ] Cache parsed config by (path, mtime_ns, size) so unchanged files are not
      re-read; use a set alongside the ordered list for O(1) dedup membership.
- [ ] Verify: parity test green, full suite green, benchmark speedup measured.

## Task 4 — Daemon pool concurrency bug (correctness that blocks parallelism)
- [ ] RED test: two concurrent acquires of the same `(ip_family, http3)` key
      must return two different daemons; acquire-after-release must reuse one.
- [ ] GREEN: store a list of entries per key and only reuse entries with
      `in_use == False`; update release/cleanup/shutdown accordingly.
- [ ] Verify: new test red→green, full suite green, no deadlocks.
      (Optional, only if cheap: lock the lazy singleton constructors.)

## Task 5 — Final verification
- [ ] Fresh `pytest -q` (full suite).
- [ ] Fresh `benchmarks/bench_baseline.py --tag after --compare baseline.json`.
- [ ] `ruff check src tests` and `mypy` if clean at baseline.
- [ ] Report honest before/after numbers; revert any change without a measured win.

## Explicit non-goals
- No deletion of backward-compat shims with live importers (public API).
- No async/event-loop rewrite: audited endpoints are sync and run in Starlette's
  threadpool, so there is no event-loop blocking to fix.
- No speculative rewrites; every changed line must map to a task above.
