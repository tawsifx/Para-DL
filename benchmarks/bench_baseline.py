#!/usr/bin/env python3
"""Reproducible micro-benchmarks for paradl's deterministic hot paths.

Runs the same pure-Python code paths at three scales (small / medium / stress),
reports latency percentiles, throughput and peak traced memory, and writes a
JSON result file so baseline and post-refactor runs can be compared exactly.

No network access is used: every case operates on locally generated data.

Usage:
    python benchmarks/bench_baseline.py --tag baseline
    python benchmarks/bench_baseline.py --tag after
    python benchmarks/bench_baseline.py --compare benchmarks/results/baseline.json
"""

from __future__ import annotations

import argparse
import json
import math
import os
import random
import re
import statistics
import subprocess
import sys
import tempfile
import time
import tracemalloc
from pathlib import Path
from typing import Any, Callable

SRC = Path(__file__).resolve().parent.parent / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

RESULTS_DIR = Path(__file__).resolve().parent / "results"

# Scales: (label, size) -- small / medium / high-concurrency-stress analogue
SCALES: list[tuple[str, int]] = [("small", 200), ("medium", 2_000), ("stress", 20_000)]


# --------------------------------------------------------------------------- #
# Case registry.  Each builder returns (callable, iterations, units_per_call).
# --------------------------------------------------------------------------- #
BUILDERS: dict[str, Callable[[int], tuple[Callable[[], Any], int, int]]] = {}


def case(name: str):
    def deco(fn):
        BUILDERS[name] = fn
        return fn

    return deco


