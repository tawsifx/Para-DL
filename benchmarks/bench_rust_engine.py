#!/usr/bin/env python3
"""Honest benchmark: native Rust engine vs REAL legacy aria2.

Run from the repo root with the project venv::

    env -u PYTHONPATH /Users/tawsifx/paradl/.venv/bin/python \
        benchmarks/bench_rust_engine.py                 # -> results/rust.json

    env -u PYTHONPATH /Users/tawsifx/paradl/.venv/bin/python \
        benchmarks/bench_rust_engine.py --tag audit --runs 3   # -> results/audit.json

    env -u PYTHONPATH /Users/tawsifx/paradl/.venv/bin/python \
        benchmarks/bench_rust_engine.py --self-test-pin       # prove the pin assertion

WHY THE PIN MATTERS
-------------------
``Aria2Engine.download()`` is the single delegation point: when
``get_active_engine() == "rust"`` and the compiled extension is available it
silently routes the transfer to ``RustEngine``.  A naive "aria2" benchmark would
therefore measure the *native* engine.  This harness pins ``get_active_engine``
to ``"aria2"`` for every aria2 run AND installs a sentinel on
``RustEngine.download``; if any transfer silently falls back to native, the
sentinel raises ``AssertionError`` and the run fails.  The native runs are
pinned to ``"rust"`` and guarded against spawning an aria2 daemon.  Every
guard emits an auditable evidence record into the JSON.

Measurements
------------
* startup   -- cold engine-module import + construction (fresh subprocess)
* 16-stream loopback throughput -- cold median AND warm median, both with
  sample spreads; loopback measures local CPU/socket cost, NOT WAN anti-throttle
* watchdog recycle latency -- in-process recycle_connections() wall time
* CPU usage -- getrusage(self + reaped children) CPU seconds per transfer
* peak RSS  -- ru_maxrss of the isolated worker (aria2 daemon child reported
  separately; it is NOT additive to the worker's own peak)
* large-file checksum cost -- the shared ChecksumEngine.verify() SHA-256 path

Honesty rules (non-negotiable)
------------------------------
* No codeload/WAN comparison is performed -- this harness is loopback-only.
  If one is ever added, BOTH engines MUST use the identical accelerated URL;
  the JSON states this explicitly (see ``honesty.codeload``).
* Warm throughput reflects daemon/socket reuse + OS page-cache hits and is
  NEVER presented as a general speedup.  Cold and warm are separate fields.
* Differences within ~1.2-1.3x across engines/runs are machine/measurement
  drift, not attributable wins (see ``variance`` and ``honesty.drift``).
* Any metric that cannot be measured fairly is written as ``{"value": null,
  "reason": "..."}`` -- no numbers are invented.
"""
from __future__ import annotations

import argparse
import contextlib
import hashlib
import importlib
import json
import os
import platform
import shutil
import statistics
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

REPO_ROOT = Path(__file__).resolve().parent.parent
RESULTS_DIR = Path(__file__).resolve().parent / "results"
sys.path.insert(0, str(REPO_ROOT / "src"))

PY = sys.executable
MB = 1024 * 1024
PAYLOAD_SIZE = 32 * MB
REPS = 3

CODELOAD_REASON = (
    "No codeload/WAN comparison performed: this harness intentionally uses an "
    "isolated loopback server for reproducible CPU/socket measurement, so no "
    "accelerated URL is involved. If a codeload comparison is added, both "
    "engines MUST use the SAME accelerated URL -- otherwise the result is not "
    "an engine comparison."
)
RECYCLE_ARIA2_REASON = (
    "Not measured: a faithful aria2 recycle comparison requires a live aria2c "
    "daemon with an active GID on a throttled URI (pause/unpause + changeUri); "
    "calling control RPCs on a nonexistent GID returns instantly and would "
    "understate real control-plane cost. The diagnosis doc records ~150 ms for "
    "this path, but it is not reproduced here, so the value is null by design."
)


# ---------------------------------------------------------------------------
# Loopback HTTP server (self-contained; no pytest dependency)
# ---------------------------------------------------------------------------


