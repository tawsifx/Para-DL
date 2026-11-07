"""Integration tests for pdl.throttle against the shared ThrottleTestServer and MacOSFakeRunner."""

from __future__ import annotations

import json
import plistlib
from pathlib import Path

import pytest
from tests.support.fakes import FakeASNResolver
from tests.support.macos_fake import get_fake_runner
from tests.support.server import SimulatorProfile, ThrottleTestServer

from pdl.cli import run_cli
from pdl.clock import FakeClock, set_clock
from pdl.throttle.budget import DataBudgetTracker
from pdl.throttle.classifier import classify_dataset
from pdl.throttle.models import (
    MeasurementDataset,
    TargetCandidate,
    VerdictClass,
)
from pdl.throttle.probes import (
    measure_http_transfer,
    run_m0_sanity,
    run_m1_baseline,
    run_m2_single_flow,
    run_m3_flow_scaling,
    run_m4_destination_independence,
    run_m5_burst_bucket,
    run_m6_policing_shaping,
    run_m7_protocol_port,
    run_m8_volume_and_upload,
    run_m9_ground_truth,
    run_m10_local_vs_intl,
)
from pdl.throttle.targets import set_asn_resolver, validate_candidates


@pytest.fixture(autouse=True)
def _use_fake_clock_and_asn() -> None:
    clock = FakeClock(initial_time=1794000000.0)
    set_clock(clock)
    fake_asn = FakeASNResolver()
    set_asn_resolver(lambda ip: fake_asn(ip))


@pytest.mark.integration
def test_m2_m3_per_flow_cap_and_shared_cap_against_simulator(
    throttle_server: ThrottleTestServer,
) -> None:
    """M2 and M3 against 'per_flow_cap' and 'shared_cap' profiles classify accurately."""
    # 1. per_flow_cap: 16 connections scale ~8-16x over 1 connection
    throttle_server.set_profile(
        SimulatorProfile(
            name="per_flow_cap",
            file_size=2 * 1024 * 1024,
            per_flow_bps=128 * 1024,
        )
    )
    cands = [TargetCandidate(id="sim-1", url=throttle_server.file_url, group="global-cdn")]
    targets = validate_candidates(cands)
    assert len(targets) == 1
    target = targets[0]

    budget = DataBudgetTracker(limit_bytes=20 * 1024 * 1024)
    m2 = run_m2_single_flow(target, budget, duration_sec=0.5, max_bytes=256 * 1024)
    m3 = run_m3_flow_scaling(
        target,
        budget,
        conn_counts=(1, 4, 16),
        per_step_duration_sec=0.4,
        per_step_max_bytes=512 * 1024,
    )
    assert m3.scaling_ratio_16_to_1 >= 2.0

    ds = MeasurementDataset(
        isp="Duronto Internet",
        m2_single_flow=[m2],
        m3_flow_scaling=[m3],
    )
    cls_res = classify_dataset(ds)
    assert any(v.verdict_class == VerdictClass.PER_FLOW_CAP.value for v in cls_res.verdicts)

    # 2. shared_cap + M4 two-destination independence -> per-destination limit
    throttle_server.set_profile(
        SimulatorProfile(
            name="shared_cap",
            file_size=2 * 1024 * 1024,
            shared_bps=256 * 1024,
        )
    )
    with ThrottleTestServer(
        SimulatorProfile(name="shared_cap", file_size=2 * 1024 * 1024, shared_bps=256 * 1024)
    ) as second_server:
        t_b = validate_candidates([TargetCandidate(id="sim-2", url=second_server.file_url, group="global-cdn")])[0]
        budget_shared = DataBudgetTracker(limit_bytes=20 * 1024 * 1024)
        m3_shared = run_m3_flow_scaling(
            target,
            budget_shared,
            conn_counts=(1, 4, 16),
            per_step_duration_sec=0.4,
            per_step_max_bytes=1024 * 1024,
        )
        m4 = run_m4_destination_independence(
            target,
            t_b,
            budget_shared,
            duration_sec=0.35,
            max_bytes_per_leg=1024 * 1024,
        )
        assert m4.independence_ratio >= 1.45
        ds_shared = MeasurementDataset(
            isp="Duronto Internet",
            m3_flow_scaling=[m3_shared],
            m4_destination_independence=m4,
        )
        res_shared = classify_dataset(ds_shared)
        assert any(v.verdict_class == VerdictClass.PER_DESTINATION_LIMIT.value for v in res_shared.verdicts)