@case("classifier.mann_whitney_u_test")
def _mann_whitney(n: int):
    rng = random.Random(1234)
    a = [rng.gauss(100, 12) for _ in range(n)]
    b = [rng.gauss(104, 12) for _ in range(n)]
    from paradl.throttle.classifier import mann_whitney_u_test

    return lambda: mann_whitney_u_test(a, b), max(3, 20_000 // n), 2 * n


@case("checksum.parse_checksum_text")
def _checksum(n: int):
    digest = "a" * 64
    # target entry deliberately last -> forces a full scan of every line
    text = "\n".join(f"{digest}  file{i:06d}.bin" for i in range(n - 1))
    text += f"\n{digest}  target.bin\n"
    from paradl.postproc.checksum import ChecksumEngine

    return lambda: ChecksumEngine.parse_checksum_text(text, "target.bin"), max(3, 30_000 // n), n


@case("parsers.parse_netstat_tcp")
def _netstat(n: int):
    lines = ["packets sent: 123456", "segments retransmitted: (7 packets, 512 bytes)"]
    lines += [f"data packets: ({i} packets, {i * 64} bytes)" for i in range(n)]
    lines.append("default gateway: 192.168.1.1 via en0")
    text = "\n".join(lines)
    from paradl.throttle.parsers import parse_netstat_tcp

    return lambda: parse_netstat_tcp(text), max(3, 20_000 // n), n


@case("mirrors.pattern_matching")
def _mirror_patterns(n: int):
    from paradl.storage.mirrors import _pattern_matches_url

    # Bound the cross-product: an unbounded patterns x URLs matrix makes the
    # harness itself the bottleneck (240M regex calls at stress scale).
    pats = [f"https://mirror{i % 17}.example.com/*" for i in range(min(40, max(8, n // 10)))]
    pats += [r"https://dl\.example\.org/.*", "https://cdn.example.net/*"]
    urls = [f"https://mirror{i % 17}.example.com/pkg/file_{i}.zip" for i in range(n)]
    work = len(urls) * len(pats)
    iters = max(2, min(500, 2_000_000 // work))

    def run():
        for u in urls:
            for p in pats:
                _pattern_matches_url(u, p)

    return run, iters, work


@case("mirrors.match_configured_mirrors_disk")
def _mirror_match_disk(n: int):
    from paradl.storage.mirrors import MirrorManager

    # n = number of mirror rules in the config file
    cfg = {
        "rules": [
            {"pattern": f"https://host{i}.example.com/*", "mirrors": [f"https://alt{i}.example.com/"]}
            for i in range(n)
        ]
    }
    tmpdir = tempfile.mkdtemp(prefix="paradl_bench_mirrors_")
    path = Path(tmpdir) / "mirrors.json"
    path.write_text(json.dumps(cfg), encoding="utf-8")
    url = f"https://host{n - 1}.example.com/pkg/file.zip"

    def run():
        MirrorManager.match_configured_mirrors(url, mirrors_file=path)

    return run, max(3, 2000 // max(1, n)), n


@case("mirrors.match_configured_mirrors_cold")
def _mirror_match_cold(n: int):
    from paradl.storage.mirrors import MirrorManager

    cfg = {
        "rules": [
            {"pattern": f"https://host{i}.example.com/*", "mirrors": [f"https://alt{i}.example.com/"]}
            for i in range(n)
        ]
    }
    tmpdir = tempfile.mkdtemp(prefix="paradl_bench_mirror_cold_")
    path = Path(tmpdir) / "mirrors.json"
    path.write_text(json.dumps(cfg), encoding="utf-8")
    urls = [f"https://host{i % n}.example.com/pkg/file_{i}.bin" for i in range(512)]
    idx = 0

    def run():
        nonlocal idx
        MirrorManager.match_configured_mirrors(urls[idx % len(urls)], mirrors_file=path)
        idx += 1

    return run, max(3, 2000 // max(1, n)), n


@case("models.sanitize_filename")
def _sanitize(n: int):
    from paradl.core.models import sanitize_filename

    rng = random.Random(7)
    seeds = [
        "plain_file.bin",
        "a/b:c*d?e\"f<g>h|i\0j",
        "CON",
        "COM3.tar.gz",
        "../etc/passwd",
        "  spaced name.zip  ",
        "café-文件.bin",
        "..." ,
    ]
    names = [rng.choice(seeds) for _ in range(n)]

    def run():
        for name in names:
            sanitize_filename(name)

    return run, max(2, 200_000 // n), n


@case("cli.cold_import_subprocess")
def _cold_import(n: int):
    py = sys.executable
    env = dict(os.environ, PYTHONPATH=str(SRC))
    cmd = [py, "-c", "import paradl.cli"]

    def run():
        r = subprocess.run(cmd, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if r.returncode != 0:
            raise RuntimeError("cold import failed")

    return run, max(3, min(30, n // 20)), 1


# --------------------------------------------------------------------------- #
# Measurement
# --------------------------------------------------------------------------- #
def _percentile(values: list[float], pct: float) -> float:
    s = sorted(values)
    if not s:
        return 0.0
    k = (len(s) - 1) * pct
    lo = int(k)
    hi = min(lo + 1, len(s) - 1)
    return s[lo] + (s[hi] - s[lo]) * (k - lo)


def _max_rss_mb() -> float:
    import resource

    rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    # macOS reports bytes, Linux reports KiB
    return rss / (1024 * 1024) if sys.platform == "darwin" else rss / 1024


def measure_one(name: str, scale: int, fn, iters: int, units: int, reps: int = 3) -> dict[str, Any]:
    fn()  # warm-up / correctness smoke
    samples: list[float] = []
    total_time = 0.0
    tracemalloc.start()
    before = _max_rss_mb()
    for _ in range(reps):
        for _i in range(iters):
            t0 = time.perf_counter()
            fn()
            samples.append(time.perf_counter() - t0)
        total_time += sum(samples[-iters:])
    _, traced_peak = tracemalloc.get_traced_memory()
    tracemalloc.stop()
    after = _max_rss_mb()
    return {
        "case": name,
        "scale": scale,
        "iterations": iters * reps,
        "units_per_call": units,
        "p50_ms": _percentile(samples, 0.5) * 1000,
        "p95_ms": _percentile(samples, 0.95) * 1000,
        "p99_ms": _percentile(samples, 0.99) * 1000,
        "mean_ms": statistics.fmean(samples) * 1000,
        "throughput_units_per_s": (units * iters * reps / total_time) if total_time else math.inf,
        "traced_peak_kb": traced_peak / 1024,
        "rss_delta_mb": after - before,
    }


def run(tag: str, only: str | None) -> dict[str, Any]:
    result: dict[str, Any] = {"tag": tag, "python": sys.version.split()[0], "results": []}
    for name, builder in BUILDERS.items():
        if only and only not in name:
            continue
        for label, size in SCALES:
            fn, iters, units = builder(size)
            row = measure_one(name, size, fn, iters, units)
            row["label"] = label
            result["results"].append(row)
            print(
                f"{name:42s} {label:6s} "
                f"p50={row['p50_ms']:8.3f}ms p95={row['p95_ms']:8.3f}ms "
                f"p99={row['p99_ms']:8.3f}ms "
                f"thru={row['throughput_units_per_s']:12.0f}/s "
                f"peak={row['traced_peak_kb']:9.1f}KB",
                flush=True,
            )
    RESULTS_DIR.mkdir(parents=True, exist_ok=True)
    out = RESULTS_DIR / f"{tag}.json"
    out.write_text(json.dumps(result, indent=2), encoding="utf-8")
    print(f"\nSaved -> {out}")
    return result


def compare(path: Path, current: dict[str, Any]) -> None:
    baseline = json.loads(path.read_text(encoding="utf-8"))
    old = {(r["case"], r["label"]): r for r in baseline["results"]}
    print(f"\n{'case':42s} {'scale':6s} {'old p50':>10s} {'new p50':>10s} {'speedup':>9s}")
    print("-" * 84)
    for r in current["results"]:
        o = old.get((r["case"], r["label"]))
        if not o or r["p50_ms"] == 0:
            continue
        sp = o["p50_ms"] / r["p50_ms"]
        print(
            f"{r['case']:42s} {r['label']:6s} "
            f"{o['p50_ms']:10.3f} {r['p50_ms']:10.3f} {sp:8.2f}x"
        )


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--tag", default="run", help="result file name (basename, .json added)")
    ap.add_argument("--only", default=None, help="run only cases whose name contains this")
    ap.add_argument("--compare", type=Path, default=None, help="compare against a baseline JSON")
    args = ap.parse_args()
    result = run(args.tag, args.only)
    if args.compare:
        compare(args.compare, result)


if __name__ == "__main__":
    main()
