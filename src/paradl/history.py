"""SQLite WAL history, per-host watchdog strategy store, learned profiles, and HTTP ETag cache."""

from __future__ import annotations

import json
import logging
import os
import shutil
import contextlib
import sqlite3
import time
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any

from paradl import platform
from paradl.paths import get_history_db_file, get_legacy_history_file, get_paradl_home

logger = logging.getLogger(__name__)

HISTORY_SCHEMA_VERSION = 4
STALE_PROFILE_DAYS = 30.0
STALE_PROFILE_SECONDS = STALE_PROFILE_DAYS * 86400.0


def compute_hour_bucket(hour_or_dt: int | datetime | None = None) -> int:
    """Compute a 4-hour time-of-day bucket (0..5) from a datetime or hour integer (0..23)."""
    if hour_or_dt is None:
        return datetime.now().hour // 4
    if isinstance(hour_or_dt, datetime):
        return hour_or_dt.hour // 4
    h = int(hour_or_dt)
    return (h % 24) // 4


def _parse_timestamp_epoch(ts_str: str) -> float:
    """Parse a timestamp string ('YYYY-MM-DD HH:MM:SS' or ISO-8601) into a Unix epoch float."""
    s = (ts_str or "").strip()
    if not s:
        return 0.0
    for fmt in ("%Y-%m-%d %H:%M:%S", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d"):
        try:
            return datetime.strptime(s[:19], fmt).timestamp()
        except ValueError:
            continue
    try:
        return datetime.fromisoformat(s).timestamp()
    except ValueError:
        return 0.0


@dataclass
class HostStrategy:
    host: str
    best_action: str = "tier1_native"
    connections: int = 16
    ip_family: str = "auto"
    speed_before: float = 0.0
    speed_after: float = 0.0
    supports_range: bool | None = None
    throttle_class: str | None = None
    updated_at: str = ""
    hour_bucket: int = -1
    network_id: str = ""
    updated_epoch: float = 0.0
    avg_rtt_ms: float = 0.0
    packet_loss_pct: float = 0.0


@dataclass
class LearnedProfile:
    """Learned per-(host, hour_bucket, network_id) outcome profile."""

    host: str
    hour_bucket: int = -1
    network_id: str = "default"
    connections: int = 16
    ip_family: str = "auto"
    recovery_action: str = "tier2_pause_unpause"
    speed_before: float = 0.0
    speed_after: float = 0.0
    supports_range: bool | None = None
    throttle_class: str | None = None
    updated_at: str = ""
    updated_epoch: float = 0.0
    source: str = "watchdog"
    avg_rtt_ms: float = 0.0
    packet_loss_pct: float = 0.0

    @property
    def best_action(self) -> str:
        return self.recovery_action

    @best_action.setter
    def best_action(self, value: str) -> None:
        self.recovery_action = value

    def effective_epoch(self) -> float:
        if self.updated_epoch > 0:
            return float(self.updated_epoch)
        return _parse_timestamp_epoch(self.updated_at)

    def age_seconds(self, *, now_epoch: float | None = None) -> float:
        now_ts = time.time() if now_epoch is None else float(now_epoch)
        eff = self.effective_epoch()
        if eff <= 0:
            return 0.0
        return max(0.0, now_ts - eff)

    def is_stale(
        self,
        *,
        now_epoch: float | None = None,
        max_age_days: float = STALE_PROFILE_DAYS,
    ) -> bool:
        """Return True if the profile is older than `max_age_days` (default 30 days)."""
        eff = self.effective_epoch()
        if eff <= 0:
            return False
        now_ts = time.time() if now_epoch is None else float(now_epoch)
        return (now_ts - eff) > (max_age_days * 86400.0)


class HistoryManager:
    """SQLite WAL-backed manager for download history, host strategies, learned profiles, and HTTP ETag cache."""

    @classmethod
    def _connect(cls) -> sqlite3.Connection:
        paradl_home = get_paradl_home()
        paradl_home.mkdir(parents=True, exist_ok=True)
        db_file = get_history_db_file()

        try:
            return cls._open_and_init(db_file)
        except sqlite3.DatabaseError as exc:
            bad_db = db_file.with_suffix(db_file.suffix + ".bad")
            logger.warning(
                "Corrupt SQLite history DB %s (%s); backing up to %s", db_file, exc, bad_db
            )
            try:
                if db_file.exists():
                    os.replace(db_file, bad_db)
                for suffix in ("-wal", "-shm"):
                    side = Path(str(db_file) + suffix)
                    if side.exists():
                        side.unlink()
            except OSError as os_err:
                logger.debug("Error rotating corrupt history DB: %s", os_err)
            return cls._open_and_init(db_file)

    @classmethod
    def _open_and_init(cls, db_file: Path) -> sqlite3.Connection:
        conn = sqlite3.connect(str(db_file), timeout=10.0)
        conn.row_factory = sqlite3.Row
        conn.execute("PRAGMA journal_mode=WAL;")
        conn.execute("PRAGMA synchronous=NORMAL;")
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS schema_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            """
        )
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS downloads (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp TEXT NOT NULL,
                target TEXT NOT NULL,
                type TEXT NOT NULL,
                filename TEXT NOT NULL,
                path TEXT NOT NULL,
                size INTEGER NOT NULL DEFAULT 0,
                duration REAL NOT NULL DEFAULT 0.0,
                success INTEGER NOT NULL DEFAULT 1,
                adb INTEGER NOT NULL DEFAULT 0,
                sha256 TEXT
            );
            """
        )
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS host_strategies (
                host TEXT PRIMARY KEY,
                best_action TEXT NOT NULL DEFAULT 'tier1_native',
                connections INTEGER NOT NULL DEFAULT 16,
                ip_family TEXT NOT NULL DEFAULT 'auto',
                speed_before REAL NOT NULL DEFAULT 0.0,
                speed_after REAL NOT NULL DEFAULT 0.0,
                supports_range INTEGER,
                throttle_class TEXT,
                updated_at TEXT NOT NULL,
                avg_rtt_ms REAL NOT NULL DEFAULT 0.0,
                packet_loss_pct REAL NOT NULL DEFAULT 0.0
            );
            """
        )
        # Migration: add RTT/loss columns to existing host_strategies tables
        try:
            cols = {
                r["name"] for r in conn.execute("PRAGMA table_info(host_strategies)").fetchall()
            }
            if "avg_rtt_ms" not in cols:
                conn.execute(
                    "ALTER TABLE host_strategies ADD COLUMN avg_rtt_ms REAL NOT NULL DEFAULT 0.0"
                )
            if "packet_loss_pct" not in cols:
                conn.execute(
                    "ALTER TABLE host_strategies ADD COLUMN packet_loss_pct REAL NOT NULL DEFAULT 0.0"
                )
        except sqlite3.Error:
            pass
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS learned_profiles (
                host TEXT NOT NULL,
                hour_bucket INTEGER NOT NULL,
                network_id TEXT NOT NULL,
                connections INTEGER NOT NULL DEFAULT 16,
                ip_family TEXT NOT NULL DEFAULT 'auto',
                recovery_action TEXT NOT NULL DEFAULT 'tier2_pause_unpause',
                speed_before REAL NOT NULL DEFAULT 0.0,
                speed_after REAL NOT NULL DEFAULT 0.0,
                supports_range INTEGER,
                throttle_class TEXT,
                updated_at TEXT NOT NULL,
                updated_epoch REAL NOT NULL DEFAULT 0.0,
                source TEXT NOT NULL DEFAULT 'watchdog',
                PRIMARY KEY (host, hour_bucket, network_id)
            );
            """
        )
        # Ensure `source` column exists on older learned_profiles tables
        try:
            cols = {
                r["name"] for r in conn.execute("PRAGMA table_info(learned_profiles)").fetchall()
            }
            if "source" not in cols:
                conn.execute(
                    "ALTER TABLE learned_profiles ADD COLUMN source TEXT NOT NULL DEFAULT 'watchdog'"
                )
            if "avg_rtt_ms" not in cols:
                conn.execute(
                    "ALTER TABLE learned_profiles ADD COLUMN avg_rtt_ms REAL NOT NULL DEFAULT 0.0"
                )
            if "packet_loss_pct" not in cols:
                conn.execute(
                    "ALTER TABLE learned_profiles ADD COLUMN packet_loss_pct REAL NOT NULL DEFAULT 0.0"
                )
        except sqlite3.Error:
            pass
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS http_cache (
                url TEXT PRIMARY KEY,
                etag TEXT NOT NULL,
                body_json TEXT NOT NULL,
                updated_at REAL NOT NULL
            );
            """
        )
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS queue_jobs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                target TEXT NOT NULL,
                target_type TEXT NOT NULL DEFAULT 'direct',
                out_dir TEXT NOT NULL,
                filename TEXT,
                mirrors_json TEXT NOT NULL DEFAULT '[]',
                priority INTEGER NOT NULL DEFAULT 0,
                connections INTEGER NOT NULL DEFAULT 16,
                status TEXT NOT NULL DEFAULT 'queued',
                require_ac INTEGER NOT NULL DEFAULT 1,
                pause_on_low_power INTEGER NOT NULL DEFAULT 0,
                when_fast_bps REAL NOT NULL DEFAULT 0.0,
                start_at TEXT,
                expected_length INTEGER,
                expected_digest TEXT,
                category TEXT NOT NULL DEFAULT '',
                bytes_downloaded INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                options_json TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            """
        )
        # Migration: add `category` column to existing queue_jobs tables
        try:
            cols = {r["name"] for r in conn.execute("PRAGMA table_info(queue_jobs)").fetchall()}
            if "category" not in cols:
                conn.execute("ALTER TABLE queue_jobs ADD COLUMN category TEXT NOT NULL DEFAULT ''")
        except sqlite3.Error:
            pass
        conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('schema_version', ?)",
            (str(HISTORY_SCHEMA_VERSION),),
        )
        conn.commit()
        cls._migrate_legacy_json_if_needed(conn)
        return conn

    @classmethod
    def _migrate_legacy_json_if_needed(cls, conn: sqlite3.Connection) -> None:
        """Read-only migration from legacy history.json into SQLite downloads table."""
        row = conn.execute("SELECT value FROM schema_meta WHERE key = 'json_migrated'").fetchone()
        if row and row["value"] == "1":
            return

        legacy_file = get_legacy_history_file()
        if not legacy_file.exists():
            conn.execute(
                "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('json_migrated', '1')"
            )
            conn.commit()
            return

        try:
            raw_text = legacy_file.read_text(encoding="utf-8")
            entries = json.loads(raw_text)
            if not isinstance(entries, list):
                raise ValueError("history.json root is not a list")
        except (OSError, json.JSONDecodeError, ValueError) as exc:
            bad_path = legacy_file.with_suffix(legacy_file.suffix + ".bad")
            logger.warning(
                "Corrupt legacy history file %s (%s); backing up to %s", legacy_file, exc, bad_path
            )
            try:
                shutil.copy2(legacy_file, bad_path)
            except OSError as copy_err:
                logger.debug("Could not copy corrupt history.json: %s", copy_err)
            conn.execute(
                "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('json_migrated', '1')"
            )
            conn.commit()
            return

        # Insert in reverse chronological order so oldest has lowest id and newest has highest id
        for item in reversed(entries):
            if not isinstance(item, dict):
                continue
            conn.execute(
                """
                INSERT INTO downloads (timestamp, target, type, filename, path, size, duration, success, adb, sha256)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    str(item.get("timestamp", datetime.now().strftime("%Y-%m-%d %H:%M:%S"))),
                    str(item.get("target", "-")),
                    str(item.get("type", "direct")),
                    str(item.get("filename", "-")),
                    str(item.get("path", "-")),
                    int(item.get("size") or 0),
                    float(item.get("duration") or 0.0),
                    1 if item.get("success", True) else 0,
                    1 if item.get("adb", False) else 0,
                    item.get("sha256"),
                ),
            )
        conn.execute(
            "INSERT OR REPLACE INTO schema_meta (key, value) VALUES ('json_migrated', '1')"
        )
        conn.commit()

    @classmethod
    def record(
        cls,
        target: str,
        target_type: str,
        file_path: str | Path | None,
        size: int | None,
        duration: float,
        success: bool,
        adb: bool = False,
        sha256: str | None = None,
        history_limit: int = 50,
    ) -> None:
        fp_str = str(file_path) if file_path else "-"
        fname = os.path.basename(fp_str) if file_path else "-"
        actual_size = size
        if actual_size is None and file_path and os.path.isfile(fp_str):
            try:
                actual_size = os.path.getsize(fp_str)
            except OSError:
                actual_size = 0

        try:
            with contextlib.closing(cls._connect()) as conn:
                conn.execute(
                    """
                    INSERT INTO downloads (timestamp, target, type, filename, path, size, duration, success, adb, sha256)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        datetime.now().strftime("%Y-%m-%d %H:%M:%S"),
                        target,
                        str(target_type),
                        fname,
                        fp_str,
                        int(actual_size or 0),
                        round(float(duration), 2),
                        1 if success else 0,
                        1 if adb else 0,
                        sha256,
                    ),
                )
                cap = max(int(history_limit), 1)
                conn.execute(
                    """
                    DELETE FROM downloads
                    WHERE id NOT IN (
                        SELECT id FROM downloads ORDER BY id DESC LIMIT ?
                    )
                    """,
                    (cap,),
                )
                conn.commit()
        except sqlite3.Error as exc:
            logger.debug("Failed to record download history: %s", exc)

    @classmethod
    def get_entries(
        cls,
        *,
        limit: int = 15,
        search: str | None = None,
        target_type: str | None = None,
        success: bool | None = None,
    ) -> list[dict[str, Any]]:
        """Return recent download history entries, optionally filtered.

        Filters:
          - search: substring match (case-insensitive) on filename or target.
          - target_type: exact match on the download type column.
          - success: filter to only successful (True) or failed (False) entries.
        """
        try:
            with contextlib.closing(cls._connect()) as conn:
                query = "SELECT * FROM downloads"
                params: list[Any] = []
                clauses: list[str] = []

                if search:
                    clauses.append("(LOWER(filename) LIKE LOWER(?) OR LOWER(target) LIKE LOWER(?))")
                    like = f"%{search}%"
                    params.extend([like, like])

                if target_type:
                    clauses.append("type = ?")
                    params.append(target_type)

                if success is not None:
                    clauses.append("success = ?")
                    params.append(1 if success else 0)

                if clauses:
                    query += " WHERE " + " AND ".join(clauses)

                query += " ORDER BY id DESC LIMIT ?"
                params.append(max(int(limit), 1))

                rows = conn.execute(query, params).fetchall()
                return [
                    {
                        "id": r["id"],
                        "timestamp": r["timestamp"],
                        "target": r["target"],
                        "type": r["type"],
                        "filename": r["filename"],
                        "path": r["path"],
                        "size": r["size"],
                        "duration": r["duration"],
                        "success": bool(r["success"]),
                        "adb": bool(r["adb"]),
                        "sha256": r["sha256"],
                    }
                    for r in rows
                ]
        except sqlite3.Error as exc:
            logger.debug("Failed to read history entries: %s", exc)
            return []

    @classmethod
    def get_entry_by_id(cls, entry_id: int) -> dict[str, Any] | None:
        """Return a single history entry by its database id, or None if not found."""
        try:
            with contextlib.closing(cls._connect()) as conn:
                row = conn.execute(
                    "SELECT * FROM downloads WHERE id = ?",
                    (int(entry_id),),
                ).fetchone()
                if row is None:
                    return None
                return {
                    "id": row["id"],
                    "timestamp": row["timestamp"],
                    "target": row["target"],
                    "type": row["type"],
                    "filename": row["filename"],
                    "path": row["path"],
                    "size": row["size"],
                    "duration": row["duration"],
                    "success": bool(row["success"]),
                    "adb": bool(row["adb"]),
                    "sha256": row["sha256"],
                }
        except sqlite3.Error as exc:
            logger.debug("Failed to read history entry %s: %s", entry_id, exc)
            return None

    @classmethod
    def clear(cls) -> bool:
        try:
            with contextlib.closing(cls._connect()) as conn:
                conn.execute("DELETE FROM downloads")
                conn.commit()
            return True
        except sqlite3.Error as exc:
            logger.debug("Failed to clear history: %s", exc)
            return False

    @classmethod
    def display(
        cls,
        *,
        limit: int = 15,
        search: str | None = None,
        target_type: str | None = None,
        success: bool | None = None,
    ) -> None:
        from paradl.models import format_size
        from paradl.ui import UI, print_plain

        entries = cls.get_entries(
            limit=limit, search=search, target_type=target_type, success=success
        )
        if UI.json_mode:
            UI.emit_event(
                "history",
                entries=entries,
                count=len(entries),
                search=search,
                target_type=target_type,
                success=success,
            )
            return
        if not entries:
            print_plain("  [dim]No download history recorded yet.[/dim]")
            return

        from rich import box
        from rich.table import Table

        title_parts = [f"Recent Downloads (Last {min(limit, len(entries))})"]
        if search:
            title_parts.append(f"search='{search}'")
        if target_type:
            title_parts.append(f"type={target_type}")
        if success is not None:
            title_parts.append(f"success={'yes' if success else 'no'}")

        table = Table(
            title=" | ".join(title_parts),
            box=box.HORIZONTALS,
            pad_edge=False,
        )
        table.add_column("ID", justify="right", style="cyan")
        table.add_column("Type", style="cyan")
        table.add_column("Filename", style="bold white")
        table.add_column("Size", justify="right", style="dim cyan")
        table.add_column("Duration", justify="right", style="dim")
        table.add_column("Status", justify="center")

        for item in entries:
            st = "[green]✓[/green]" if item.get("success") else "[red]✗[/red]"
            sz = format_size(item.get("size")) if item.get("size") else "-"
            dur = f"{item.get('duration', 0):.1f}s"
            table.add_row(
                str(item.get("id", "-")),
                str(item.get("type", "-")),
                str(item.get("filename", "-"))[:45],
                sz,
                dur,
                st,
            )
        UI.print()
        UI.print_renderable(table)
        UI.print()

    # ------------------------------------------------------------------
    # Host Watchdog Strategy & Learned Profiles (Phase 3 #5 + Phase 7 A3)
    # ------------------------------------------------------------------
    @staticmethod
    def _row_to_learned_profile(row: sqlite3.Row) -> LearnedProfile:
        row_keys = row.keys()
        return LearnedProfile(
            host=str(row["host"]),
            hour_bucket=int(row["hour_bucket"]),
            network_id=str(row["network_id"]),
            connections=int(row["connections"]),
            ip_family=str(row["ip_family"]),
            recovery_action=str(row["recovery_action"]),
            speed_before=float(row["speed_before"]),
            speed_after=float(row["speed_after"]),
            supports_range=None if row["supports_range"] is None else bool(row["supports_range"]),
            throttle_class=row["throttle_class"],
            updated_at=str(row["updated_at"]),
            updated_epoch=float(row["updated_epoch"]),
            source=str(row["source"]) if "source" in row_keys and row["source"] else "watchdog",
            avg_rtt_ms=float(row["avg_rtt_ms"]) if "avg_rtt_ms" in row_keys else 0.0,
            packet_loss_pct=float(row["packet_loss_pct"]) if "packet_loss_pct" in row_keys else 0.0,
        )

    @classmethod
    def save_learned_profile(cls, profile: LearnedProfile) -> None:
        """Persist a LearnedProfile keyed by (host, hour_bucket, network_id) and sync host_strategies."""
        if not profile.host:
            return
        host_low = profile.host.lower()
        hb = compute_hour_bucket() if profile.hour_bucket < 0 else int(profile.hour_bucket)
        net_id = (profile.network_id or platform.get_network_id() or "default").strip() or "default"
        conns = min(max(int(profile.connections), 1), 16)
        src_val = (profile.source or "watchdog").strip() or "watchdog"

        now_epoch = float(profile.updated_epoch) if profile.updated_epoch > 0 else 0.0
        now_str = (profile.updated_at or "").strip()
        if now_epoch > 0 and not now_str:
            now_str = datetime.fromtimestamp(now_epoch).strftime("%Y-%m-%d %H:%M:%S")
        elif now_str and now_epoch <= 0:
            parsed_ep = _parse_timestamp_epoch(now_str)
            now_epoch = parsed_ep if parsed_ep > 0 else time.time()
        elif not now_str and now_epoch <= 0:
            now_epoch = time.time()
            now_str = datetime.fromtimestamp(now_epoch).strftime("%Y-%m-%d %H:%M:%S")

        sup_range_db = (
            None if profile.supports_range is None else (1 if profile.supports_range else 0)
        )

        try:
            with contextlib.closing(cls._connect()) as conn:
                conn.execute(
                    """
                    INSERT OR REPLACE INTO learned_profiles
                    (host, hour_bucket, network_id, connections, ip_family, recovery_action,
                     speed_before, speed_after, supports_range, throttle_class, updated_at, updated_epoch, source,
                     avg_rtt_ms, packet_loss_pct)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        host_low,
                        hb,
                        net_id,
                        conns,
                        profile.ip_family,
                        profile.recovery_action,
                        float(profile.speed_before),
                        float(profile.speed_after),
                        sup_range_db,
                        profile.throttle_class,
                        now_str,
                        now_epoch,
                        src_val,
                        float(profile.avg_rtt_ms),
                        float(profile.packet_loss_pct),
                    ),
                )
                conn.execute(
                    """
                    INSERT OR REPLACE INTO host_strategies
                    (host, best_action, connections, ip_family, speed_before, speed_after,
                     supports_range, throttle_class, updated_at, avg_rtt_ms, packet_loss_pct)
                    VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        host_low,
                        profile.recovery_action,
                        conns,
                        profile.ip_family,
                        float(profile.speed_before),
                        float(profile.speed_after),
                        sup_range_db,
                        profile.throttle_class,
                        now_str,
                        float(profile.avg_rtt_ms),
                        float(profile.packet_loss_pct),
                    ),
                )
                conn.commit()
        except sqlite3.Error as exc:
            logger.debug("Failed to save learned profile for %s: %s", profile.host, exc)

    @classmethod
    def get_best_profile(
        cls,
        host: str,
        *,
        hour_bucket: int | None = None,
        network_id: str | None = None,
    ) -> LearnedProfile | None:
        """Select the best learned profile for `host` matching `(hour_bucket, network_id)`.

        Priority:
        1. Exact match on `(host, hour_bucket, network_id)`
        2. Match on `(host, network_id)` across hour buckets (highest `speed_after`)
        3. Match on `(host, hour_bucket)` across network IDs (highest `speed_after`)
        4. Best profile for `host` across any bucket/network, or legacy `host_strategies` row.
        """
        if not host:
            return None
        host_low = host.lower()
        target_hb = compute_hour_bucket() if hour_bucket is None else int(hour_bucket)
        target_net = (
            network_id if network_id is not None else (platform.get_network_id() or "default")
        ).strip() or "default"

        try:
            with contextlib.closing(cls._connect()) as conn:
                rows = conn.execute(
                    "SELECT * FROM learned_profiles WHERE host = ?",
                    (host_low,),
                ).fetchall()

                if rows:
                    profiles = [cls._row_to_learned_profile(r) for r in rows]

                    def _score(p: LearnedProfile) -> tuple[int, float, float]:
                        hb_match = p.hour_bucket == target_hb
                        net_match = p.network_id == target_net
                        if hb_match and net_match:
                            specificity = 3
                        elif net_match:
                            specificity = 2
                        elif hb_match:
                            specificity = 1
                        else:
                            specificity = 0
                        return (specificity, p.speed_after, p.effective_epoch())

                    profiles.sort(key=_score, reverse=True)
                    return profiles[0]

                # Fallback to legacy host_strategies table if present
                legacy = conn.execute(
                    "SELECT * FROM host_strategies WHERE host = ?",
                    (host_low,),
                ).fetchone()
                if legacy:
                    up_at = str(legacy["updated_at"])
                    return LearnedProfile(
                        host=str(legacy["host"]),
                        hour_bucket=target_hb,
                        network_id=target_net,
                        connections=int(legacy["connections"]),
                        ip_family=str(legacy["ip_family"]),
                        recovery_action=str(legacy["best_action"]),
                        speed_before=float(legacy["speed_before"]),
                        speed_after=float(legacy["speed_after"]),
                        supports_range=None
                        if legacy["supports_range"] is None
                        else bool(legacy["supports_range"]),
                        throttle_class=legacy["throttle_class"],
                        updated_at=up_at,
                        updated_epoch=_parse_timestamp_epoch(up_at),
                    )

                # Fallback to global 'throttle-probe' baseline profile if available
                probe_row = conn.execute(
                    "SELECT * FROM learned_profiles WHERE host = '*' AND source = 'throttle-probe' ORDER BY updated_epoch DESC LIMIT 1"
                ).fetchone()
                if probe_row:
                    prof = cls._row_to_learned_profile(probe_row)
                    prof.host = host_low
                    return prof
        except sqlite3.Error as exc:
            logger.debug("Failed to query best profile for %s: %s", host, exc)
        return None

    @classmethod
    def list_learned_profiles(cls, host: str | None = None) -> list[LearnedProfile]:
        """Return all stored learned profiles (optionally filtered by host), ordered by updated_epoch DESC."""
        try:
            with contextlib.closing(cls._connect()) as conn:
                if host:
                    rows = conn.execute(
                        "SELECT * FROM learned_profiles WHERE host = ? ORDER BY updated_epoch DESC, speed_after DESC",
                        (host.lower(),),
                    ).fetchall()
                else:
                    rows = conn.execute(
                        "SELECT * FROM learned_profiles ORDER BY updated_epoch DESC, host ASC, hour_bucket ASC"
                    ).fetchall()
                return [cls._row_to_learned_profile(r) for r in rows]
        except sqlite3.Error as exc:
            logger.debug("Failed to list learned profiles: %s", exc)
            return []

    @classmethod
    def reset_profiles(cls, host: str | None = None) -> int:
        """Delete learned profiles (and corresponding host_strategies) for `host` (or all hosts if None)."""
        try:
            with contextlib.closing(cls._connect()) as conn:
                if host:
                    h_low = host.strip().lower()
                    c1 = conn.execute("DELETE FROM learned_profiles WHERE host = ?", (h_low,))
                    c2 = conn.execute("DELETE FROM host_strategies WHERE host = ?", (h_low,))
                    conn.commit()
                    return int(c1.rowcount or 0) or int(c2.rowcount or 0)
                c1 = conn.execute("DELETE FROM learned_profiles")
                c2 = conn.execute("DELETE FROM host_strategies")
                conn.commit()
                return max(int(c1.rowcount or 0), int(c2.rowcount or 0))
        except sqlite3.Error as exc:
            logger.debug("Failed to reset profiles for %s: %s", host, exc)
            return 0

    @classmethod
    def display_profiles(cls, host: str | None = None) -> None:
        """Display learned profiles table or emit JSONL event for `pdl profiles`."""
        from paradl.models import format_speed
        from paradl.ui import UI, print_plain

        profiles = cls.list_learned_profiles(host=host)
        now_ts = time.time()
        serialized = [
            {
                "host": p.host,
                "hour_bucket": p.hour_bucket,
                "network_id": p.network_id,
                "connections": p.connections,
                "ip_family": p.ip_family,
                "recovery_action": p.recovery_action,
                "speed_before": round(p.speed_before, 2),
                "speed_after": round(p.speed_after, 2),
                "supports_range": p.supports_range,
                "throttle_class": p.throttle_class,
                "updated_at": p.updated_at,
                "source": p.source,
                "stale": p.is_stale(now_epoch=now_ts),
            }
            for p in profiles
        ]

        if UI.json_mode:
            UI.emit_event("profiles", profiles=serialized, count=len(serialized))
            return

        if not profiles:
            print_plain("  [dim]No learned host profiles recorded yet.[/dim]")
            return

        from rich import box
        from rich.table import Table

        table = Table(
            title=f"Learned Host Profiles ({len(profiles)})",
            box=box.HORIZONTALS,
            pad_edge=False,
        )
        table.add_column("Host", style="bold white")
        table.add_column("Bucket", justify="right", style="cyan")
        table.add_column("Network", style="cyan")
        table.add_column("Conns", justify="right", style="bold green")
        table.add_column("IP", justify="center", style="dim")
        table.add_column("Action", style="yellow")
        table.add_column("Before -> After", justify="right", style="white")
        table.add_column("Status", justify="center")

        for p in profiles:
            stale = p.is_stale(now_epoch=now_ts)
            status_str = "[yellow]stale (>30d)[/yellow]" if stale else "[green]fresh[/green]"
            spd_str = f"{format_speed(p.speed_before)} -> {format_speed(p.speed_after)}"
            table.add_row(
                p.host,
                f"H{p.hour_bucket}",
                p.network_id,
                f"{p.connections}x",
                p.ip_family,
                p.recovery_action,
                spd_str,
                status_str,
            )

        UI.print()
        UI.print_renderable(table)
        UI.print()

    @classmethod
    def get_host_strategy(cls, host: str) -> HostStrategy | None:
        if not host:
            return None
        best = cls.get_best_profile(host)
        if best is not None:
            return HostStrategy(
                host=best.host,
                best_action=best.recovery_action,
                connections=best.connections,
                ip_family=best.ip_family,
                speed_before=best.speed_before,
                speed_after=best.speed_after,
                supports_range=best.supports_range,
                throttle_class=best.throttle_class,
                updated_at=best.updated_at,
                hour_bucket=best.hour_bucket,
                network_id=best.network_id,
                updated_epoch=best.updated_epoch,
                avg_rtt_ms=best.avg_rtt_ms,
                packet_loss_pct=best.packet_loss_pct,
            )
        return None

    @classmethod
    def save_host_strategy(cls, strategy: HostStrategy) -> None:
        if not strategy.host:
            return
        hb = compute_hour_bucket() if strategy.hour_bucket < 0 else int(strategy.hour_bucket)
        net_id = (
            strategy.network_id or platform.get_network_id() or "default"
        ).strip() or "default"
        cls.save_learned_profile(
            LearnedProfile(
                host=strategy.host.lower(),
                hour_bucket=hb,
                network_id=net_id,
                connections=int(strategy.connections),
                ip_family=strategy.ip_family,
                recovery_action=strategy.best_action,
                speed_before=float(strategy.speed_before),
                speed_after=float(strategy.speed_after),
                supports_range=strategy.supports_range,
                throttle_class=strategy.throttle_class,
                updated_at=strategy.updated_at,
                updated_epoch=strategy.updated_epoch,
                avg_rtt_ms=float(strategy.avg_rtt_ms),
                packet_loss_pct=float(strategy.packet_loss_pct),
            )
        )

    @classmethod
    def list_host_strategies(cls) -> list[HostStrategy]:
        try:
            with contextlib.closing(cls._connect()) as conn:
                rows = conn.execute(
                    "SELECT * FROM host_strategies ORDER BY updated_at DESC"
                ).fetchall()
                return [
                    HostStrategy(
                        host=r["host"],
                        best_action=r["best_action"],
                        connections=int(r["connections"]),
                        ip_family=r["ip_family"],
                        speed_before=float(r["speed_before"]),
                        speed_after=float(r["speed_after"]),
                        supports_range=None
                        if r["supports_range"] is None
                        else bool(r["supports_range"]),
                        throttle_class=r["throttle_class"],
                        updated_at=r["updated_at"],
                        avg_rtt_ms=float(r["avg_rtt_ms"]) if "avg_rtt_ms" in r else 0.0,
                        packet_loss_pct=float(r["packet_loss_pct"])
                        if "packet_loss_pct" in r
                        else 0.0,
                    )
                    for r in rows
                ]
        except sqlite3.Error as exc:
            logger.debug("Failed to list host strategies: %s", exc)
            return []

    @classmethod
    def reset_hosts(cls) -> int:
        return cls.reset_profiles(host=None)

    # ------------------------------------------------------------------
    # HTTP ETag Metadata Cache (Phase 4)
    # ------------------------------------------------------------------
    @classmethod
    def get_http_cache(cls, url: str) -> tuple[str, Any] | None:
        try:
            with contextlib.closing(cls._connect()) as conn:
                row = conn.execute(
                    "SELECT etag, body_json FROM http_cache WHERE url = ?", (url,)
                ).fetchone()
                if not row:
                    return None
                return str(row["etag"]), json.loads(row["body_json"])
        except (sqlite3.Error, json.JSONDecodeError) as exc:
            logger.debug("HTTP cache miss/error for %s: %s", url, exc)
            return None

    @classmethod
    def set_http_cache(cls, url: str, etag: str, body_obj: Any) -> None:
        try:
            with contextlib.closing(cls._connect()) as conn:
                conn.execute(
                    "INSERT OR REPLACE INTO http_cache (url, etag, body_json, updated_at) VALUES (?, ?, ?, ?)",
                    (url, etag, json.dumps(body_obj), time.time()),
                )
                conn.commit()
        except (sqlite3.Error, TypeError) as exc:
            logger.debug("Failed to store HTTP cache for %s: %s", url, exc)
