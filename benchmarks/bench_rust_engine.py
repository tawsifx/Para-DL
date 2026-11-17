#!/usr/bin/env python3
"""Honest benchmark: aria2 path vs native Rust engine.

Run from the repo root with the project venv::

    env -u PYTHONPATH /Users/tawsifx/paradl/.venv/bin/python \
        benchmarks/bench_rust_engine.py

Measurements
------------
* cold import + engine construction (subprocess, fresh interpreter)
* 16-stream loopback throughput (MB/s), cold run AND warm median separately
* socket-recycle latency (ms), measured against a live throttled transfer
* peak RSS (MB) of an isolated worker subprocess

Honesty rules
-------------
* Loopback throughput measures local CPU/socket overhead, NOT WAN
  anti-throttle gains.  No such claim is made.
* aria2's first transfer includes daemon spawn; later transfers reuse the
  daemon pool.  Cold and warm numbers are reported separately and warm-cache
  hits are never presented as a general speedup.
* If the native extension is not built (``paradl._rust_engine`` absent) the
  native fields are recorded as ``null`` with a reason -- no numbers are
  invented.  Re-run after ``maturin develop --release`` to populate them.

Results are appended/overwritten at ``benchmarks/results/rust.json``.
"""

from __future__ import annotations

import contextlib
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
RESULTS_PATH = Path(__file__).resolve().parent / "results" / "rust.json"
sys.path.insert(0, str(REPO_ROOT / "src"))

PY = sys.executable
PAYLOAD_SIZE = 32 * 1024 * 1024  # 32 MiB
REPS = 3


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


# ---------------------------------------------------------------------------
# Worker modes (isolated subprocesses for clean RSS / cold-import numbers)
# ---------------------------------------------------------------------------


def worker(kind: str, url: str = "") -> None:
    if kind == "import_aria2":
        t0 = time.perf_counter()
        importlib.import_module("paradl.engines.aria2")
        print(f"RESULT_MS={(time.perf_counter() - t0) * 1000:.3f}")
        return
    if kind == "import_native":
        t0 = time.perf_counter()
        importlib.import_module("paradl._rust_engine")
        print(f"RESULT_MS={(time.perf_counter() - t0) * 1000:.3f}")
        return
    if kind in {"transfer_aria2", "transfer_native"}:
        out = Path(os.environ["BENCH_OUT"])
        out.mkdir(parents=True, exist_ok=True)
        if kind == "transfer_aria2":
            engine = importlib.import_module("paradl.engines.aria2").Aria2Engine()
        else:
            engine = importlib.import_module("paradl.engines.rust_engine").RustEngine()
        with _force_engine("aria2" if kind == "transfer_aria2" else "rust"):
            result = engine.execute(
                _target_info(url),
                out,
                connections=16,
                watchdog_mode="off",
                anti_throttle=None,
                net_watch=False,
                caffeinate=False,
            )
        if not result.success:
            print(f"RESULT_ERROR={result.error}")
            sys.exit(2)
        print(f"RESULT_RSS_BYTES={_rss_bytes()}")
        return
    if kind == "recycle_native":
        rust = importlib.import_module("paradl._rust_engine")
        out = Path(os.environ["BENCH_OUT"])
        out.mkdir(parents=True, exist_ok=True)
        dest = out / "recycle.bin"
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
        print(f"RESULT_MS={(time.perf_counter() - t0) * 1000:.3f}")
        handle.cancel()
        return
    raise SystemExit(f"unknown worker kind: {kind}")


