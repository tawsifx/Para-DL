"""Wave-2 Task 1: SQLite schema-init guard and batched profile persistence parity tests."""

from __future__ import annotations

import contextlib
import sqlite3
from datetime import UTC, datetime
from pathlib import Path

import pytest

from paradl.core.clock import FakeClock
from paradl.core.paths import get_history_db_file
from paradl.storage.history import HISTORY_SCHEMA_VERSION, HistoryManager, LearnedProfile
from paradl.throttle.models import (
    LocalVsInternationalResult,
    MeasurementDataset,
    RecommendedProfile,
    SanityResult,
    ValidatedTarget,
)
from paradl.throttle.store import ThrottleStore

FIXED_NOW = 1_700_000_000.0

HISTORY_TABLES = {
    "schema_meta",
    "downloads",
    "host_strategies",
    "learned_profiles",
    "http_cache",
    "host_caps",
    "queue_jobs",
}
THROTTLE_TABLES = {"throttle_samples", "throttle_runs", "asn_cache"}


def _dump_schema(db_file: Path) -> dict[str, object]:
    """Capture the complete on-disk schema plus connection-level pragmas."""
    conn = sqlite3.connect(str(db_file))
    conn.row_factory = sqlite3.Row
    try:
        master = [
            (row["type"], row["name"], row["sql"])
            for row in conn.execute(
                "SELECT type, name, sql FROM sqlite_master ORDER BY type, name"
            ).fetchall()
        ]
        tables = [
            row["name"]
            for row in conn.execute(
                "SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name"
            ).fetchall()
        ]
        table_info = {
            table: [
                (
                    row["cid"],
                    row["name"],
                    row["type"],
                    row["notnull"],
                    row["dflt_value"],
                    row["pk"],
                )
                for row in conn.execute(f"PRAGMA table_info({table})").fetchall()
            ]
            for table in tables
        }
        pragmas = {
            "journal_mode": conn.execute("PRAGMA journal_mode").fetchone()[0],
            "synchronous": conn.execute("PRAGMA synchronous").fetchone()[0],
            "user_version": conn.execute("PRAGMA user_version").fetchone()[0],
        }
        return {"master": master, "table_info": table_info, "pragmas": pragmas}
    finally:
        conn.close()


def _table_names(db_file: Path) -> set[str]:
    conn = sqlite3.connect(str(db_file))
    try:
        return {
            row[0] for row in conn.execute("SELECT name FROM sqlite_master WHERE type = 'table'")
        }
    finally:
        conn.close()


def _dump_profile_rows(db_file: Path) -> tuple[list[tuple], list[tuple]]:
    conn = sqlite3.connect(str(db_file))
    try:
        learned = conn.execute(
            "SELECT * FROM learned_profiles ORDER BY host, hour_bucket, network_id"
        ).fetchall()
        strategies = conn.execute("SELECT * FROM host_strategies ORDER BY host").fetchall()
        return [tuple(row) for row in learned], [tuple(row) for row in strategies]
    finally:
        conn.close()


def _fixture_inputs() -> tuple[RecommendedProfile, list[ValidatedTarget], MeasurementDataset]:
    profile = RecommendedProfile(
        domestic_connections=12,
        international_connections=6,
        default_connections=16,
        ip_family="ipv4",
        recovery_action="single_stream",
    )
    targets = [
        ValidatedTarget(
            id="intl-1",
            url="https://a.example/x",
            host="A.Example",
            ip="1.1.1.1",
            group="global-cdn",
            locality="international",
            supports_range=True,
        ),
        ValidatedTarget(
            id="intl-2",
            url="https://b.example/x",
            host="b.example",
            ip="2.2.2.2",
            group="global-cdn",
            locality="international",
            supports_range=False,
        ),
        ValidatedTarget(
            id="dom-1",
            url="https://c.example/x",
            host="C.Example",
            ip="3.3.3.3",
            group="domestic",
            locality="domestic",
            supports_range=True,
        ),
        ValidatedTarget(
            id="loop",
            url="http://127.0.0.1/x",
            host="127.0.0.1",
            ip="127.0.0.1",
            group="own-server",
            locality="loopback",
        ),
    ]
    dataset = MeasurementDataset(
        m0_sanity=SanityResult(ssid="test-ssid", active_interface="en0"),
        m10_local_vs_intl=LocalVsInternationalResult(
            international_median_bps=500_000.0,
            international_16_bps=2_000_000.0,
            domestic_16_bps=5_000_000.0,
        ),
    )
    return profile, targets, dataset


