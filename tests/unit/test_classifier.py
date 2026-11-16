"""Wave-2 Task 4 parity tests: classifier next-best-hours and ordered mirror dedup.

These tests lock the observable behaviour that the Wave-2 optimizations must preserve:

* ``_compute_next_best_hours`` must reproduce, byte-for-byte, the suggestion list
  produced by the (now discarded) coverage matrix rebuilt by
  ``ThrottleStore.build_coverage_matrix`` for any synthetic sample set.
* The three ``x not in some_list`` dedup sites in ``storage.mirrors`` must keep
  the exact first-occurrence order and returned values after switching to a
  parallel set.
"""

from __future__ import annotations

import random
from unittest.mock import patch

import pytest

from paradl.storage.mirrors import (
    HttpProbeResult,
    MetalinkCompanion,
    MirrorManager,
    parse_metalink_bytes,
)
from paradl.throttle.classifier import _compute_next_best_hours, classify_dataset
from paradl.throttle.models import MeasurementDataset, SampleRecord, TimeVolumeUploadResult
from paradl.throttle.store import ThrottleStore


def _sample(
    hour: int,
    *,
    is_weekend: bool = False,
    source: str = "active",
    download_bps: float = 1_000_000.0,
) -> SampleRecord:
    return SampleRecord(
        timestamp=f"2026-11-17T{hour % 24:02d}:00:00",
        epoch=1_763_337_600.0 + hour * 3600,
        hour=hour,
        is_weekend=is_weekend,
        interface="en0",
        ssid="",
        network_id="net0",
        source=source,
        download_bps=download_bps,
    )


# ---------------------------------------------------------------------------
# (a) next_best_hours parity with the coverage-matrix implementation
# ---------------------------------------------------------------------------


def test_next_best_hours_matches_coverage_matrix_empty() -> None:
    assert _compute_next_best_hours([]) == ThrottleStore.build_coverage_matrix([])[1]


@pytest.mark.parametrize("seed", list(range(12)))
def test_next_best_hours_matches_coverage_matrix_randomized(seed: int) -> None:
    rng = random.Random(seed)
    samples = [
        _sample(
            rng.randint(0, 27),
            is_weekend=rng.random() < 0.5,
            source=rng.choice(["active", "passive", "weird-source"]),
            download_bps=rng.choice([0.0, 0.0, 250_000.0, 5_000_000.0, rng.random() * 1e7]),
        )
        for _ in range(rng.randint(0, 220))
    ]
    old = ThrottleStore.build_coverage_matrix(samples)[1]
    assert _compute_next_best_hours(samples) == old


def test_next_best_hours_matches_coverage_matrix_edge_cases() -> None:
    cases: list[list[SampleRecord]] = [
        # All samples filtered out (zero download) -> same as empty.
        [_sample(21, download_bps=0.0), _sample(9, download_bps=0.0)],
        # Only passive samples at low weights.
        [_sample(h, source="passive") for h in range(19, 24)] * 2,
        # Heavy weekday peak/offpeak/afternoon coverage with some holes -> fallback branch.
        [_sample(h, source="active") for h in range(1, 11)] * 4
        + [_sample(h, source="active") for h in range(11, 19)] * 4
        + [_sample(h, source="active") for h in range(19, 24)] * 4,
        # Weekend-only peak coverage.
        [_sample(h, is_weekend=True, source="active") for h in range(19, 24)] * 3,
        # Mixed fractional effective weights (passive 0.35 rounding boundaries).
        [_sample(h, source="passive") for h in (21, 22, 23, 9, 15)],
    ]
    for samples in cases:
        assert _compute_next_best_hours(samples) == ThrottleStore.build_coverage_matrix(samples)[1]


def test_classify_dataset_next_best_hours_parity() -> None:
    samples = [_sample(h, is_weekend=h % 2 == 0, source="active") for h in (21, 22, 9, 15)]
    dataset = MeasurementDataset(
        m8_time_volume_upload=TimeVolumeUploadResult(samples=samples)
    )
    expected = ThrottleStore.build_coverage_matrix(samples)[1]
    assert classify_dataset(dataset).next_best_hours == expected


# ---------------------------------------------------------------------------
# (b) ordered dedup at the three mirror sites
# ---------------------------------------------------------------------------


def test_resolve_and_validate_candidate_mirror_dedup_order() -> None:
    primary = "https://example.com/file.zip"
    m1, m2, m3, m4, m5 = (f"https://mirror{i}.example.com/file.zip" for i in range(1, 6))

    metalink = MetalinkCompanion(
        source="synthetic",
        raw_bytes=b"",
        urls=[m5, m3, m1, primary],  # duplicates + primary -> must all be dropped in order
    )

    ok_probe = HttpProbeResult(ok=True, final_url="", status_code=200, accept_ranges=True)

    with (
        patch.object(
            MirrorManager, "match_configured_mirrors", return_value=[m4, m1]
        ),
        patch("paradl.storage.mirrors.find_or_fetch_metalink", return_value=metalink),
        patch.object(
            MirrorManager,
            "_probe_uri_with_failure_tracking",
            return_value=(ok_probe, False),
        ),
    ):
        result = MirrorManager.resolve_and_validate(
            primary,
            item_mirrors=[m1, m2, m1],
            cli_mirrors=[m2, m3, primary],
            mirrors_file=None,
        )

    assert result.active_uris == [primary, m1, m2, m3, m4, m5]
    assert result.rejected_size_mirrors == []
    assert result.dropped_failed_mirrors == []


_METALINK_XML = b"""<?xml version="1.0" encoding="UTF-8"?>
<metalink version="3.0" xmlns="http://www.metalinker.org/">
  <files>
    <file name="file.zip">
      <size>1234</size>
      <resources>
        <url>https://m1.example.com/f.zip</url>
        <url>https://m2.example.com/f.zip</url>
        <url>https://m1.example.com/f.zip</url>
        <url>ftp://m3.example.com/f.zip</url>
        <url>ftp://m3.example.com/f.zip</url>
        <url>http://m4.example.com/f.zip</url>
        <url>file:///not/a/http/url</url>
      </resources>
    </file>
  </files>
</metalink>
"""


def test_parse_metalink_bytes_url_dedup_order() -> None:
    companion = parse_metalink_bytes(_METALINK_XML)
    assert companion is not None
    assert companion.urls == [
        "https://m1.example.com/f.zip",
        "https://m2.example.com/f.zip",
        "ftp://m3.example.com/f.zip",
        "http://m4.example.com/f.zip",
    ]
