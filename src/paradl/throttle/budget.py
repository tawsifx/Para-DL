"""Hard total data budget tracker, connection capper, seeded jitter pauses, and 429/5xx exponential backoff."""

from __future__ import annotations

import logging
import random
import re
import threading
from dataclasses import dataclass, field

from paradl.clock import get_clock
from paradl.throttle.models import BudgetSummary

logger = logging.getLogger(__name__)

DEFAULT_QUICK_BUDGET_BYTES = 300 * 1024 * 1024  # 300 MiB
DEFAULT_FULL_BUDGET_BYTES = 3 * 1024 * 1024 * 1024  # 3 GiB
DEFAULT_SAMPLE_BUDGET_BYTES = 25 * 1024 * 1024  # 25 MiB

_SIZE_RE = re.compile(r"^\s*([0-9]+(?:\.[0-9]+)?)\s*([a-zA-Z]*)\s*$")


def parse_byte_budget(value: str | int | float | None, default_bytes: int) -> int:
    """Parse a byte budget string (e.g. '300MB', '3GB', '50M', '1048576') into integer bytes."""
    if value is None:
        return int(default_bytes)
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        if value <= 0:
            raise ValueError(f"Data budget must be positive, got {value}")
        return int(value)
    s = str(value).strip()
    if not s:
        return int(default_bytes)
    m = _SIZE_RE.match(s)
    if not m:
        raise ValueError(
            f"Unparseable --data-budget {value!r} (expected e.g. '300MB', '3GB', '50M')"
        )
    num = float(m.group(1))
    unit = m.group(2).strip().upper()
    if num <= 0:
        raise ValueError(f"Data budget must be > 0, got {value!r}")
    multipliers = {
        "": 1,
        "B": 1,
        "K": 1024,
        "KB": 1024,
        "KIB": 1024,
        "M": 1024**2,
        "MB": 1024**2,
        "MIB": 1024**2,
        "G": 1024**3,
        "GB": 1024**3,
        "GIB": 1024**3,
    }
    if unit not in multipliers:
        raise ValueError(f"Unknown unit {unit!r} in --data-budget {value!r}")
    return max(1024, int(num * multipliers[unit]))


@dataclass
class DataBudgetTracker:
    """Thread-safe enforcer guaranteeing total bytes transferred never exceed `limit_bytes`."""

    limit_bytes: int = DEFAULT_QUICK_BUDGET_BYTES
    planned_worst_case_bytes: int = 0
    seed: int = 42
    max_failures_per_target: int = 3
    base_backoff_sec: float = 0.5

    used_bytes: int = 0
    target_failures: dict[str, int] = field(default_factory=dict)
    dropped_targets: set[str] = field(default_factory=set)
    backoff_history: list[tuple[str, int, float]] = field(default_factory=list)
    jitter_history: list[float] = field(default_factory=list)

    def __post_init__(self) -> None:
        self.limit_bytes = max(1024, int(self.limit_bytes))
        if self.planned_worst_case_bytes <= 0:
            self.planned_worst_case_bytes = int(self.limit_bytes * 0.85)
        else:
            self.planned_worst_case_bytes = min(
                int(self.planned_worst_case_bytes), self.limit_bytes
            )
        self._lock = threading.Lock()
        self._rng = random.Random(int(self.seed))

    @property
    def exhausted(self) -> bool:
        with self._lock:
            return self.used_bytes >= self.limit_bytes

    def remaining_bytes(self) -> int:
        with self._lock:
            return max(0, self.limit_bytes - self.used_bytes)

    def allocate_probe_budget(self, desired_bytes: int) -> int:
        """Return the maximum bytes a probe is permitted to transfer within the remaining budget."""
        with self._lock:
            avail = max(0, self.limit_bytes - self.used_bytes)
            return min(max(0, int(desired_bytes)), avail)

    def consume_up_to(self, chunk_len: int) -> int:
        """Atomically charge up to `chunk_len` bytes against the budget and return allowed byte count.

        Guarantees `self.used_bytes <= self.limit_bytes` across concurrent worker threads.
        """
        if chunk_len <= 0:
            return 0
        with self._lock:
            avail = max(0, self.limit_bytes - self.used_bytes)
            taken = min(int(chunk_len), avail)
            self.used_bytes += taken
            return taken

    @staticmethod
    def clamp_connections(connections: int, *, allow_32: bool = False) -> int:
        """Enforce connection safety cap: max 16 per host (or 32 only if explicitly permitted)."""
        cap = 32 if allow_32 else 16
        return min(max(int(connections), 1), cap)

    def jitter_pause(self, min_sec: float = 0.15, max_sec: float = 0.45) -> float:
        """Sleep for a seeded random jitter interval via `Clock.sleep` between probes."""
        with self._lock:
            delay = self._rng.uniform(min_sec, max_sec)
            self.jitter_history.append(delay)
        get_clock().sleep(delay)
        return delay

    def record_success(self, target_id: str) -> None:
        """Reset consecutive failure count on a successful probe."""
        with self._lock:
            if target_id not in self.dropped_targets:
                self.target_failures[target_id] = 0

    def record_failure(self, target_id: str, status_code: int | None = None) -> bool:
        """Record a target failure, apply exponential backoff + jitter on 429/5xx, and drop after repeated failures.

        Returns True if the target has now been dropped.
        """
        with self._lock:
            cnt = self.target_failures.get(target_id, 0) + 1
            self.target_failures[target_id] = cnt
            is_rate_or_server_err = status_code in (429, 500, 502, 503, 504) or status_code is None
            backoff = 0.0
            if is_rate_or_server_err:
                mult = 2.0 if status_code == 429 else 1.5
                jitter = self._rng.uniform(0.05, 0.25)
                backoff = min(self.base_backoff_sec * (mult ** (cnt - 1)) + jitter, 15.0)
                self.backoff_history.append((target_id, status_code or 0, backoff))
            dropped = cnt >= self.max_failures_per_target
            if dropped:
                self.dropped_targets.add(target_id)

        if backoff > 0:
            get_clock().sleep(backoff)
        return dropped

    def is_dropped(self, target_id: str) -> bool:
        with self._lock:
            return target_id in self.dropped_targets

    def to_summary(self) -> BudgetSummary:
        with self._lock:
            return BudgetSummary(
                limit_bytes=self.limit_bytes,
                planned_worst_case_bytes=self.planned_worst_case_bytes,
                actual_used_bytes=self.used_bytes,
                exceeded=(self.used_bytes > self.limit_bytes),
            )