@pytest.mark.integration
def test_m5_burst_and_m6_policing_vs_shaping_against_simulator(
    throttle_server: ThrottleTestServer,
) -> None:
    """M5 burst-then-throttle and M6 policing vs shaping against simulator profiles."""
    # M5: burst_then_throttle
    throttle_server.set_profile(
        SimulatorProfile(
            name="burst_then_throttle",
            file_size=2 * 1024 * 1024,
            burst_bps=1024 * 1024,
            throttled_bps=64 * 1024,
            burst_duration_sec=0.5,
        )
    )
    target = validate_candidates([TargetCandidate(id="sim-burst", url=throttle_server.file_url, group="global-cdn")])[0]
    budget = DataBudgetTracker(limit_bytes=25 * 1024 * 1024)
    m2 = run_m2_single_flow(target, budget, duration_sec=1.5, max_bytes=1024 * 1024)
    m5 = run_m5_burst_bucket(m2, target=target, budget=budget, test_refill=True)
    assert m5.burst_to_sustained_ratio >= 1.75
    assert m5.refill_30s_ratio is not None

    # M6: loss_based_policing (retransmits delta + low RTT growth)
    runner = get_fake_runner()
    runner.netstat_retransmit_step = 45
    runner.ping_rtt_override_ms = 22.0
    throttle_server.set_profile(
        SimulatorProfile(
            name="loss_based_policing",
            file_size=1024 * 1024,
            loss_threshold_bytes=128 * 1024,
        )
    )
    m6_pol = run_m6_policing_shaping(target, budget, duration_sec=0.4, idle_rtt_hint_ms=20.0)
    assert m6_pol.mechanism_hint == "policing"

    # M6: queue_shaping (high loaded RTT inflation + 0 retransmits)
    runner.netstat_retransmit_step = 0
    runner.ping_rtt_override_ms = 95.0
    throttle_server.set_profile(
        SimulatorProfile(
            name="queue_shaping",
            file_size=1024 * 1024,
            queue_delay_sec=0.04,
            per_flow_bps=256 * 1024,
        )
    )
    m6_shp = run_m6_policing_shaping(target, budget, duration_sec=0.4, idle_rtt_hint_ms=20.0)
    assert m6_shp.rtt_inflation_ms >= 35.0
    assert m6_shp.mechanism_hint == "shaping"
    runner.ping_rtt_override_ms = None


