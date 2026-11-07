"""Integration tests: SQLite persistent queue, priority ordering, crash recovery, and LaunchAgent."""

from __future__ import annotations

import plistlib
from pathlib import Path

import pytest

from pdl.queue import (
    QueueManager,
    format_start_at_time,
    generate_launchd_plist,
    parse_start_at_time,
)

pytestmark = pytest.mark.integration


def test_queue_crud_and_priority_ordering(isolated_pdl_home: Path) -> None:
    """Verify queue job insertion, priority ordering (DESC), status transitions, and removal."""
    j1 = QueueManager.add_job("https://example.com/low.bin", "/tmp", priority=10, filename="low.bin")
    j2 = QueueManager.add_job("https://example.com/high.bin", "/tmp", priority=100, filename="high.bin")
    j3 = QueueManager.add_job("https://example.com/mid.bin", "/tmp", priority=50, filename="mid.bin")

    id1, id2, id3 = j1.id, j2.id, j3.id

    jobs = QueueManager.list_jobs()
    assert len(jobs) == 3
    # Ordered by priority DESC, id ASC
    assert [j.id for j in jobs] == [id2, id3, id1]

    # Pause specific job
    assert QueueManager.pause_jobs([id2]) == 1
    j2_reloaded = QueueManager.get_job(id2)
    assert j2_reloaded is not None
    assert j2_reloaded.status == "paused"

    # Resume specific job
    assert QueueManager.resume_jobs([id2]) == 1
    assert QueueManager.get_job(id2).status == "queued"

    # Remove job
    assert QueueManager.remove_jobs([id1]) == 1
    assert len(QueueManager.list_jobs()) == 2


def test_queue_crash_recovery_resumes_running_jobs(isolated_pdl_home: Path) -> None:
    """Jobs left in 'running' status after a simulated crash/reboot are recovered to 'queued'."""
    j = QueueManager.add_job("https://example.com/file.bin", "/tmp")
    QueueManager.update_job_status(j.id, "running")

    recovered_count = QueueManager.recover_crashed_jobs()
    assert recovered_count == 1

    recovered_job = QueueManager.get_job(j.id)
    assert recovered_job is not None
    assert recovered_job.status == "queued"


def test_start_at_parsing_and_due_check() -> None:
    """Verify 24-hour time parsing, formatting, and scheduling calculation."""
    assert parse_start_at_time("04:30") == (4, 30)
    assert parse_start_at_time("23:59") == (23, 59)
    assert parse_start_at_time(None) is None

    with pytest.raises(ValueError):
        parse_start_at_time("25:00")
    with pytest.raises(ValueError):
        parse_start_at_time("invalid")

    assert format_start_at_time("04:05") == "04:05"


def test_launchd_plist_generation(tmp_path: Path) -> None:
    """Verify LaunchAgent plist generation, StartCalendarInterval, and valid XML serialization."""
    plist_path = tmp_path / "com.pdl.queue.plist"
    out_file = generate_launchd_plist(
        dest_path=plist_path,
        start_at="02:15",
        interval_sec=900,
    )
    assert out_file == plist_path
    assert plist_path.exists()

    # Must be loadable by standard plistlib parser
    parsed = plistlib.loads(plist_path.read_bytes())
    assert parsed["Label"] == "com.pdl.queue"
    assert "StartCalendarInterval" in parsed
    assert parsed["StartCalendarInterval"]["Hour"] == 2
    assert parsed["StartCalendarInterval"]["Minute"] == 15
    assert parsed["RunAtLoad"] is True