class Loopback:
    """Threading server serving a deterministic payload with 206 + throttle."""

    def __init__(self, size: int = PAYLOAD_SIZE) -> None:
        self.payload = bytes(((i * 37 + 11) % 256) for i in range(size))
        self.throttle_bps = 0.0
        self._httpd: ThreadingHTTPServer | None = None
        self._thread: threading.Thread | None = None

    @property
    def port(self) -> int:
        assert self._httpd is not None
        return int(self._httpd.server_address[1])

    def url(self) -> str:
        return f"http://127.0.0.1:{self.port}/file"

    def start(self) -> None:
        outer = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, fmt: str, *args: Any) -> None:
                return

            def handle_one_request(self) -> None:
                # A client that abandons an HTTP/1.1 keep-alive connection (the
                # native engine closing its socket after reading a response, or a
                # worker subprocess exiting) makes rfile.readline() raise on the
                # next iteration of BaseHTTPRequestHandler.handle. That is a normal
                # peer disconnect, not a server fault: close quietly so socketserver
                # does not print an "Exception occurred during processing of request"
                # teardown traceback. Same guard as tests/test_rust_engine_parity.py.
                try:
                    super().handle_one_request()
                except (ConnectionResetError, ConnectionAbortedError, BrokenPipeError):
                    self.close_connection = True

            def _write(self, body: bytes) -> None:
                chunk = 8192
                pause = chunk / outer.throttle_bps if outer.throttle_bps > 0 else 0.0
                for i in range(0, len(body), chunk):
                    if pause and i:
                        time.sleep(pause)
                    try:
                        self.wfile.write(body[i : i + chunk])
                    except (BrokenPipeError, ConnectionResetError):
                        return
                self.wfile.flush()

            def do_GET(self) -> None:
                path = urlparse(self.path).path
                if path != "/file":
                    self.send_response(404)
                    self.send_header("Content-Length", "0")
                    self.end_headers()
                    return
                data = outer.payload
                rng = self.headers.get("Range")
                if rng and rng.startswith("bytes="):
                    span = rng.split("=", 1)[1].split(",", 1)[0]
                    start_s, _, end_s = span.partition("-")
                    start = int(start_s)
                    end = int(end_s) if end_s else len(data) - 1
                    end = min(end, len(data) - 1)
                    piece = data[start : end + 1]
                    self.send_response(206)
                    self.send_header("Content-Length", str(len(piece)))
                    self.send_header("Accept-Ranges", "bytes")
                    self.send_header("Content-Range", f"bytes {start}-{end}/{len(data)}")
                    self.end_headers()
                    self._write(piece)
                    return
                self.send_response(200)
                self.send_header("Content-Length", str(len(data)))
                self.send_header("Accept-Ranges", "bytes")
                self.end_headers()
                self._write(data)

        httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        httpd.daemon_threads = True
        self._httpd = httpd
        self._thread = threading.Thread(target=httpd.serve_forever, daemon=True)
        self._thread.start()

    def close(self) -> None:
        if self._httpd is not None:
            self._httpd.shutdown()
            self._httpd.server_close()
        if self._thread is not None:
            self._thread.join(timeout=2)


# ---------------------------------------------------------------------------
# Small helpers
# ---------------------------------------------------------------------------


def native_available() -> bool:
    """True only when a compiled extension actually backs the shim.

    S2 ships an importable shim that raises RustEngineUnavailable on use;
    checking importability alone would falsely report the engine as present.
    """
    try:
        mod = importlib.import_module("paradl._rust_engine")
        return bool(callable(getattr(mod, "is_available", None)) and mod.is_available())
    except Exception:
        return False


def adapter_available() -> bool:
    try:
        importlib.import_module("paradl.engines.rust_engine")
        return True
    except Exception:
        return False


def aria2_available() -> bool:
    return shutil.which("aria2c") is not None


def _target_info(url: str) -> Any:
    from paradl.core.models import TargetInfo, TargetType

    return TargetInfo(
        raw=url,
        target_type=TargetType.DIRECT,
        clean_url=url,
        filename_hint="bench.bin",
        accept_ranges=True,
    )


def _rss_bytes() -> int:
    import resource

    value = int(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)
    # macOS reports bytes; Linux reports KiB.
    return value if sys.platform == "darwin" else value * 1024


def _child_rss_bytes() -> int:
    import resource

    value = int(resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss)
    return value if sys.platform == "darwin" else value * 1024


def _cpu_seconds() -> float:
    """CPU seconds of this worker + its reaped children (aria2c daemon)."""
    import resource

    s = resource.getrusage(resource.RUSAGE_SELF)
    c = resource.getrusage(resource.RUSAGE_CHILDREN)
    return float((s.ru_utime + s.ru_stime) + (c.ru_utime + c.ru_stime))


def _unmeasurable(reason: str) -> dict[str, Any]:
    """JSON shape for a metric that cannot be measured fairly."""
    return {"value": None, "reason": reason}


def _spread(values: list[float]) -> dict[str, float]:
    return {
        "min": round(min(values), 4),
        "max": round(max(values), 4),
        "stdev": round(statistics.stdev(values), 4) if len(values) > 1 else 0.0,
        "n": len(values),
    }


def _median(values: list[float]) -> float:
    return statistics.median(values)


# ---------------------------------------------------------------------------
# Engine pinning + anti-fallback guard
# ---------------------------------------------------------------------------