def _expected_profiles(
    profile: RecommendedProfile, targets: list[ValidatedTarget], dataset: MeasurementDataset
) -> list[LearnedProfile]:
    """Reproduce the exact LearnedProfile objects persist_recommended_profile builds."""
    dt = datetime.fromtimestamp(FIXED_NOW, tz=UTC).astimezone()
    hb = dt.hour // 4
    net_id = dataset.m0_sanity.ssid or dataset.m0_sanity.active_interface or "default"
    intl_1 = dataset.m10_local_vs_intl.international_median_bps or 500_000.0
    intl_16 = dataset.m10_local_vs_intl.international_16_bps or max(intl_1, 2_000_000.0)
    dom_16 = dataset.m10_local_vs_intl.domestic_16_bps or max(intl_16, 5_000_000.0)

    expected = [
        LearnedProfile(
            host="*",
            hour_bucket=hb,
            network_id=net_id,
            connections=min(max(int(profile.default_connections), 1), 16),
            ip_family=profile.ip_family,
            recovery_action=profile.recovery_action,
            speed_before=float(intl_1),
            speed_after=float(intl_16),
            supports_range=True,
            throttle_class=None,
            updated_epoch=FIXED_NOW,
            source="throttle-probe",
        )
    ]
    for t in targets:
        if not t.host or t.locality == "loopback":
            continue
        conns = (
            profile.domestic_connections
            if t.locality == "domestic"
            else profile.international_connections
        )
        if not t.supports_range:
            conns = 1
        spd_after = dom_16 if t.locality == "domestic" else intl_16
        expected.append(
            LearnedProfile(
                host=t.host.lower(),
                hour_bucket=hb,
                network_id=net_id,
                connections=min(max(int(conns), 1), 16),
                ip_family=profile.ip_family,
                recovery_action="single_stream"
                if not t.supports_range
                else profile.recovery_action,
                speed_before=float(intl_1),
                speed_after=float(spd_after),
                supports_range=t.supports_range,
                throttle_class=None,
                updated_epoch=FIXED_NOW,
                source="throttle-probe",
            )
        )
    return expected


def test_schema_unchanged_across_batch_of_connections():
    """Schema, pragmas and schema_meta must not change after a batch of fresh connections."""
    db_file = get_history_db_file()
    with contextlib.closing(HistoryManager._connect()):
        pass
    with ThrottleStore._conn():
        pass
    before = _dump_schema(db_file)

    for index in range(5):
        HistoryManager.record(f"https://example.com/{index}", "direct", None, None, 1.0, True)
        ThrottleStore.set_cached_asn(f"9.9.9.{index}", "AS9", "US", "Example")

    after = _dump_schema(db_file)
    assert after == before
    assert after["pragmas"]["journal_mode"] == "wal"

    # WAL/synchronous pragmas are per-connection: a managed connection still applies them.
    with contextlib.closing(HistoryManager._connect()) as managed:
        assert managed.execute("PRAGMA journal_mode").fetchone()[0] == "wal"
        assert managed.execute("PRAGMA synchronous").fetchone()[0] == 1

    conn = sqlite3.connect(str(db_file))
    try:
        version = conn.execute(
            "SELECT value FROM schema_meta WHERE key = 'schema_version'"
        ).fetchone()
    finally:
        conn.close()
    assert version is not None
    assert version[0] == str(HISTORY_SCHEMA_VERSION)


