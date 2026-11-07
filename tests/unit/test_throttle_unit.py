"""Unit tests for the pdl.throttle ISP Throttling Diagnostic Suite."""

from __future__ import annotations

import struct

import pytest
from tests.support.fakes import (
    CLASSIFIER_SCENARIO_TABLE,
    FakeASNResolver,
    gappy_time_samples_factory,
    measurement_dataset_factory,
)
from tests.support.macos_fake import read_golden

from pdl.clock import FakeClock, set_clock
from pdl.throttle.budget import DataBudgetTracker, parse_byte_budget
from pdl.throttle.classifier import classify_dataset, mann_whitney_u_test
from pdl.throttle.models import (
    REPORT_SCHEMA_VERSION,
    SingleFlowSeries,
    TargetCandidate,
    VerdictClass,
)
from pdl.throttle.parsers import (
    parse_netstat_tcp,
    parse_nettop,
    parse_networksetup,
    parse_ping,
    parse_scutil_env,
    parse_traceroute,
)
from pdl.throttle.probes import analyze_burst_series
from pdl.throttle.report import (
    generate_html_report,
    generate_markdown_report,
    report_from_dict,
)
from pdl.throttle.store import ThrottleStore
from pdl.throttle.targets import (
    _build_dns_txt_query,
    _parse_dns_txt_response,
    load_target_candidates,
    resolve_ip_asn_country,
    set_asn_resolver,
)


@pytest.mark.unit
@pytest.mark.parametrize(
    ("scenario", "expected_class", "expected_note", "_exclude_tod"),
    CLASSIFIER_SCENARIO_TABLE,
)
def test_classifier_scenarios_table(
    scenario: str,
    expected_class: str,
    expected_note: str | None,
    _exclude_tod: bool,
) -> None:
    """Table-driven verification of every classifier scenario and auxiliary note."""
    ds = measurement_dataset_factory(scenario)
    result = classify_dataset(ds)

    verdict_classes = [v.verdict_class for v in result.verdicts]
    assert expected_class in verdict_classes, (
        f"Scenario {scenario!r} expected {expected_class!r}, got {verdict_classes}"
    )

    # Specifically verify per-flow cap on domestic & international alike is NOT labeled international bandwidth limit
    if scenario == "per_flow_cap_domestic_and_intl_alike":
        assert VerdictClass.INTERNATIONAL_BW_LIMIT.value not in verdict_classes

    # Specifically verify cached fast + uncached slow is NOT labeled SNI/service throttling
    if scenario == "cached_fast_uncached_slow":
        assert VerdictClass.SNI_SERVICE_DIFF.value not in verdict_classes

    if expected_note is not None:
        note_codes = [n.code for n in result.notes]
        assert expected_note in note_codes, f"Scenario {scenario!r} expected note {expected_note!r}, got {note_codes}"


@pytest.mark.unit
def test_classifier_accuracy_above_90_percent_and_confidence_scoring() -> None:
    """Classifier accuracy across non-time-of-day scenarios must be >= 90% and contaminated confidence lower."""
    non_tod = [row for row in CLASSIFIER_SCENARIO_TABLE if not row[3]]
    correct = 0
    for scenario, expected_class, expected_note, _ in non_tod:
        res = classify_dataset(measurement_dataset_factory(scenario))
        classes = [v.verdict_class for v in res.verdicts]
        notes = [n.code for n in res.notes]
        if expected_class in classes and (expected_note is None or expected_note in notes):
            correct += 1

    accuracy = correct / len(non_tod)
    assert accuracy >= 0.90, f"Classifier accuracy {accuracy:.2%} < 90%"

    # Verify contaminated environment lowers confidence compared to clean environment
    clean_ds = measurement_dataset_factory("per_flow_cap")
    dirty_ds = measurement_dataset_factory("per_flow_cap")
    dirty_ds.m0_sanity.contaminated = True
    dirty_ds.m0_sanity.warnings = ["Competing traffic detected"]

    clean_res = classify_dataset(clean_ds)
    dirty_res = classify_dataset(dirty_ds)
    assert dirty_res.verdicts[0].confidence < clean_res.verdicts[0].confidence


@pytest.mark.unit
@pytest.mark.parametrize("gappy_kind", ["evening_only", "weekend_only", "single_bucket"])
def test_classifier_gappy_time_datasets_report_insufficient_time_data(gappy_kind: str) -> None:
    """Sparse/gappy time datasets must report 'insufficient time data' and NEVER claim time-of-day congestion."""
    ds = measurement_dataset_factory("per_flow_cap")
    ds.m8_time_volume_upload.samples = gappy_time_samples_factory(gappy_kind)

    res = classify_dataset(ds)
    assert res.time_of_day_status == "insufficient time data"
    verdict_classes = [v.verdict_class for v in res.verdicts]
    assert VerdictClass.TIME_OF_DAY_CONGESTION.value not in verdict_classes
    assert any(n.code == "insufficient_time_data" for n in res.notes)
    assert len(res.next_best_hours) >= 1