@contextlib.contextmanager
def _force_engine(name: str) -> Any:
    """Pin the process-wide default engine for a baseline run."""
    from paradl.core import config as cfg

    original = cfg.get_active_engine
    cfg.get_active_engine = lambda: name
    try:
        yield
    finally:
        cfg.get_active_engine = original


def _validate_engine_audit(engine: str, audit: dict[str, Any]) -> None:
    """Prove the pinned engine was actually used (no silent fallback)."""
    if engine == "aria2":
        assert audit["observed_engine_at_decision"] == "aria2", (
            "aria2 run observed active engine "
            f"{audit['observed_engine_at_decision']!r}, expected 'aria2'"
        )
        assert audit["use_rust_decision"] is False, (
            "aria2 transfer decided to use the native Rust engine"
        )
        assert audit["native_fallback_called"] is False, (
            "silent fallback to native RustEngine.download occurred"
        )
        assert audit["aria2_daemon_spawned"] is True, (
            "aria2 run did not spawn a real aria2c daemon"
        )
    else:
        assert native_available(), "native run requested but compiled extension unavailable"
        assert audit["native_fallback_called"] is False
        assert audit["aria2_daemon_spawned"] is False, (
            "native run spawned an aria2c daemon -- engine isolation broken"
        )


@contextlib.contextmanager
def _engine_guard(engine: str) -> Any:
    """Pin ``engine`` and install monitors that fail on a silent fallback.

    Yields an ``audit`` dict filled with evidence.  On successful exit the
    evidence is validated; an ``AssertionError`` means the measurement would
    have been invalid and the harness fails loudly instead of recording a
    number for the wrong engine.
    """
    import paradl.core.config as cfg

    a2 = importlib.import_module("paradl.engines.aria2")
    audit: dict[str, Any] = {
        "requested_engine": engine,
        "observed_engine_at_decision": None,
        "use_rust_decision": None,
        "native_fallback_called": False,
        "aria2_daemon_spawned": False,
    }
    patches: list[tuple[Any, str, Any]] = []

    # Spy on the delegation decision so we can see the engine that was observed.
    orig_use = a2._use_rust_for

    def spy(*args: Any, **kwargs: Any) -> bool:
        audit["observed_engine_at_decision"] = cfg.get_active_engine()
        decision = bool(orig_use(*args, **kwargs))
        audit["use_rust_decision"] = decision
        return decision

    a2._use_rust_for = spy
    patches.append((a2, "_use_rust_for", orig_use))

    # Track real aria2c daemon starts for BOTH engines: aria2 MUST spawn one,
    # native MUST NOT.  Same spy, opposite validation expectations.
    daemon_cls = a2.Aria2RpcDaemon
    orig_start = daemon_cls.start

    def daemon_spy(self: Any, *args: Any, **kwargs: Any) -> None:
        audit["aria2_daemon_spawned"] = True
        orig_start(self, *args, **kwargs)

    daemon_cls.start = daemon_spy
    patches.append((daemon_cls, "start", orig_start))

    if engine == "aria2":
        # Sentinel: if the "aria2" transfer delegates to RustEngine, fail.
        re_mod = importlib.import_module("paradl.engines.rust_engine")

        def boom(*args: Any, **kwargs: Any) -> Any:
            audit["native_fallback_called"] = True
            raise AssertionError("SILENT FALLBACK to native RustEngine.download")

        orig_download = re_mod.RustEngine.download
        re_mod.RustEngine.download = boom
        patches.append((re_mod.RustEngine, "download", orig_download))

    with _force_engine(engine):
        try:
            yield audit
            _validate_engine_audit(engine, audit)
        finally:
            for obj, attr, orig in reversed(patches):
                setattr(obj, attr, orig)