@pytest.mark.integration
def test_m10_domestic_vs_international_and_cached_vs_uncached(
    throttle_server: ThrottleTestServer,
) -> None:
    """M10 paired domestic vs international vs cached probes against simulator."""
    throttle_server.set_profile(
        SimulatorProfile(
            name="domestic_vs_international",
            file_size=2 * 1024 * 1024,
            burst_bps=2048 * 1024,
            throttled_bps=128 * 1024,
        )
    )
    cands = [
        TargetCandidate(
            id="dom-1",
            url=f"{throttle_server.base_url}/domestic/file.bin",
            group="domestic",
        ),
        TargetCandidate(
            id="cache-1",
            url=f"{throttle_server.base_url}/cached/file.bin",
            group="isp-cached",
        ),
        TargetCandidate(
            id="intl-1",
            url=f"{throttle_server.base_url}/international/file.bin",
            group="global-cdn",
        ),
    ]
    targets = validate_candidates(cands)
    assert {t.locality for t in targets} == {"domestic", "isp-cached", "international"}

    budget = DataBudgetTracker(limit_bytes=50 * 1024 * 1024)
    m2_list = [run_m2_single_flow(t, budget, duration_sec=0.4, max_bytes=512 * 1024) for t in targets]
    m3_list = [
        run_m3_flow_scaling(
            t,
            budget,
            conn_counts=(1, 16),
            per_step_duration_sec=0.3,
            per_step_max_bytes=512 * 1024,
        )
        for t in targets
    ]
    m10 = run_m10_local_vs_intl(targets, m2_list, m3_list)
    assert (m10.domestic_median_bps or 0) > (m10.international_median_bps or 0) * 2.5
    assert (m10.isp_cached_median_bps or 0) > (m10.international_median_bps or 0) * 2.5

    # Also exercise M0, M1, M7, M8, M9
    m0 = run_m0_sanity(budget, loopback_url=throttle_server.file_url, plan_mbps=50.0)
    assert m0.loopback_ok is True
    m1 = run_m1_baseline(targets[:1])
    assert len(m1.targets) == 1
    m7 = run_m7_protocol_port(targets[0], budget, duration_sec=0.25)
    assert m7.host != ""
    m8 = run_m8_volume_and_upload(targets[0], budget, test_volume_fup=True, volume_max_bytes=256 * 1024)
    assert m8.volume_tested_bytes > 0
    m9 = run_m9_ground_truth(
        targets[0],
        budget,
        direct_1_bps=200_000.0,
        direct_16_bps=400_000.0,
        proxy_url="http://user:secret_pass_123@127.0.0.1:9",
        duration_sec=0.2,
    )
    assert m9.proxy_url_redacted is not None
    assert "secret_pass_123" not in m9.proxy_url_redacted


@pytest.mark.integration
def test_adversarial_data_budget_enforcement(
    throttle_server: ThrottleTestServer,
) -> None:
    """Budget enforcer strictly caps bytes on endless streams, 200-only, huge redirects, and 503s."""
    throttle_server.set_profile(SimulatorProfile(name="none", file_size=2 * 1024 * 1024))

    # 1. Endless stream without Content-Length
    budget_endless = DataBudgetTracker(limit_bytes=120 * 1024)
    b_read, _, _, _ = measure_http_transfer(
        f"{throttle_server.base_url}/endless.bin",
        budget_endless,
        target_id="endless",
        connections=4,
        duration_sec=2.0,
    )
    assert b_read <= budget_endless.limit_bytes
    assert budget_endless.used_bytes <= budget_endless.limit_bytes

    # 2. Server that ignores Range and returns 200 OK
    no_range_vt = validate_candidates(
        [TargetCandidate(id="no-range", url=throttle_server.no_range_url, group="global-cdn")]
    )[0]
    assert no_range_vt.supports_range is False
    budget_200 = DataBudgetTracker(limit_bytes=90 * 1024)
    b_200, _, _, _ = measure_http_transfer(
        throttle_server.no_range_url,
        budget_200,
        target_id="no-range",
        connections=8,
        duration_sec=2.0,
        supports_range=False,
    )
    assert b_200 <= budget_200.limit_bytes
    assert budget_200.used_bytes <= budget_200.limit_bytes

    # 3. Redirect to huge 10 GiB file
    budget_huge = DataBudgetTracker(limit_bytes=100 * 1024)
    b_huge, _, _, _ = measure_http_transfer(
        f"{throttle_server.base_url}/redirect-huge",
        budget_huge,
        target_id="redirect-huge",
        connections=4,
        duration_sec=2.0,
    )
    assert b_huge <= budget_huge.limit_bytes
    assert budget_huge.used_bytes <= budget_huge.limit_bytes

    # 4. Target returning 503 repeatedly is backed off and dropped after 3 failures
    throttle_server.set_profile(SimulatorProfile(name="always_503"))
    budget_503 = DataBudgetTracker(limit_bytes=512 * 1024, max_failures_per_target=3)
    for _ in range(4):
        measure_http_transfer(
            throttle_server.file_url,
            budget_503,
            target_id="bad-503",
            connections=1,
            duration_sec=0.2,
        )
    assert budget_503.is_dropped("bad-503") is True
    assert len(budget_503.backoff_history) == 3