@pytest.mark.unit
def test_mann_whitney_u_test_significance() -> None:
    """Mann-Whitney U test distinguishes separated distributions from overlapping noise."""
    fast = [5_000_000.0, 5_100_000.0, 4_950_000.0, 5_200_000.0, 5_050_000.0]
    slow = [1_000_000.0, 1_100_000.0, 950_000.0, 1_050_000.0, 1_150_000.0]
    u_stat, p_val = mann_whitney_u_test(fast, slow)
    assert u_stat == 0.0
    assert p_val < 0.05

    noisy_a = [2_000_000.0, 2_100_000.0, 1_900_000.0]
    noisy_b = [2_050_000.0, 1_950_000.0, 2_000_000.0]
    _, p_noisy = mann_whitney_u_test(noisy_a, noisy_b)
    assert p_noisy > 0.20


@pytest.mark.unit
def test_parsers_against_golden_files() -> None:
    """Verify pure parsers against golden outputs in tests/support/golden/."""
    ping_res = parse_ping(read_golden("ping.txt"))
    assert ping_res["loss_pct"] == 0.0
    assert ping_res["rtt_avg_ms"] > 0.0
    assert ping_res["mtu_exceeded"] is False

    ping_df = parse_ping(read_golden("ping_df_mtu.txt"))
    assert ping_df["mtu_exceeded"] is True
    assert ping_df["loss_pct"] == 100.0

    hops = parse_traceroute(read_golden("traceroute.txt"))
    assert len(hops) >= 1
    assert hops[0].hop == 1
    assert hops[0].ip == "127.0.0.1"

    tcp_stats = parse_netstat_tcp(read_golden("netstat.txt"))
    assert tcp_stats["packets_sent"] == 12450
    assert tcp_stats["data_packets"] == 9800
    assert tcp_stats["retransmitted_packets"] == 42
    assert tcp_stats["connection_resets"] == 2

    comp_procs = parse_nettop(read_golden("nettop.txt"))
    assert len(comp_procs) >= 1
    assert sum(p.bytes_in + p.bytes_out for p in comp_procs) > 0

    ns_info = parse_networksetup(read_golden("networksetup_airport.txt"))
    assert "ssid" in ns_info

    env = parse_scutil_env(read_golden("scutil_nwi.txt") + "\n" + read_golden("scutil_query_airport.txt"))
    assert env["primary_interface"] == "en0"


@pytest.mark.unit
def test_team_cymru_dns_packet_and_fake_asn_resolver() -> None:
    """Verify DNS TXT wire-format builder/parser and shared FakeASNResolver hook."""
    qname = "1.1.1.1.origin.asn.cymru.com"
    pkt = _build_dns_txt_query(qname)
    assert pkt[:2] == b"\x42\x42"

    # Build a synthetic DNS response with 1 question and 1 TXT answer
    txt_str = b"13335 | 1.1.1.0/24 | SG | apnic | 2010-07-01"
    rdata = bytes([len(txt_str)]) + txt_str
    header = struct.pack("!HHHHHH", 0x4242, 0x8180, 1, 1, 0, 0)
    question = pkt[12:]
    answer = (
        b"\xc0\x0c"  # pointer to qname
        + struct.pack("!HHIH", 16, 1, 300, len(rdata))
        + rdata
    )
    parsed_txt = _parse_dns_txt_response(header + question + answer)
    assert parsed_txt == "13335 | 1.1.1.0/24 | SG | apnic | 2010-07-01"

    fake_asn = FakeASNResolver()
    set_asn_resolver(lambda ip: fake_asn(ip))
    try:
        asn, country, org = resolve_ip_asn_country("103.10.20.1")
        assert asn == "AS58717"
        assert country == "BD"
        assert "BDIX" in org
    finally:
        set_asn_resolver(None)