def self_test_pin() -> int:
    """Deliberately break the pin to prove the guard detects silent fallback.

    Returns 0 only if (a) a correct pin is accepted and (b) a broken pin is
    rejected via AssertionError.  Called with --self-test-pin; raw output is
    captured in the task-4 report as the RED/GREEN proof.
    """
    t = _target_info("http://127.0.0.1/file")
    a2 = importlib.import_module("paradl.engines.aria2")
    re_mod = importlib.import_module("paradl.engines.rust_engine")
    from paradl.core import config as cfg

    if not native_available():
        print("SKIP: native extension unavailable; fallback path cannot be exercised")
        return 0

    # (a) correct pin -> accepted (no real download here, so acknowledge the
    # daemon that a real aria2 transfer would have spawned).
    with _engine_guard("aria2") as audit_ok:
        decision = a2._use_rust_for(False, target_info=t)
        if decision:
            re_mod.RustEngine.download(t, Path("."))
        audit_ok["aria2_daemon_spawned"] = True
    assert audit_ok["observed_engine_at_decision"] == "aria2"
    assert audit_ok["use_rust_decision"] is False
    assert audit_ok["native_fallback_called"] is False
    print(f"PASS correct pin accepted: {json.dumps(audit_ok, sort_keys=True)}")

    # (b) broken pin -> sentinel fires (this is the deliberate RED case)
    caught_fallback = False
    try:
        with _engine_guard("aria2") as audit_bad:
            cfg.get_active_engine = lambda: "rust"  # simulate silent fallback
            decision = a2._use_rust_for(False, target_info=t)
            if decision:
                re_mod.RustEngine.download(t, Path("."))
    except AssertionError as exc:
        caught_fallback = True
        print(f"PASS broken pin detected (fallback sentinel): {exc}")
    assert caught_fallback, "SELF-TEST FAILED: silent fallback was NOT detected"

    # (c) wrong observed engine alone must also be rejected
    caught_observe = False
    try:
        with _engine_guard("aria2") as audit_obs:
            cfg.get_active_engine = lambda: "rust"
            a2._use_rust_for(False, target_info=t)  # no fallback call; wrong engine only
    except AssertionError as exc:
        caught_observe = True
        print(f"PASS broken pin detected (wrong observed engine): {exc}")
    assert caught_observe, "SELF-TEST FAILED: wrong observed engine was NOT detected"

    # Each guard's _force_engine restored the real get_active_engine on exit.
    print("SELF-TEST: all 3 checks passed")
    return 0


# ---------------------------------------------------------------------------
# Worker modes (isolated subprocesses for clean RSS / cold-import numbers)
# ---------------------------------------------------------------------------


def _make_engine(engine: str) -> Any:
    if engine == "aria2":
        return importlib.import_module("paradl.engines.aria2").Aria2Engine()
    return importlib.import_module("paradl.engines.rust_engine").RustEngine()


def _execute(engine: str, url: str, out_dir: Path) -> Any:
    return _make_engine(engine).execute(
        _target_info(url),
        out_dir,
        connections=16,
        watchdog_mode="off",
        anti_throttle=None,
        net_watch=False,
        caffeinate=False,
    )


def worker(kind: str, url: str = "") -> None:
    if kind == "import_aria2":
        t0 = time.perf_counter()
        importlib.import_module("paradl.engines.aria2").Aria2Engine()
        print(f"RESULT_MS={(time.perf_counter() - t0) * 1000:.3f}")
        return
    if kind == "import_native":
        if not native_available():
            print("RESULT_ERROR=native extension unavailable")
            sys.exit(2)
        t0 = time.perf_counter()
        importlib.import_module("paradl.engines.rust_engine").RustEngine()
        print(f"RESULT_MS={(time.perf_counter() - t0) * 1000:.3f}")
        return
    if kind in {"transfer_once", "transfer_warm"}:
        engine = os.environ.get("BENCH_ENGINE", "")
        out = Path(os.environ["BENCH_OUT"])
        out.mkdir(parents=True, exist_ok=True)
        if kind == "transfer_once":
            with _engine_guard(engine) as audit:
                t0 = time.perf_counter()
                result = _execute(engine, url, out)
                wall = time.perf_counter() - t0
            if not result.success:
                print(f"RESULT_ERROR={result.error}")
                sys.exit(2)
            # Reap the aria2 daemon so RUSAGE_CHILDREN includes its CPU/RSS.
            importlib.import_module("paradl.engines.aria2").cleanup_children_and_temps()
            print(f"RESULT_WALL_SECONDS={wall:.6f}")
            print(f"RESULT_CPU_SECONDS={_cpu_seconds():.6f}")
            print(f"RESULT_RSS_BYTES={_rss_bytes()}")
            print(f"RESULT_CHILD_RSS_BYTES={_child_rss_bytes()}")
            print(f"RESULT_ENGINE_AUDIT={json.dumps(audit, sort_keys=True)}")
            return
        # warm: first (daemon-spawning) transfer is not timed, then REPS timed ones
        with _engine_guard(engine) as audit:
            warmup = _execute(engine, url, out)
            if not warmup.success:
                print(f"RESULT_ERROR={warmup.error}")
                sys.exit(2)
            samples = []
            for i in range(REPS):
                wdir = out / f"warm_{i}"
                wdir.mkdir(parents=True, exist_ok=True)
                t0 = time.perf_counter()
                r = _execute(engine, url, wdir)
                samples.append(time.perf_counter() - t0)
                if not r.success:
                    print(f"RESULT_ERROR={r.error}")
                    sys.exit(2)
        importlib.import_module("paradl.engines.aria2").cleanup_children_and_temps()
        print("RESULT_WARM_SECONDS=" + ",".join(f"{v:.6f}" for v in samples))
        print(f"RESULT_ENGINE_AUDIT={json.dumps(audit, sort_keys=True)}")
        return
    if kind == "recycle_native":
        if not native_available():
            print("RESULT_ERROR=native extension unavailable")
            sys.exit(2)
        rust = importlib.import_module("paradl._rust_engine")
        out = Path(os.environ["BENCH_OUT"])
        out.mkdir(parents=True, exist_ok=True)
        dest = out / "recycle.bin"
        with _engine_guard("native") as audit:
            handle = rust.start_download(url, str(dest), connections=16)
            deadline = time.monotonic() + 5.0
            while time.monotonic() < deadline:
                snap = handle.poll_snapshot()
                if isinstance(snap, dict):
                    downloaded = snap.get("downloaded", snap.get("bytes_downloaded", 0))
                else:
                    downloaded = getattr(snap, "downloaded", getattr(snap, "bytes_downloaded", 0))
                if int(downloaded or 0) >= 64 * 1024:
                    break
                time.sleep(0.02)
            t0 = time.perf_counter()
            handle.recycle_connections()
            elapsed = (time.perf_counter() - t0) * 1000
            handle.cancel()
        print(f"RESULT_MS={elapsed:.3f}")
        print(f"RESULT_ENGINE_AUDIT={json.dumps(audit, sort_keys=True)}")
        return
    raise SystemExit(f"unknown worker kind: {kind}")