def test_ddl_executed_at_most_once_for_repeated_connections(monkeypatch: pytest.MonkeyPatch):
    """Repeated connections to one DB file must not re-run the CREATE TABLE DDL."""
    statements: list[str] = []
    real_connect = sqlite3.connect

    def traced_connect(*args, **kwargs):  # type: ignore[no-untyped-def]
        conn = real_connect(*args, **kwargs)
        conn.set_trace_callback(statements.append)
        return conn

    with monkeypatch.context() as patcher:
        patcher.setattr("paradl.storage.history.sqlite3.connect", traced_connect)
        for _ in range(5):
            with contextlib.closing(HistoryManager._connect()):
                pass
            with ThrottleStore._conn():
                pass

    create_statements = [
        statement for statement in statements if statement.strip().upper().startswith("CREATE")
    ]
    # 7 history tables + 3 throttle tables, initialized exactly once per process/file.
    assert len(create_statements) <= 10, create_statements


def test_reopen_after_delete_recreates_schema():
    """Deleting/recreating the DB file invalidates the once-per-process guard."""
    db_file = get_history_db_file()
    with contextlib.closing(HistoryManager._connect()):
        pass
    with ThrottleStore._conn():
        pass
    before = _dump_schema(db_file)

    db_file.unlink()
    for suffix in ("-wal", "-shm"):
        side = Path(str(db_file) + suffix)
        if side.exists():
            side.unlink()
    assert not db_file.exists()

    with contextlib.closing(HistoryManager._connect()) as conn:
        names = {
            row[0] for row in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")
        }
    assert names >= HISTORY_TABLES

    with ThrottleStore._conn() as conn:
        names = {
            row[0] for row in conn.execute("SELECT name FROM sqlite_master WHERE type='table'")
        }
    assert names >= THROTTLE_TABLES

    assert _dump_schema(db_file) == before


def test_persist_recommended_profile_uses_single_connection(monkeypatch: pytest.MonkeyPatch):
    """Batched persistence must open exactly one connection, not one per profile."""
    calls: list[int] = []
    real_connect = HistoryManager._connect

    @classmethod
    def counting_connect(cls):  # type: ignore[no-untyped-def]
        calls.append(1)
        return real_connect.__func__(cls)

    monkeypatch.setattr(HistoryManager, "_connect", counting_connect)
    monkeypatch.setattr("paradl.throttle.store.get_clock", lambda: FakeClock(FIXED_NOW))

    profile, targets, dataset = _fixture_inputs()
    ThrottleStore.persist_recommended_profile(profile, targets, dataset)
    assert len(calls) == 1, calls


def test_batched_persistence_matches_one_by_one(monkeypatch: pytest.MonkeyPatch, tmp_path: Path):
    """Rows written by the batched path must be byte-for-byte identical to per-profile saves."""
    monkeypatch.setattr("paradl.throttle.store.get_clock", lambda: FakeClock(FIXED_NOW))
    profile, targets, dataset = _fixture_inputs()
    expected = _expected_profiles(profile, targets, dataset)

    # Baseline: save each profile independently (legacy behaviour).
    monkeypatch.setenv("PARADL_HOME", str(tmp_path / "one_by_one"))
    db_one = get_history_db_file()
    with contextlib.closing(HistoryManager._connect()):
        pass
    for learned in expected:
        HistoryManager.save_learned_profile(learned)
    baseline = _dump_profile_rows(db_one)

    # Batched path: one call writes the same rows.
    monkeypatch.setenv("PARADL_HOME", str(tmp_path / "batched"))
    db_batch = get_history_db_file()
    with contextlib.closing(HistoryManager._connect()):
        pass
    ThrottleStore.persist_recommended_profile(profile, targets, dataset)
    batched = _dump_profile_rows(db_batch)

    assert batched == baseline
    assert len(batched[0]) == len(expected)
