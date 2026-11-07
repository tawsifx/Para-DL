"""End-to-end tests for queue execution, priority scheduling, crash-resume, and --when-fast deferral."""

from __future__ import annotations

import hashlib
from pathlib import Path

import pytest
from tests.support.server import SimulatorProfile, ThrottleTestServer

from pdl.cli import run_cli
from pdl.models import ExitCode
from pdl.platform import macos
from pdl.platform.macos import PowerStatus
from pdl.queue import QueueJob, QueueManager, check_when_fast_gate


def _sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


@pytest.mark.slow
def test_queue_cli_crud_priority_ordering_and_crash_resume(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Queue add/list/pause/resume/remove, priority ordering (DESC), and crash-resume."""
    pdl_home = tmp_path / "pdl_home"
    pdl_home.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("PDL_HOME", str(pdl_home))

    monkeypatch.setattr(
        macos,
        "get_power_status",
        lambda: PowerStatus(on_ac_power=True, battery_percent=100, low_power_mode=False),
    )

    with (
        ThrottleTestServer(profile=SimulatorProfile(file_size=128 * 1024, name="none")) as srv1,
        ThrottleTestServer(profile=SimulatorProfile(file_size=128 * 1024, name="none")) as srv2,
        ThrottleTestServer(profile=SimulatorProfile(file_size=256 * 1024, name="none")) as srv_crash,
    ):
        out_dir = tmp_path / "queue_out"
        out_dir.mkdir(parents=True, exist_ok=True)

        # 1. Add two jobs via CLI with different priorities (1 and 10)
        rc1 = run_cli(
            [
                "queue",
                "add",
                srv1.file_url,
                "-o",
                str(out_dir),
                "--priority",
                "1",
                "--no-caffeinate",
                "--no-net-watch",
            ]
        )
        assert rc1 == int(ExitCode.OK)

        rc2 = run_cli(
            [
                "queue",
                "add",
                srv2.file_url,
                "-o",
                str(out_dir),
                "--priority",
                "10",
                "--no-caffeinate",
                "--no-net-watch",
            ]
        )
        assert rc2 == int(ExitCode.OK)

        jobs = QueueManager.list_jobs()
        assert len(jobs) == 2
        assert jobs[0].priority == 10
        assert jobs[1].priority == 1
        assert jobs[0].require_ac is True
        assert jobs[1].require_ac is True

        QueueManager.update_job_status(jobs[0].id, "queued", filename="high_prio.bin")
        QueueManager.update_job_status(jobs[1].id, "queued", filename="low_prio.bin")

        low_id = jobs[1].id
        assert run_cli(["queue", "pause", str(low_id)]) == int(ExitCode.OK)
        j_paused = QueueManager.get_job(low_id)
        assert j_paused is not None and j_paused.status == "paused"

        assert run_cli(["queue", "resume", str(low_id)]) == int(ExitCode.OK)
        j_resumed = QueueManager.get_job(low_id)
        assert j_resumed is not None and j_resumed.status == "queued"

        assert run_cli(["queue", "list"]) == int(ExitCode.OK)

        # 2. Simulate a crashed job left in status='running' with partial bytes on disk
        crash_job = QueueManager.add_job(
            srv_crash.file_url,
            out_dir,
            filename="crashed_job.bin",
            priority=20,
            connections=4,
            expected_length=srv_crash.profile.file_size,
            expected_digest=f"sha256:{srv_crash.sha256_hex}",
        )
        partial_file = out_dir / "crashed_job.bin"
        partial_file.write_bytes(srv_crash.payload[: 64 * 1024])
        QueueManager.update_job_status(
            crash_job.id,
            "running",
            bytes_downloaded=64 * 1024,
            filename="crashed_job.bin",
        )

        started_order: list[int] = []

        def on_start(job: QueueJob) -> None:
            started_order.append(job.id)

        rc_run, executed = QueueManager.run_queue(
            global_connections=8,
            caffeinate=False,
            net_watch=False,
            on_job_start=on_start,
        )
        assert rc_run == int(ExitCode.OK)
        assert len(executed) == 3

        # Priority order must be: crash_job (20) -> jobs[0] (10) -> jobs[1] (1)
        assert started_order == [crash_job.id, jobs[0].id, jobs[1].id]

        assert _sha256_file(out_dir / "crashed_job.bin") == srv_crash.sha256_hex
        assert _sha256_file(out_dir / "high_prio.bin") == srv2.sha256_hex
        assert _sha256_file(out_dir / "low_prio.bin") == srv1.sha256_hex


@pytest.mark.slow
def test_simulator_when_fast_gating_and_queue_deferral(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """--when-fast gates download when server is slow and proceeds once fast."""
    pdl_home = tmp_path / "pdl_home"
    pdl_home.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("PDL_HOME", str(pdl_home))
    monkeypatch.setattr(
        macos,
        "get_power_status",
        lambda: PowerStatus(on_ac_power=True, battery_percent=100, low_power_mode=False),
    )

    with ThrottleTestServer(
        profile=SimulatorProfile(file_size=256 * 1024, name="uniformly_slow", uniform_slow_bps=32 * 1024)
    ) as srv:
        threshold_bps = 500 * 1024

        # 1. While server is uniformly_slow (~32 KiB/s), threshold 500 KiB/s must fail gate
        passed_slow, measured_slow, err_slow = check_when_fast_gate(
            srv.file_url,
            threshold_bps=threshold_bps,
            probe_bytes=32 * 1024,
            max_polls=1,
        )
        assert not passed_slow
        assert measured_slow < threshold_bps
        assert err_slow is not None and "Deferred by --when-fast" in err_slow

        # 2. Queue a job with when_fast_bps=500 KiB/s
        out_dir = tmp_path / "when_fast_out"
        out_dir.mkdir(parents=True, exist_ok=True)
        job = QueueManager.add_job(
            srv.file_url,
            out_dir,
            filename="when_fast.bin",
            when_fast_bps=threshold_bps,
        )

        rc_slow, executed_slow = QueueManager.run_queue(
            global_connections=4,
            caffeinate=False,
            net_watch=False,
            when_fast_max_polls=1,
            speed_prober=lambda _u: 32 * 1024.0,
        )
        assert rc_slow == int(ExitCode.OK)
        assert len(executed_slow) == 0
        j_deferred = QueueManager.get_job(job.id)
        assert j_deferred is not None and j_deferred.status == "queued"
        assert not (out_dir / "when_fast.bin").exists()

        # 3. Switch server to unthrottled ("none") mode and re-run queue
        srv.set_profile(SimulatorProfile(file_size=256 * 1024, name="none"))
        passed_fast, measured_fast, err_fast = check_when_fast_gate(
            srv.file_url,
            threshold_bps=threshold_bps,
            probe_bytes=64 * 1024,
            max_polls=1,
        )
        assert passed_fast
        assert measured_fast >= threshold_bps
        assert err_fast is None

        rc_fast, executed_fast = QueueManager.run_queue(
            global_connections=4,
            caffeinate=False,
            net_watch=False,
            when_fast_max_polls=1,
        )
        assert rc_fast == int(ExitCode.OK)
        assert len(executed_fast) == 1
        j_completed = QueueManager.get_job(job.id)
        assert j_completed is not None and j_completed.status == "completed"
        assert _sha256_file(out_dir / "when_fast.bin") == srv.sha256_hex


@pytest.mark.slow
def test_simulator_when_fast_periodic_mid_transfer_retest_deferral(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Periodic --when-fast re-test during active download defers job if speed drops."""
    pdl_home = tmp_path / "pdl_home"
    pdl_home.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("PDL_HOME", str(pdl_home))
    monkeypatch.setattr(
        macos,
        "get_power_status",
        lambda: PowerStatus(on_ac_power=True, battery_percent=100, low_power_mode=False),
    )

    with ThrottleTestServer(
        profile=SimulatorProfile(file_size=512 * 1024, name="per_flow_cap", per_flow_bps=80 * 1024)
    ) as srv:
        out_dir = tmp_path / "periodic_retest_out"
        out_dir.mkdir(parents=True, exist_ok=True)
        threshold_bps = 500 * 1024

        job = QueueManager.add_job(
            srv.file_url,
            out_dir,
            filename="periodic.bin",
            connections=2,
            when_fast_bps=threshold_bps,
        )

        probe_calls = 0

        def dynamic_prober(_url: str) -> float:
            nonlocal probe_calls
            probe_calls += 1
            return 1024 * 1024.0 if probe_calls == 1 else 50 * 1024.0

        rc1, executed1 = QueueManager.run_queue(
            global_connections=2,
            caffeinate=False,
            net_watch=False,
            poll_interval=0.1,
            when_fast_retest_interval_sec=0.15,
            speed_prober=dynamic_prober,
        )
        assert rc1 == int(ExitCode.OK)
        assert len(executed1) == 1
        assert probe_calls >= 2
        j_mid = QueueManager.get_job(job.id)
        assert j_mid is not None and j_mid.status == "queued"

        # Now speed recovers and next queue run completes the job
        srv.set_profile(SimulatorProfile(file_size=512 * 1024, name="none"))
        rc2, executed2 = QueueManager.run_queue(
            global_connections=4,
            caffeinate=False,
            net_watch=False,
            when_fast_retest_interval_sec=5.0,
            speed_prober=lambda _u: 2 * 1024 * 1024.0,
        )
        assert rc2 == int(ExitCode.OK)
        assert len(executed2) == 1
        j_done = QueueManager.get_job(job.id)
        assert j_done is not None and j_done.status == "completed"
        assert _sha256_file(out_dir / "periodic.bin") == srv.sha256_hex