def run_worker(
    kind: str, *, url: str = "", out_dir: Path | None = None, engine: str = ""
) -> dict[str, Any]:
    env = dict(os.environ)
    if out_dir is not None:
        env["BENCH_OUT"] = str(out_dir)
    if engine:
        env["BENCH_ENGINE"] = engine
    proc = subprocess.run(
        [PY, str(Path(__file__).resolve()), "--worker", kind, url],
        capture_output=True,
        text=True,
        env=env,
        timeout=180,
        check=False,
    )
    result: dict[str, Any] = {"returncode": proc.returncode}
    for line in proc.stdout.splitlines():
        if "=" in line:
            key, val = line.split("=", 1)
            result[key] = val
    if proc.returncode != 0:
        result["stderr"] = proc.stderr.strip()[-500:]
    return result


# ---------------------------------------------------------------------------
# Measurements
# ---------------------------------------------------------------------------


def bench_startup() -> dict[str, Any]:
    out: dict[str, Any] = {}
    for engine, kind in (("aria2", "import_aria2"), ("native", "import_native")):
        if engine == "native" and not native_available():
            out[engine] = _unmeasurable("compiled native extension not available")
            continue
        samples = []
        for _ in range(REPS):
            r = run_worker(kind)
            if "RESULT_MS" in r:
                samples.append(float(r["RESULT_MS"]))
        if not samples:
            out[engine] = _unmeasurable("worker produced no timing sample")
            continue
        out[engine] = {
            "cold_import_ms_samples": [round(v, 3) for v in samples],
            "median_ms": round(_median(samples), 3),
            "spread_ms": _spread(samples),
            "scope": "engine adapter module import + construction in a fresh interpreter",
        }
    return out