def run_worker(kind: str, *, url: str = "", out_dir: Path | None = None) -> dict[str, Any]:
    env = dict(os.environ)
    if out_dir is not None:
        env["BENCH_OUT"] = str(out_dir)
    proc = subprocess.run(
        [PY, str(Path(__file__).resolve()), "--worker", kind, url],
        capture_output=True,
        text=True,
        env=env,
        timeout=120,
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
            out[engine] = None
            continue
        samples = []
        for _ in range(REPS):
            r = run_worker(kind)
            if "RESULT_MS" in r:
                samples.append(float(r["RESULT_MS"]))
        out[engine] = {
            "cold_import_ms_samples": samples,
            "median_ms": round(statistics.median(samples), 3) if samples else None,
        }
    return out


@contextlib.contextmanager
def _force_engine(name: str) -> Any:
    """Pin the default engine for a baseline run (aria2.py delegates to Rust by default)."""
    from paradl.core import config as cfg

    original = cfg.get_active_engine
    cfg.get_active_engine = lambda: name
    try:
        yield
    finally:
        cfg.get_active_engine = original


def _run_transfer(engine: str, url: str, out_dir: Path) -> float:
    if engine == "aria2":
        impl = importlib.import_module("paradl.engines.aria2").Aria2Engine
    else:
        impl = importlib.import_module("paradl.engines.rust_engine").RustEngine
    t0 = time.perf_counter()
    with _force_engine(engine):
        result = impl().execute(
            _target_info(url),
            out_dir,
            connections=16,
            watchdog_mode="off",
            anti_throttle=None,
            net_watch=False,
            caffeinate=False,
        )
    elapsed = time.perf_counter() - t0
    assert result.success, f"{engine} transfer failed: {result.error}"
    return elapsed


def bench_throughput(server: Loopback, root: Path) -> dict[str, Any]:
    out: dict[str, Any] = {}
    for engine in ("aria2", "native"):
        if engine == "native" and not (native_available() and adapter_available()):
            out[engine] = None
            continue
        try:
            importlib.import_module(
                "paradl.engines.rust_engine" if engine == "native" else "paradl.engines.aria2"
            )
        except Exception as exc:
            out[engine] = {"error": f"adapter/engine import failed: {exc}"}
            continue
        import shutil as _shutil

        cold_dir = root / f"{engine}_cold"
        _shutil.rmtree(cold_dir, ignore_errors=True)
        cold_s = _run_transfer(engine, server.url(), cold_dir)
        warm_samples = []
        for i in range(REPS):
            warm_dir = root / f"{engine}_warm_{i}"
            _shutil.rmtree(warm_dir, ignore_errors=True)
            warm_samples.append(_run_transfer(engine, server.url(), warm_dir))
        mb = PAYLOAD_SIZE / (1024 * 1024)
        out[engine] = {
            "cold_seconds": round(cold_s, 4),
            "cold_mbps": round(mb / cold_s, 2),
            "warm_samples_seconds": [round(v, 4) for v in warm_samples],
            "warm_median_seconds": round(statistics.median(warm_samples), 4),
            "warm_median_mbps": round(mb / statistics.median(warm_samples), 2),
            "bytes": PAYLOAD_SIZE,
            "connections": 16,
        }
    return out


def bench_recycle(root: Path) -> dict[str, Any]:
    out: dict[str, Any] = {"native": None, "aria2": None}
    if native_available():
        srv = Loopback(size=8 * 1024 * 1024)
        srv.throttle_bps = 128 * 1024
        srv.start()
        try:
            d = root / "recycle_native"
            r = run_worker("recycle_native", url=srv.url(), out_dir=d)
            if "RESULT_MS" in r:
                out["native"] = {"latency_ms": float(r["RESULT_MS"])}
            else:
                out["native"] = {"error": r.get("stderr", r)}
        finally:
            srv.close()
    else:
        out["native"] = None

    if aria2_available():
        out["aria2"] = {
            "latency_ms": None,
            "note": (
                "Not measured: a faithful comparison requires a live aria2c daemon with "
                "an active GID on a throttled URI; pause/unpause on a nonexistent GID "
                "would return instantly and understate real control-plane cost. "
                "The diagnosis doc records ~150 ms for this path."
            ),
        }
    return out


def bench_peak_rss(root: Path) -> dict[str, Any]:
    out: dict[str, Any] = {}
    srv = Loopback(size=8 * 1024 * 1024)
    srv.start()
    try:
        for engine in ("aria2", "native"):
            if engine == "native" and not (native_available() and adapter_available()):
                out[engine] = None
                continue
            d = root / f"rss_{engine}"
            r = run_worker(f"transfer_{engine}", url=srv.url(), out_dir=d)
            if "RESULT_RSS_BYTES" in r:
                out[engine] = {
                    "peak_rss_mb": round(int(r["RESULT_RSS_BYTES"]) / (1024 * 1024), 2)
                }
            else:
                out[engine] = {"error": r.get("stderr", r)}
    finally:
        srv.close()
    return out


def main() -> None:
    if len(sys.argv) > 1 and sys.argv[1] == "--worker":
        worker(sys.argv[2], sys.argv[3] if len(sys.argv) > 3 else "")
        return

    root = Path(
        os.environ.get(
            "BENCH_TMP",
            "/private/var/folders/zq/y94lwyxd3691r95vgrbk1ymr0000gn/T/opencode/rust-bench",
        )
    )
    root.mkdir(parents=True, exist_ok=True)

    srv = Loopback()
    srv.start()
    try:
        throughput = bench_throughput(srv, root)
    finally:
        srv.close()

    report: dict[str, Any] = {
        "schema_version": 1,
        "generated_at": time.strftime("%Y-%m-%dT%H:%M:%S%z"),
        "environment": {
            "python": sys.version.split()[0],
            "platform": platform.platform(),
            "aria2c": shutil.which("aria2c"),
            "native_extension_present": native_available(),
            "rust_adapter_present": adapter_available(),
        },
        "methodology": {
            "startup": "cold import in a fresh subprocess (median of 3)",
            "throughput": (
                "32 MiB deterministic loopback payload, 16 connections; cold run (first "
                "invocation) and warm median (3 runs) reported separately. Loopback "
                "measures local CPU/socket cost, NOT WAN anti-throttle performance."
            ),
            "recycle": (
                "in-process recycle_connections() wall time after >=64 KiB of throttled "
                "progress; native only (see aria2 note in results)."
            ),
            "peak_rss": "ru_maxrss of an isolated worker subprocess after one transfer",
            "honesty": (
                "No warm-cache hit is presented as a general speedup. Native values are "
                "null until the PyO3 extension is built and this script is re-run."
            ),
        },
        "benchmarks": {
            "startup_overhead": bench_startup(),
            "throughput_16_stream": throughput,
            "socket_recycle_latency": bench_recycle(root),
            "peak_rss": bench_peak_rss(root),
        },
    }

    RESULTS_PATH.parent.mkdir(parents=True, exist_ok=True)
    RESULTS_PATH.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n")
    print(json.dumps(report, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
