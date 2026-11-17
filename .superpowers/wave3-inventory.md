# Wave-3 Phase-1 Test Inventory (BASE ab2629b)

Command: `PYTHONPATH=$PWD/src /Users/tawsifx/paradl/.venv/bin/python -m pytest --durations=25 -q`
Result: 222 passed in 2.12s. pytest 9.2.0.

| file | tests | notable slow call | tier target |
|---|---:|---|---|
| tests/unit_test_config.py | 31 | — | unit/test_config.py |
| tests/unit_test_watchdog.py | 27 | — | unit/test_watchdog.py |
| tests/unit_test_net.py | 25 | — | unit/test_net.py |
| tests/unit_test_core.py | 22 | — | unit/test_models.py |
| tests/unit_test_aria2.py | 22 | 0.03s | unit/test_aria2.py |
| tests/test_bug_fixes.py | 20 | r1 probe 0.31s | split: unit/test_bug_fixes.py (19) + integration/test_probe_identity.py (1) |
| tests/test_wave2_classifier.py | 17 | — | unit/test_classifier.py |
| tests/test_isp_detection.py | 12 | 0.01s | integration/test_isp_detection.py |
| tests/test_platform_windows.py | 9 | — | unit/test_platform_windows.py |
| tests/test_image_probe_and_http_errors.py | 6 | 0.02s | integration/test_engine_batch.py |
| tests/test_perf_parity.py | 5 | — | unit/test_perf_parity.py PRESERVE-EXACT |
| tests/test_wave2_sqlite.py | 5 | 0.01s | integration/test_sqlite_guard.py |
| tests/test_wave2_concurrency.py | 5 | dispatch 0.15s | integration/test_concurrency.py |
| tests/test_wave2_aria2.py | 5 | mypy 0.26s, extractor 0.12s | integration/test_aria2_rpc.py (+ e2e slow guard for mypy) |
| tests/test_platform_unification.py | 4 | doctor 0.45s | integration/ + e2e slow guard (doctor) |
| tests/test_engines_windows.py | 4 | — | unit/test_engines_windows.py |
| tests/test_pypi_transitive.py | 2 | 0.01s | unit/test_pypi_transitive.py |
| tests/test_config_shim.py | 1 | — | unit/test_config_shim.py PRESERVE-EXACT |
| tests/conftest.py | 0 | — | keep at root, drop sys.path hack |

Total 19 files, 222 tests.

Structural facts:
- No http.server / socket servers / bind / listen anywhere in tests → support/server.py is NEW, not a consolidation.
- No duplicate test function names across files.
- No xfail markers; one `pytest.skip` conditional in the mypy guard test.
- paradl.cli is a package (src/paradl/cli/{__init__,main,parser,dispatch}.py); deleted monolith was src/paradl/cli.py (file). Imports in test_bug_fixes.py & test_perf_parity.py are valid.
- paradl.core.clock provides Clock, FakeClock, get_clock — reuse.
- Real time.sleep(0.02) in test_wave2_aria2.py singleton tests (race-window wideners) — concurrency, not FakeClock-able.
- conftest.py autouse: isolate_paradl_home, reset_isp_cache. Root scope covers all future tiers.

Dedup candidates (verify, don't assume):
1. unit_test_aria2.py::TestWatchdogModes::test_values  ==/=  unit_test_core.py::TestWatchdogModes::test_watchdog_mode_values
2. unit_test_core.py::TestFormatSpeedAndSize::test_format_size  ==/=  test_bug_fixes.py::test_l3_format_size
NOT duplicates (keep both): config default schema vs config-shim re-export; classifier mirror dedup vs perf-parity mirror dedup; zone-identifier case-insensitivity vs ADS handling.