def bench_transfer_metrics(server: Loopback, root: Path) -> dict[str, Any]:
    """One pass yields throughput + CPU + RSS from the same guarded workers."""
    ret: dict[str, Any] = {
        "throughput_16_stream": {},
        "cpu_usage": {},
        "peak_rss": {},
        "engine_pinning_evidence": {},
    }
    mb = PAYLOAD_SIZE / MB
    for engine in ("aria2", "native"):
        if engine == "native" and not (native_available() and adapter_available()):
            reason = "native extension or adapter not available"
            ret["throughput_16_stream"][engine] = _unmeasurable(reason)
            ret["cpu_usage"][engine] = _unmeasurable(reason)
            ret["peak_rss"][engine] = _unmeasurable(reason)
            continue
        if engine == "aria2" and not aria2_available():
            reason = "aria2c binary not available"
            ret["throughput_16_stream"][engine] = _unmeasurable(reason)
            ret["cpu_usage"][engine] = _unmeasurable(reason)
            ret["peak_rss"][engine] = _unmeasurable(reason)
            continue

        cold: list[float] = []
        cpu: list[float] = []
        rss: list[int] = []
        child_rss: list[int] = []
        audit: dict[str, Any] = {}
        failed: str | None = None
        for i in range(REPS):
            d = root / f"tp_{engine}_cold_{i}"
            shutil.rmtree(d, ignore_errors=True)
            r = run_worker("transfer_once", url=server.url(), out_dir=d, engine=engine)
            if "RESULT_WALL_SECONDS" not in r:
                failed = r.get("RESULT_ERROR", r.get("stderr", "worker produced no timing sample"))
                break
            cold.append(float(r["RESULT_WALL_SECONDS"]))
            cpu.append(float(r["RESULT_CPU_SECONDS"]))
            rss.append(int(r["RESULT_RSS_BYTES"]))
            child_rss.append(int(r.get("RESULT_CHILD_RSS_BYTES", "0")))
            audit = json.loads(r["RESULT_ENGINE_AUDIT"])
        if failed is not None:
            for key in ("throughput_16_stream", "cpu_usage", "peak_rss"):
                ret[key][engine] = _unmeasurable(f"transfer worker failed: {failed}")
            continue

        warm: list[float] = []
        d = root / f"tp_{engine}_warm"
        shutil.rmtree(d, ignore_errors=True)
        rw = run_worker("transfer_warm", url=server.url(), out_dir=d, engine=engine)
        if "RESULT_WARM_SECONDS" in rw:
            warm = [float(v) for v in rw["RESULT_WARM_SECONDS"].split(",")]
            warm_audit = json.loads(rw["RESULT_ENGINE_AUDIT"])
        else:
            failed = rw.get("RESULT_ERROR", rw.get("stderr", "warm worker produced no sample"))

        cold_med = _median(cold)
        ret["throughput_16_stream"][engine] = {
            "bytes": PAYLOAD_SIZE,
            "connections": 16,
            "cold_samples_seconds": [round(v, 4) for v in cold],
            "cold_median_seconds": round(cold_med, 4),
            "cold_median_mbps": round(mb / cold_med, 2),
            "cold_spread_seconds": _spread(cold),
            "warm_samples_seconds": [round(v, 4) for v in warm] or None,
            "warm_median_seconds": round(_median(warm), 4) if warm else None,
            "warm_median_mbps": round(mb / _median(warm), 2) if warm else None,
            "warm_spread_seconds": _spread(warm) if warm else None,
            "warm_note": (
                "Warm median reflects daemon/socket reuse + OS page-cache hits; "
                "NOT a general speedup. Compare cold-vs-cold for an engine diff."
            ),
        }
        cpu_med = _median(cpu)
        ret["cpu_usage"][engine] = {
            "cpu_seconds_samples": [round(v, 4) for v in cpu],
            "cpu_seconds_median": round(cpu_med, 4),
            "cpu_spread_seconds": _spread(cpu),
            "wall_median_seconds": round(cold_med, 4),
            "cpu_utilization_pct": round(100.0 * cpu_med / cold_med, 1),
            "scope": (
                "getrusage(self + reaped children) CPU seconds for one cold "
                "32 MiB transfer; includes the aria2c daemon child for aria2."
            ),
        }
        rss_med = _median([float(v) for v in rss])
        rss_entry: dict[str, Any] = {
            "peak_rss_mb_samples": [round(v / MB, 2) for v in rss],
            "peak_rss_mb": round(rss_med / MB, 2),
            "peak_rss_spread_mb": _spread([v / MB for v in rss]),
            "scope": "ru_maxrss of the isolated worker process after one cold transfer",
        }
        if engine == "aria2":
            rss_entry["child_daemon_peak_rss_mb"] = round(_median([float(v) for v in child_rss]) / MB, 2)
            rss_entry["note"] = (
                "Worker peak excludes the aria2c daemon child. Child peak "
                "(largest single reaped child) is reported separately and is "
                "NOT additive to the worker peak, so total aria2 footprint is "
                "understated; do not treat this as an apples-to-apples RSS win."
            )
        ret["peak_rss"][engine] = rss_entry
        ret["engine_pinning_evidence"][engine] = warm_audit if warm else audit
    return ret


def bench_recycle(root: Path) -> dict[str, Any]:
    out: dict[str, Any] = {"native": None, "aria2": _unmeasurable(RECYCLE_ARIA2_REASON)}
    if not native_available():
        out["native"] = _unmeasurable("compiled native extension not available")
        return out
    srv = Loopback(size=8 * MB)
    srv.throttle_bps = 128 * 1024
    srv.start()
    try:
        samples: list[float] = []
        audit: dict[str, Any] = {}
        for i in range(REPS):
            r = run_worker("recycle_native", url=srv.url(), out_dir=root / f"recycle_{i}")
            if "RESULT_MS" not in r:
                out["native"] = _unmeasurable(
                    f"recycle worker failed: {r.get('RESULT_ERROR', r.get('stderr', r))}"
                )
                break
            samples.append(float(r["RESULT_MS"]))
            audit = json.loads(r["RESULT_ENGINE_AUDIT"])
        else:
            out["native"] = {
                "samples_ms": [round(v, 4) for v in samples],
                "median_ms": round(_median(samples), 4),
                "spread_ms": _spread(samples),
                "engine_audit": audit,
                "scope": (
                    "in-process recycle_connections() wall time after >=64 KiB of "
                    "throttled progress on a live native handle"
                ),
            }
    finally:
        srv.close()
    return out