@pytest.mark.unit
def test_m5_burst_and_drop_shape_estimation() -> None:
    """Verify M5 burst rate, duration, bucket bytes, and step/decay/periodic/flat shapes."""
    # 1. Step drop: 3 seconds at 4 MB/s (12 samples at 0.25s), then 1 MB/s (28 samples)
    step_samples = [4_000_000.0] * 12 + [1_000_000.0] * 28
    step_series = SingleFlowSeries(
        target_id="t1",
        host="speed.cloudflare.com",
        locality="international",
        duration_sec=10.0,
        sample_interval_sec=0.25,
        samples_bps=step_samples,
    )
    step_res = analyze_burst_series(step_series)
    assert step_res.drop_shape == "step"
    assert step_res.burst_to_sustained_ratio >= 3.5
    assert step_res.estimated_bucket_bytes > 5_000_000

    # 2. Periodic oscillation
    periodic_samples = [3_000_000.0 if (i // 2) % 2 == 0 else 800_000.0 for i in range(32)]
    per_series = SingleFlowSeries(
        target_id="t2",
        host="speed.cloudflare.com",
        locality="international",
        duration_sec=8.0,
        sample_interval_sec=0.25,
        samples_bps=periodic_samples,
    )
    per_res = analyze_burst_series(per_series)
    assert per_res.drop_shape == "periodic"

    # 3. Flat steady transfer
    flat_samples = [2_000_000.0] * 24
    flat_series = SingleFlowSeries(
        target_id="t3",
        host="speed.cloudflare.com",
        locality="international",
        duration_sec=6.0,
        sample_interval_sec=0.25,
        samples_bps=flat_samples,
    )
    flat_res = analyze_burst_series(flat_series)
    assert flat_res.drop_shape == "flat"


@pytest.mark.unit
def test_sample_idempotency_45_minute_window_and_coverage_matrix() -> None:
    """Verify 45-minute sample idempotency on FakeClock and 24x2 coverage matrix recommendations."""
    clock = FakeClock(initial_time=1794000000.0)
    set_clock(clock)

    can_run, _ = ThrottleStore.can_record_active_sample()
    assert can_run is True

    samples = gappy_time_samples_factory("evening_only")
    samples[0].epoch = clock.now()
    ThrottleStore.record_sample(samples[0])

    # 10 minutes later -> blocked by 45-min idempotency window
    clock.sleep(600.0)
    can_run_10m, elapsed_10m = ThrottleStore.can_record_active_sample()
    assert can_run_10m is False
    assert elapsed_10m == pytest.approx(600.0)

    # 46 minutes later -> allowed again
    clock.sleep(2200.0)
    can_run_46m, _ = ThrottleStore.can_record_active_sample()
    assert can_run_46m is True

    # Coverage matrix & next-best-hour recommendations
    stored = ThrottleStore.get_samples()
    matrix, suggestions = ThrottleStore.build_coverage_matrix(stored)
    assert len(matrix) == 48
    assert any("09:00" in s or "21:00" in s for s in suggestions)


@pytest.mark.unit
def test_budget_backoff_jitter_and_report_schema() -> None:
    """Verify exponential backoff on 429/503, seeded jitter via FakeClock, and report.json schema roundtrip."""
    clock = FakeClock()
    set_clock(clock)

    assert parse_byte_budget("300MB", 1024) == 300 * 1024 * 1024
    assert parse_byte_budget("2GB", 1024) == 2 * 1024 * 1024 * 1024

    tracker = DataBudgetTracker(limit_bytes=1024 * 1024, seed=99, max_failures_per_target=3)
    t0 = clock.monotonic()
    delay = tracker.jitter_pause(0.2, 0.4)
    assert 0.2 <= delay <= 0.4
    assert clock.monotonic() == pytest.approx(t0 + delay)

    # 3 consecutive 429 failures drop the target and record increasing backoffs
    assert tracker.record_failure("cf-1", 429) is False
    assert tracker.record_failure("cf-1", 429) is False
    assert tracker.record_failure("cf-1", 503) is True
    assert tracker.is_dropped("cf-1") is True
    assert len(tracker.backoff_history) == 3
    assert tracker.backoff_history[1][2] > tracker.backoff_history[0][2]

    # Verify default_targets.toml loads and report roundtrips cleanly
    cands = load_target_candidates(domestic_urls=["https://bdix.duronto.net.bd/test.bin"])
    assert any(isinstance(c, TargetCandidate) and c.group == "domestic" for c in cands)

    ds = measurement_dataset_factory("domestic_fast_intl_slow_shared_upstream")
    cls_res = classify_dataset(ds)
    from pdl.throttle.models import ThrottleReport

    rep = ThrottleReport(
        run_id="unit-run-1",
        mode="quick",
        isp="Duronto Internet",
        plan_mbps=40.0,
        timestamp="2026-11-07T20:00:00+0600",
        budget=tracker.to_summary(),
        dataset=ds,
        classification=cls_res,
    )
    d = rep.to_dict()
    assert d["schema_version"] == REPORT_SCHEMA_VERSION
    assert d["isp"] == "Duronto Internet"
    rehydrated = report_from_dict(d)
    assert rehydrated.isp == "Duronto Internet"
    assert rehydrated.classification.verdicts[0].verdict_class == VerdictClass.INTERNATIONAL_BW_LIMIT.value

    md = generate_markdown_report(rehydrated)
    html_doc = generate_html_report(rehydrated)
    assert "Duronto Internet" in md
    assert "Local vs International" in md
    assert "<svg" in html_doc
    assert "http://" not in html_doc and "https://" not in html_doc
