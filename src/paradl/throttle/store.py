"""SQLite persistence for throttle samples, diagnostic runs, ASN cache, coverage matrix, and learned profiles."""

from __future__ import annotations

import json
import logging
import sqlite3
import statistics
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

from paradl.clock import get_clock
from paradl.history import HistoryManager, LearnedProfile, compute_hour_bucket
from paradl.paths import get_throttle_dir
from paradl.platform import macos
from paradl.throttle.models import (
    CoverageCell,
    MeasurementDataset,
    RecommendedProfile,
    SampleRecord,
    ThrottleReport,
    ValidatedTarget,
)

logger = logging.getLogger(__name__)

SAMPLE_IDEMPOTENCY_WINDOW_SEC = 45 * 60.0  # 45 minutes


class ThrottleStore:
    """Manages SQLite tables (`throttle_samples`, `throttle_runs`, `asn_cache`) inside `$PARADL_HOME/history.db`."""

    @classmethod
    def _conn(cls) -> sqlite3.Connection:
        conn = HistoryManager._connect()
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS throttle_samples (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                epoch REAL NOT NULL,
                hour INTEGER NOT NULL,
                is_weekend INTEGER NOT NULL DEFAULT 0,
                interface TEXT NOT NULL DEFAULT 'en0',
                ssid TEXT NOT NULL DEFAULT '',
                network_id TEXT NOT NULL DEFAULT 'default',
                source TEXT NOT NULL DEFAULT 'active',
                target_group TEXT NOT NULL DEFAULT 'international',
                host TEXT NOT NULL DEFAULT '',
                download_bps REAL NOT NULL DEFAULT 0.0,
                upload_bps REAL NOT NULL DEFAULT 0.0,
                rtt_ms REAL NOT NULL DEFAULT 0.0,
                bytes_used INTEGER NOT NULL DEFAULT 0,
                series_json TEXT NOT NULL DEFAULT '[]'
            );
            """
        )
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS throttle_runs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL UNIQUE,
                mode TEXT NOT NULL,
                isp TEXT NOT NULL,
                created_at TEXT NOT NULL,
                created_epoch REAL NOT NULL,
                budget_bytes INTEGER NOT NULL DEFAULT 0,
                bytes_used INTEGER NOT NULL DEFAULT 0,
                report_json TEXT NOT NULL
            );
            """
        )
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS asn_cache (
                ip TEXT PRIMARY KEY,
                asn TEXT NOT NULL,
                country TEXT NOT NULL,
                org TEXT NOT NULL,
                updated_epoch REAL NOT NULL
            );
            """
        )
        conn.commit()
        return conn

    # ------------------------------------------------------------------
    # ASN & Country Cache
    # ------------------------------------------------------------------
    @classmethod
    def get_cached_asn(
        cls, ip: str, *, max_age_sec: float = 7 * 86400.0
    ) -> tuple[str, str, str] | None:
        if not ip:
            return None
        now = get_clock().now()
        try:
            with cls._conn() as conn:
                row = conn.execute("SELECT * FROM asn_cache WHERE ip = ?", (ip,)).fetchone()
                if row and (now - float(row["updated_epoch"])) <= max_age_sec:
                    return str(row["asn"]), str(row["country"]), str(row["org"])
        except sqlite3.Error as exc:
            logger.debug("ASN cache lookup failed for %s: %s", ip, exc)
        return None

    @classmethod
    def set_cached_asn(cls, ip: str, asn: str, country: str, org: str) -> None:
        if not ip:
            return
        now = get_clock().now()
        try:
            with cls._conn() as conn:
                conn.execute(
                    """
                    INSERT OR REPLACE INTO asn_cache (ip, asn, country, org, updated_epoch)
                    VALUES (?, ?, ?, ?, ?)
                    """,
                    (ip, asn, country, org, now),
                )
                conn.commit()
        except sqlite3.Error as exc:
            logger.debug("ASN cache write failed for %s: %s", ip, exc)

    # ------------------------------------------------------------------
    # Active & Passive Time-of-Day Sampling (M8)
    # ------------------------------------------------------------------
    @classmethod
    def can_record_active_sample(
        cls,
        *,
        now_epoch: float | None = None,
        window_sec: float = SAMPLE_IDEMPOTENCY_WINDOW_SEC,
    ) -> tuple[bool, float]:
        """Return (can_sample, seconds_since_last_active_sample).

        Enforces the 45-minute idempotency window using `Clock.now()`.
        """
        now = get_clock().now() if now_epoch is None else float(now_epoch)
        try:
            with cls._conn() as conn:
                row = conn.execute(
                    "SELECT epoch FROM throttle_samples WHERE source = 'active' ORDER BY epoch DESC LIMIT 1"
                ).fetchone()
                if row is None:
                    return True, float("inf")
                last_epoch = float(row["epoch"])
                elapsed = now - last_epoch
                if 0 <= elapsed < window_sec:
                    return False, elapsed
                return True, max(0.0, elapsed)
        except sqlite3.Error as exc:
            logger.debug("Failed to check sample idempotency: %s", exc)
            return True, float("inf")

    @classmethod
    def record_sample(cls, sample: SampleRecord) -> int:
        """Persist an active or passive SampleRecord into SQLite."""
        try:
            with cls._conn() as conn:
                cur = conn.execute(
                    """
                    INSERT INTO throttle_samples (
                        timestamp, epoch, hour, is_weekend, interface, ssid, network_id,
                        source, target_group, host, download_bps, upload_bps, rtt_ms,
                        bytes_used, series_json
                    )
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        sample.timestamp,
                        float(sample.epoch),
                        int(sample.hour) % 24,
                        1 if sample.is_weekend else 0,
                        sample.interface,
                        sample.ssid,
                        sample.network_id,
                        sample.source,
                        sample.target_group,
                        sample.host,
                        float(sample.download_bps),
                        float(sample.upload_bps),
                        float(sample.rtt_ms),
                        int(sample.bytes_used),
                        json.dumps([round(x, 2) for x in sample.series_bps]),
                    ),
                )
                conn.commit()
                return int(cur.lastrowid or 0)
        except sqlite3.Error as exc:
            logger.debug("Failed to store throttle sample: %s", exc)
            return 0

    @classmethod
    def record_passive_download(
        cls,
        *,
        host: str,
        speed_bps: float,
        bytes_downloaded: int,
        series_bps: list[float] | None = None,
        target_group: str = "international",
    ) -> None:
        """Record a passive throughput sample from a normal `pdl` transfer at zero extra data cost."""
        if speed_bps <= 0 or bytes_downloaded < 32768:
            return
        now = get_clock().now()
        dt = datetime.fromtimestamp(now, tz=UTC).astimezone()
        net_id = macos.get_network_id() or "default"
        iface = net_id if net_id.startswith(("en", "utun", "bridge")) else "en0"
        ssid = net_id if not net_id.startswith(("en", "utun", "bridge", "default")) else ""
        sample = SampleRecord(
            timestamp=dt.strftime("%Y-%m-%dT%H:%M:%S%z"),
            epoch=now,
            hour=dt.hour,
            is_weekend=(dt.weekday() >= 5),
            interface=iface,
            ssid=ssid,
            network_id=net_id,
            source="passive",
            target_group=target_group,
            host=host.lower(),
            download_bps=float(speed_bps),
            upload_bps=0.0,
            rtt_ms=0.0,
            bytes_used=0,  # Zero extra diagnostic data cost
            series_bps=list(series_bps or [speed_bps]),
        )
        cls.record_sample(sample)

    @classmethod
    def get_samples(cls, limit: int = 1000) -> list[SampleRecord]:
        """Return stored samples ordered chronologically."""
        try:
            with cls._conn() as conn:
                rows = conn.execute(
                    "SELECT * FROM throttle_samples ORDER BY epoch DESC LIMIT ?",
                    (max(1, int(limit)),),
                ).fetchall()
                result: list[SampleRecord] = []
                for r in reversed(rows):
                    try:
                        series = [float(x) for x in json.loads(r["series_json"] or "[]")]
                    except (ValueError, TypeError):
                        series = []
                    result.append(
                        SampleRecord(
                            timestamp=str(r["timestamp"]),
                            epoch=float(r["epoch"]),
                            hour=int(r["hour"]) % 24,
                            is_weekend=bool(r["is_weekend"]),
                            interface=str(r["interface"]),
                            ssid=str(r["ssid"]),
                            network_id=str(r["network_id"]),
                            source=str(r["source"]),
                            target_group=str(r["target_group"]),
                            host=str(r["host"]),
                            download_bps=float(r["download_bps"]),
                            upload_bps=float(r["upload_bps"]),
                            rtt_ms=float(r["rtt_ms"]),
                            bytes_used=int(r["bytes_used"]),
                            series_bps=series,
                        )
                    )
                return result
        except sqlite3.Error as exc:
            logger.debug("Failed to load throttle samples: %s", exc)
            return []

    # ------------------------------------------------------------------
    # Coverage Matrix & Next-Best-Hour Recommender
    # ------------------------------------------------------------------
    @staticmethod
    def build_coverage_matrix(
        samples: list[SampleRecord],
    ) -> tuple[list[CoverageCell], list[str]]:
        """Build the 24-hour x (weekday, weekend) matrix and recommend high-value missing hours."""
        cells: dict[tuple[int, bool], list[SampleRecord]] = {
            (h, wk): [] for wk in (False, True) for h in range(24)
        }
        for s in samples:
            if s.download_bps > 0:
                cells[(int(s.hour) % 24, bool(s.is_weekend))].append(s)

        matrix: list[CoverageCell] = []
        for wk in (False, True):
            for h in range(24):
                bucket_samples = cells[(h, wk)]
                act = sum(1 for x in bucket_samples if x.source == "active")
                pas = sum(1 for x in bucket_samples if x.source != "active")
                eff_w = round(act * 1.0 + pas * 0.35, 2)
                if bucket_samples:
                    speeds = [x.download_bps for x in bucket_samples]
                    med = float(statistics.median(speeds))
                    mean = float(statistics.fmean(speeds))
                else:
                    med = None
                    mean = None
                matrix.append(
                    CoverageCell(
                        hour=h,
                        is_weekend=wk,
                        active_count=act,
                        passive_count=pas,
                        effective_weight=eff_w,
                        median_bps=med,
                        mean_bps=mean,
                    )
                )

        # Compute next-best-hour recommendations for sparse coverage
        # Priority windows:
        # 1. Weekday evening peak (19:00-23:00, target 21:00)
        # 2. Weekday morning/off-peak baseline (01:00-10:00, target 09:00 or 03:00)
        # 3. Weekday afternoon (13:00-17:00, target 15:00)
        # 4. Weekend evening peak (19:00-23:00, target 21:00)
        cell_lookup = {(c.hour, c.is_weekend): c for c in matrix}

        def _window_weight(hours: range, is_wk: bool) -> float:
            return sum(cell_lookup[(h, is_wk)].effective_weight for h in hours)

        suggestions: list[str] = []
        wd_peak_w = _window_weight(range(19, 24), False)
        wd_offpeak_w = _window_weight(range(1, 11), False)
        wd_afternoon_w = _window_weight(range(11, 19), False)
        wk_peak_w = _window_weight(range(19, 24), True)

        if wd_peak_w < 3.0:
            suggestions.append(
                f"run one sample around 21:00 on a weekday (evening peak window has weight {wd_peak_w:.1f}/3.0)"
            )
        if wd_offpeak_w < 3.0:
            suggestions.append(
                f"run one sample around 09:00 on a weekday (morning/off-peak baseline has weight {wd_offpeak_w:.1f}/3.0)"
            )
        if wd_afternoon_w < 2.0:
            suggestions.append(
                "run one sample around 15:00 on a weekday (daytime comparison window)"
            )
        if wk_peak_w < 2.0:
            suggestions.append("run one sample around 21:00 on a weekend (weekend peak comparison)")

        if not suggestions:
            # Find any specific 4-hour block with zero data
            for h_rep in (21, 9, 15, 3):
                if not cell_lookup[(h_rep, False)].has_data:
                    suggestions.append(f"run one sample around {h_rep:02d}:00 on a weekday")
                    break

        return matrix, suggestions

    # ------------------------------------------------------------------
    # Run Storage & Learned Profile Sync
    # ------------------------------------------------------------------
    @classmethod
    def save_run(cls, report: ThrottleReport) -> Path:
        """Persist a full diagnostic run to SQLite and `$PARADL_HOME/throttle/latest_report.json`."""
        now = get_clock().now()
        report_dict = report.to_dict()
        report_json_str = json.dumps(report_dict, indent=2)

        try:
            with cls._conn() as conn:
                conn.execute(
                    """
                    INSERT OR REPLACE INTO throttle_runs (
                        run_id, mode, isp, created_at, created_epoch,
                        budget_bytes, bytes_used, report_json
                    )
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        report.run_id,
                        report.mode,
                        report.isp,
                        report.timestamp,
                        now,
                        report.budget.limit_bytes,
                        report.budget.actual_used_bytes,
                        report_json_str,
                    ),
                )
                conn.commit()
        except sqlite3.Error as exc:
            logger.debug("Failed to store throttle run in SQLite: %s", exc)

        t_dir = get_throttle_dir()
        t_dir.mkdir(parents=True, exist_ok=True)
        latest_path = t_dir / "latest_report.json"
        latest_path.write_text(report_json_str + "\n", encoding="utf-8")
        return latest_path

    @classmethod
    def get_latest_run_dict(cls) -> dict[str, Any] | None:
        """Load the most recent stored run dict from SQLite or `$PARADL_HOME/throttle/latest_report.json`."""
        try:
            with cls._conn() as conn:
                row = conn.execute(
                    "SELECT report_json FROM throttle_runs ORDER BY created_epoch DESC, id DESC LIMIT 1"
                ).fetchone()
                if row and row["report_json"]:
                    data = json.loads(str(row["report_json"]))
                    if isinstance(data, dict):
                        return data
        except (sqlite3.Error, json.JSONDecodeError) as exc:
            logger.debug("Failed to load latest run from SQLite: %s", exc)

        latest_path = get_throttle_dir() / "latest_report.json"
        if latest_path.is_file():
            try:
                data = json.loads(latest_path.read_text(encoding="utf-8"))
                if isinstance(data, dict):
                    return data
            except (OSError, json.JSONDecodeError):
                pass
        return None

    @classmethod
    def persist_recommended_profile(
        cls,
        profile: RecommendedProfile,
        targets: list[ValidatedTarget],
        dataset: MeasurementDataset,
    ) -> None:
        """Write recommended download profile into `learned_profiles` under `source='throttle-probe'`
        so WatchdogController uses it as a starting point.
        """
        now_epoch = get_clock().now()
        dt = datetime.fromtimestamp(now_epoch, tz=UTC).astimezone()
        hb = compute_hour_bucket(dt.hour)
        net_id = (
            dataset.m0_sanity.ssid
            or dataset.m0_sanity.active_interface
            or macos.get_network_id()
            or "default"
        )

        # Determine representative before/after speeds
        intl_1 = dataset.m10_local_vs_intl.international_median_bps or 500_000.0
        intl_16 = dataset.m10_local_vs_intl.international_16_bps or max(intl_1, 2_000_000.0)
        dom_16 = dataset.m10_local_vs_intl.domestic_16_bps or max(intl_16, 5_000_000.0)

        primary_verdict = None
        # Save global wildcard '*' starting profile for WatchdogController
        HistoryManager.save_learned_profile(
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
                throttle_class=primary_verdict,
                updated_epoch=now_epoch,
                source="throttle-probe",
            )
        )

        # Save per-host profiles for each validated target
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
            HistoryManager.save_learned_profile(
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
                    throttle_class=primary_verdict,
                    updated_epoch=now_epoch,
                    source="throttle-probe",
                )
            )