def bench_checksum(root: Path) -> dict[str, Any]:
    """Large-file SHA-256 cost of the shared verification path (256 MiB, mmap)."""
    from paradl.postproc.checksum import ChecksumEngine

    size = 256 * MB
    path = root / "checksum_big.bin"
    if not path.exists() or path.stat().st_size != size:
        block = bytes(((i * 37 + 11) % 256) for i in range(MB))
        with open(path, "wb") as fh:
            for _ in range(256):
                fh.write(block)
    digest = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(MB), b""):
            digest.update(chunk)
    expected = f"sha256:{digest.hexdigest()}"

    samples: list[float] = []
    for _ in range(REPS):
        t0 = time.perf_counter()
        matched, algo, computed = ChecksumEngine.verify(path, expected)
        samples.append(time.perf_counter() - t0)
        assert matched and algo == "sha256" and computed == digest.hexdigest()
    med = _median(samples)
    per_engine = {
        "aria2": {"value": round(med, 4), "shared_path": True},
        "native": {"value": round(med, 4), "shared_path": True},
    }
    return {
        "file_size_bytes": size,
        "algorithm": "sha256",
        "samples_seconds": [round(v, 4) for v in samples],
        "median_seconds": round(med, 4),
        "spread_seconds": _spread(samples),
        "throughput_mbps": round((size / MB) / med, 2),
        "shared_by_both_engines": True,
        "per_engine": per_engine,
        "difference": _unmeasurable(
            "Both engines call the same paradl.postproc.checksum.ChecksumEngine.verify "
            "(Python hashlib); checksum cost is engine-independent, so no per-engine "
            "difference exists to measure."
        ),
        "note": (
            "256 MiB crosses the 100 MiB mmap threshold in ChecksumEngine; this "
            "measures the real large-file path. Warm OS page-cache may lower the "
            "number -- it is not a WAN or engine-specific throughput claim."
        ),
    }


# ---------------------------------------------------------------------------
# Suite assembly + variance
# ---------------------------------------------------------------------------


VARIANCE_PATHS = [
    ("startup_overhead", "aria2", "median_ms"),
    ("startup_overhead", "native", "median_ms"),
    ("throughput_16_stream", "aria2", "cold_median_mbps"),
    ("throughput_16_stream", "native", "cold_median_mbps"),
    ("throughput_16_stream", "aria2", "warm_median_mbps"),
    ("throughput_16_stream", "native", "warm_median_mbps"),
    ("socket_recycle_latency", "native", "median_ms"),
    ("cpu_usage", "aria2", "cpu_seconds_median"),
    ("cpu_usage", "native", "cpu_seconds_median"),
    ("peak_rss", "aria2", "peak_rss_mb"),
    ("peak_rss", "native", "peak_rss_mb"),
    ("checksum_large_file", "median_seconds"),
]


def _dig(data: dict[str, Any], path: tuple[str, ...]) -> Any:
    cur: Any = data
    for key in path:
        if not isinstance(cur, dict) or key not in cur:
            return None
        cur = cur[key]
    return cur


def compute_variance(runs: list[dict[str, Any]]) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for path in VARIANCE_PATHS:
        vals = [v for v in (_dig(r["benchmarks"], path) for r in runs) if isinstance(v, (int, float))]
        if not vals:
            out[".".join(path)] = _unmeasurable("no numeric samples across runs")
            continue
        out[".".join(path)] = {
            "values": [round(float(v), 4) for v in vals],
            "median": round(_median([float(v) for v in vals]), 4),
            "min": round(min(float(v) for v in vals), 4),
            "max": round(max(float(v) for v in vals), 4),
            "spread_pct_of_median": (
                round(100.0 * (max(vals) - min(vals)) / _median([float(v) for v in vals]), 1)
                if _median([float(v) for v in vals])
                else None
            ),
        }
    out["_drift_rule"] = (
        "Differences within ~1.2-1.3x (or within the per-metric cross-run spread) "
        "are machine/measurement drift, not attributable engine wins."
    )
    return out


