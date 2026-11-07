# `pdl` Test Suite Guide & Conventions

This directory contains the automated test suite for `parallel-dl` (`pdl` v4.0.0), a resilient parallel download manager on Apple Silicon macOS.

---

## Directory Architecture

Tests are strictly structured by latency, isolation tier, and purpose:

```
tests/
├── conftest.py               # Root fixtures, worker-level simulator, loopback & subprocess autouse guards
├── unit/                     # Pure business logic, strictly isolated, zero network, fast (<0.5s total)
│   ├── test_archive_safety.py
│   ├── test_checksum.py
│   ├── test_classification.py
│   ├── test_config_and_history.py
│   ├── test_curl_parser.py   # Phase E "Copy as cURL" input parsing tests
│   ├── test_models_and_rates.py
│   ├── test_throttle_unit.py # ISP Throttling Diagnostic Suite unit & classifier tests
│   └── test_toolchain.py
├── integration/              # Local loopback sockets, in-memory SQLite, fake macOS CLI runners (~5s)
│   ├── test_aria2_engine.py
│   ├── test_auth_keychain.py
│   ├── test_curl_download.py # Phase E end-to-end curl download flow
│   ├── test_doctor.py
│   ├── test_events_clock.py
│   ├── test_macos_fake.py
│   ├── test_mirrors.py
│   ├── test_queue.py
│   ├── test_throttle_integration.py # pdl.throttle integration against ThrottleTestServer
│   └── test_throttle_simulator.py
├── e2e/                      # Multi-second benchmark flows, crash-resumes, watchdog recycling (marked @pytest.mark.slow)
│   ├── test_mirrors_metalink_e2e.py
│   ├── test_netprobe_bench_e2e.py
│   ├── test_queue_crash_resume_e2e.py
│   ├── test_resilience_e2e.py
│   ├── test_throttle_e2e.py  # pdl throttle quick/full/server e2e tests
│   └── test_watchdog_e2e.py
└── support/                  # THE ONLY place for shared test fixtures, fakes, and utilities
    ├── fakes.py              # FakeKeychain, FakeAria2Server, FakeClock, and factories
    ├── golden/               # macOS CLI outputs (scutil, pmset, networksetup, etc.)
    ├── guards.py             # Loopback socket guard and subprocess isolation guard
    ├── macos_fake.py         # Deterministic subprocess execution runner for macOS tools
    └── server.py             # ONE local HTTP throttling simulator with 11 named profiles
```

---

## Test Execution Modes

Pytest is configured with `strict-markers = true`. By default, slow e2e tests are skipped so the fast development cycle completes in <10 seconds.

### 1. Fast Development Run (Unit + Integration)
```bash
./scripts/test.sh fast
# or:
pytest -m "not slow" --durations=15
```

### 2. Full Suite (Including E2E)
```bash
./scripts/test.sh full
# or:
pytest -m "" --durations=15
```

### 3. Coverage Analysis
Enforces >=80% floor for core modules (`watchdog`, `engines`, `queue`, `classifier`, `auth`) and preserves overall coverage:
```bash
./scripts/test.sh cov
```

### 4. Full Quality Gate (Linter, Typecheck, Fast Tests)
```bash
./scripts/check.sh
```

---

## Test Markers

- `@pytest.mark.unit`: Fast, pure logic tests without file system side effects or network.
- `@pytest.mark.integration`: Component tests using local sockets, SQLite, or fake subprocesses.
- `@pytest.mark.slow`: Long-running scenarios (>1s each), such as real aria2 transfers or throttling benchmarks.
- `@pytest.mark.macos`: Tests requiring native macOS APIs (automatically bypassed if running on non-Darwin platforms).

---

## Shared Test Support (`tests/support`)

### 1. Unified Throttling Simulator (`tests/support/server.py`)
Provides one `ThreadingHTTPServer` with 11 named parametrizable profiles:
1. `per_flow_cap`: Constant bit-rate per socket flow.
2. `shared_cap`: Bandwidth divided dynamically across concurrent streams.
3. `burst_then_throttle`: Fast initial burst collapsing to lower throughput (triggers Watchdog v2).
4. `uniformly_slow`: Consistently slow speed (tests backoff and thrash prevention).
5. `loss_based_policing`: Simulates connection resets/packet loss after a byte threshold.
6. `queue_shaping`: Adds buffer bloat and queueing delays.
7. `time_of_day`: Varies throughput based on simulated time-of-day.
8. `volume_step_down`: Shifts speed downwards after downloading N megabytes.
9. `200_only`: Rejects byte ranges (`Range: bytes=...`) with HTTP 200 OK.
10. `protocol_specific`: Adjusts rate based on request headers/paths.
11. `domestic_vs_international`: Region-based throttling.

Usage:
```python
from tests.support.server import SimulatorProfile, ThrottleTestServer

profile = SimulatorProfile(name="burst_then_throttle", burst_bps=1200 * 1024, throttled_bps=55 * 1024)
with ThrottleTestServer(profile=profile) as srv:
    target_url = srv.file_url
```

### 2. Deterministic macOS Subprocess Runner (`tests/support/macos_fake.py`)
Any unmocked external subprocess call will trip the `subprocess_guard` and fail immediately. Pre-recorded golden outputs for `scutil`, `pmset`, `networksetup`, `security`, etc., are located in `tests/support/golden/`.

Usage:
```python
from tests.support.macos_fake import MacOSFakeRunner


def test_my_feature(macos_runner: MacOSFakeRunner):
    macos_runner.register("pmset", "-g batt", returncode=0, stdout="Now drawing from 'AC Power'...")
```

### 3. Fake Clock & Protocol (`src/pdl/clock.py`, `tests/support/fakes.py`)
All time-dependent components (`watchdog`, `ResilienceEventSource`, `QueueScheduler`, `netprobe`) use the pluggable `Clock` protocol. In tests, use `FakeClock`:
```python
from pdl.clock import FakeClock, set_clock, reset_clock

clock = FakeClock(initial_time=1000.0)
set_clock(clock)
try:
    clock.advance(60.0)  # Simulates 60 seconds with zero wall-clock sleep
finally:
    reset_clock()
```

### 4. Factories (`tests/support/fakes.py`)
- `target_factory(...)`: Generate `TargetInfo` objects.
- `job_factory(...)`: Generate `QueueJob` instances.
- `result_factory(...)`: Generate `TransferResult` models.
- `archive_factory(...)`: Synthesize zip or tar archives with specific payloads (path traversal, symlinks).
- `metalink_factory(...)`: Create RFC 5854 `.meta4` XML companions with valid piece hashes.