@pytest.mark.integration
def test_cli_subcommands_sample_coverage_launchd_and_report(
    throttle_server: ThrottleTestServer,
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """Verify CLI subcommands: targets, sample, coverage, launchd-plist, and report --html."""
    throttle_server.set_profile(SimulatorProfile(name="per_flow_cap", file_size=1024 * 1024, per_flow_bps=256 * 1024))
    targets_toml = tmp_path / "custom_targets.toml"
    targets_toml.write_text(
        f"""
[groups.global-cdn]
targets = [
  {{ id = "sim-intl", url = "{throttle_server.file_url}", label = "Sim International" }}
]
[groups.domestic]
targets = [
  {{ id = "sim-dom", url = "{throttle_server.base_url}/domestic/file.bin", label = "Sim Domestic" }}
]
""",
        encoding="utf-8",
    )

    # 1. `pdl throttle targets --json`
    rc = run_cli(["throttle", "targets", "--targets", str(targets_toml), "--json"])
    assert rc == 0
    t_out = json.loads(capsys.readouterr().out)
    assert len(t_out["validated"]) == 2

    # 2. `pdl throttle sample` records once, then skips within 45 min idempotency window
    rc_s1 = run_cli(["throttle", "sample", "--targets", str(targets_toml), "--isp", "Duronto Internet", "--json"])
    assert rc_s1 == 0
    s1_data = json.loads(capsys.readouterr().out)
    assert s1_data["status"] == "recorded"

    rc_s2 = run_cli(["throttle", "sample", "--targets", str(targets_toml), "--json"])
    assert rc_s2 == 0
    s2_data = json.loads(capsys.readouterr().out.strip())
    assert s2_data["status"] == "skipped"
    assert s2_data["reason"] == "idempotency_window"

    # Battery skip check
    runner = get_fake_runner()
    runner.battery_status = "battery"
    try:
        rc_bat = run_cli(["throttle", "sample", "--targets", str(targets_toml), "--json"])
        assert rc_bat == 0
        _ = capsys.readouterr()
        # Advance clock past 45m to hit battery check directly
        from pdl.clock import get_clock

        get_clock().sleep(3000.0)
        rc_bat2 = run_cli(["throttle", "sample", "--targets", str(targets_toml), "--json"])
        assert rc_bat2 == 0
        bat_data = json.loads(capsys.readouterr().out.strip())
        assert bat_data["reason"] == "on_battery"
    finally:
        runner.battery_status = "ac"

    # 3. `pdl throttle coverage --json`
    rc_cov = run_cli(["throttle", "coverage", "--isp", "Duronto Internet", "--json"])
    assert rc_cov == 0
    cov_data = json.loads(capsys.readouterr().out)
    assert cov_data["populated_cells"] >= 1
    assert cov_data["total_cells"] == 48

    # 4. `pdl throttle launchd-plist` generates valid plist and NEVER calls launchctl load/bootstrap
    plist_out = tmp_path / "com.pdl.throttle.sample.plist"
    rc_plist = run_cli(["throttle", "launchd-plist", "-o", str(plist_out), "--isp", "Duronto Internet"])
    assert rc_plist == 0
    assert plist_out.is_file()
    plist_data = plistlib.loads(plist_out.read_bytes())
    assert plist_data["Label"] == "com.pdl.throttle.sample"
    assert plist_data["RunAtLoad"] is True
    assert plist_data["StartCalendarInterval"] == {"Minute": 0}
    assert not any(c[0].endswith("launchctl") for c in runner.recorded_calls)

    # 5. `pdl throttle report --html` rebuilds from stored samples/runs
    rep_dir = tmp_path / "rep"
    rc_rep = run_cli(["throttle", "report", "--html", "-o", str(rep_dir), "--isp", "Duronto Internet"])
    assert rc_rep == 0
    assert (rep_dir / "report.md").is_file()
    assert (rep_dir / "report.json").is_file()
    assert (rep_dir / "report.html").is_file()
    html_text = (rep_dir / "report.html").read_text(encoding="utf-8")
    assert "Duronto Internet" in html_text
    assert "<svg" in html_text
    assert "no data" in html_text
    assert "http://" not in html_text and "https://" not in html_text