def run_benchmark_suite(root: Path) -> dict[str, Any]:
    srv = Loopback()
    srv.start()
    try:
        tm = bench_transfer_metrics(srv, root)
    finally:
        srv.close()

    pinning = dict(tm["engine_pinning_evidence"])

    def _evidence_ok(eng: str, ev: Any) -> bool:
        if not isinstance(ev, dict):
            return False
        if eng == "aria2":
            return (
                ev.get("observed_engine_at_decision") == "aria2"
                and ev.get("use_rust_decision") is False
                and ev.get("native_fallback_called") is False
                and ev.get("aria2_daemon_spawned") is True
            )
        return (
            ev.get("native_fallback_called") is False
            and ev.get("aria2_daemon_spawned") is False
        )

    pinning["method"] = (
        "aria2 runs pin get_active_engine()='aria2', spy on Aria2Engine._use_rust_for, "
        "and sentinel RustEngine.download to fail on silent fallback (+assert a real "
        "aria2c daemon spawned). Native runs pin 'rust' and assert no aria2 daemon. "
        "--self-test-pin proves both the positive and the deliberately-broken case."
    )
    pinning["assertions_passed"] = all(
        _evidence_ok(eng, ev)
        for eng, ev in pinning.items()
        if not isinstance(ev, str)
    )

    return {
        "environment": {
            "python": sys.version.split()[0],
            "platform": platform.platform(),
            "aria2c": shutil.which("aria2c"),
            "native_extension_present": native_available(),
            "rust_adapter_present": adapter_available(),
        },
        "methodology": {
            "startup": "cold engine-module import + construction, median of 3 fresh subprocesses",
            "throughput": (
                "32 MiB deterministic loopback, 16 connections; 3 cold transfers in "
                "fresh subprocesses + warm median (3 transfers after daemon/engine "
                "warmup). Loopback measures local CPU/socket cost, NOT WAN "
                "anti-throttle performance. Reports medians + spreads."
            ),
            "recycle": (
                "in-process recycle_connections() wall time after >=64 KiB throttled "
                "progress; native only (aria2 null+reason -- see honesty)."
            ),
            "cpu_usage": "getrusage(self + reaped children) CPU seconds for one cold transfer",
            "peak_rss": "ru_maxrss of isolated worker; aria2 daemon child reported separately",
            "checksum": "shared ChecksumEngine.verify SHA-256 on a 256 MiB mmap file",
        },
        "honesty": {
            "codeload": _unmeasurable(CODELOAD_REASON),
            "warm_cache": (
                "Warm throughput includes daemon/socket reuse + page-cache hits and is "
                "NOT presented as a general speedup; cold-vs-cold is the engine comparison."
            ),
            "drift": (
                "The ~1.2-1.3x cross-run drift documented in PARALLEL_DL_DEV_CONTEXT.md "
                "sec.7 is measurement noise, NOT a code-attributable win. See variance."
            ),
            "unfair_metrics": (
                "Metrics that cannot be measured fairly are written as "
                "{'value': null, 'reason': '...'} and never invented."
            ),
        },
        "engine_pinning": pinning,
        "benchmarks": {
            "startup_overhead": bench_startup(),
            "throughput_16_stream": tm["throughput_16_stream"],
            "socket_recycle_latency": bench_recycle(root),
            "cpu_usage": tm["cpu_usage"],
            "peak_rss": tm["peak_rss"],
            "checksum_large_file": bench_checksum(root),
        },
    }


def main() -> int:
    # Worker mode is parsed before argparse: the extra positional URL would
    # otherwise be rejected as an unrecognized argument.
    if len(sys.argv) > 1 and sys.argv[1] == "--worker":
        kind = sys.argv[2] if len(sys.argv) > 2 else ""
        url = sys.argv[3] if len(sys.argv) > 3 else ""
        worker(kind, url)
        return 0

    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--tag", default="rust", help="result basename under results/ (default: rust)")
    ap.add_argument("--runs", type=int, default=1, help="benchmark repetitions for variance")
    ap.add_argument("--self-test-pin", action="store_true", help="prove the pin assertion")
    args = ap.parse_args()

    if args.self_test_pin:
        return self_test_pin()

    root = Path(
        os.environ.get(
            "BENCH_TMP",
            "/private/var/folders/zq/y94lwyxd3691r95vgrbk1ymr0000gn/T/opencode/rust-bench",
        )
    )
    root.mkdir(parents=True, exist_ok=True)

    runs = [run_benchmark_suite(root) for _ in range(max(1, args.runs))]
    final = dict(runs[-1])
    final["schema_version"] = 1
    final["harness_version"] = 2
    final["generated_at"] = time.strftime("%Y-%m-%dT%H:%M:%S%z")
    final["runs"] = runs
    final["variance"] = (
        compute_variance(runs)
        if len(runs) > 1
        else _unmeasurable("single run; re-run with --runs 3 to quantify variance")
    )

    out_path = RESULTS_DIR / f"{args.tag}.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(final, indent=2, sort_keys=True) + "\n")
    print(json.dumps(final, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
