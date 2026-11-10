"""REST API server for pdl — exposes download management via HTTP.

Endpoints mirror Gopeed's MCP tool set:

  POST   /resolve          -> classify/resolve a target without downloading  (resolve_task)
  POST   /tasks            -> create and queue a download                   (create_task)
  GET    /tasks            -> list all tasks with status & progress         (list_tasks)
  GET    /tasks/{id}       -> get a single task's details                   (get_task)
  POST   /tasks/{id}/pause -> pause a queued/running task                   (pause_task)
  POST   /tasks/{id}/resume-> resume a paused task                          (continue_task)
  DELETE /tasks/{id}       -> cancel & remove a task                        (delete_task)

Run with::

    pdl server                 # defaults: http://127.0.0.1:8000
    pdl server --port 8080     # custom port
    pdl server --host 0.0.0.0  # listen on all interfaces (careful: no auth yet)

Requires optional extras:  pip install "parallel-dl[api]"
"""

from __future__ import annotations

import contextlib
import logging
import threading
import uuid
from collections.abc import Callable
from dataclasses import dataclass, field
from datetime import datetime
from pathlib import Path
from typing import Any

from paradl.config import PDLConfig
from paradl.engines.aria2 import Aria2Engine
from paradl.history import HistoryManager
from paradl.models import (
    TargetType,
    TransferResult,
    WatchdogMode,
    format_speed,
)
from paradl.net import probe_target
from paradl.queue import QueueManager
from paradl.ui import UI, check_optional_dep

logger = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# Optional FastAPI / uvicorn import
# ---------------------------------------------------------------------------

try:
    import uvicorn
    from fastapi import FastAPI, HTTPException, Query
    from fastapi import Path as FastAPIPath
    from pydantic import BaseModel, ConfigDict, Field

    _FASTAPI_AVAILABLE = True
except ImportError:  # pragma: no cover - depends on optional install
    _FASTAPI_AVAILABLE = False

    class BaseModel:  # type: ignore[no-redef]
        """Fallback stub so module imports without fastapi installed."""
        def model_dump(self) -> dict[str, Any]:
            return {}

    class Field:  # type: ignore[no-redef]
        def __init__(self, default: Any = None, **_kw: Any) -> None:
            self.default = default


# ===========================================================================
# API request / response models
# ===========================================================================

class ResolveRequest(BaseModel):
    """Classify a download target without starting a transfer (Gopeed resolve_task)."""
    target: str = Field(..., description="URL, magnet, torrent, hf:/gh:/ipsw:/pip: shortcut, or search query")


class ResolveResponse(BaseModel):
    target: str
    target_type: str
    clean_url: str | None = None
    filename_hint: str | None = None
    content_length: int | None = None
    mime_type: str | None = None
    accept_ranges: bool | None = None
    etag_sha256: str | None = None
    expected_digest: str | None = None
    status_code: int | None = None
    probe_error: str | None = None


class CreateTaskRequest(BaseModel):
    """Create a new download task (Gopeed create_task)."""
    model_config = ConfigDict(extra="ignore")

    target: str = Field(..., description="URL or shortcut (hf:/gh:/ipsw:/pip: etc.)")
    out_dir: str | None = Field(
        default=None, description="Output directory (default: pdl config output_dir)"
    )
    filename: str | None = Field(default=None, description="Suggested output filename")
    connections: int = Field(default=16, ge=1, le=16, description="Parallel connection streams (1-16)")
    mirrors: list[str] = Field(default_factory=list, description="Alternate mirror URLs")
    priority: int = Field(default=0, description="Queue priority (higher runs first)")
    watchdog: str = Field(default="v2", description="Anti-throttle watchdog: v1, v2, or off")
    anti_throttle: str | None = Field(
        default="4MBps", description="Auto-recycle when speed drops below this (e.g. 4MBps)"
    )
    limit_rate: str | None = Field(default=None, description="Hard speed cap (e.g. 10MB/s)")
    require_ac: bool = Field(default=False, description="Fail unless on AC power")
    pause_on_low_power: bool = Field(
        default=False, description="Pause while macOS Low Power / Battery Saver is active"
    )
    when_fast_bps: float = Field(default=0.0, description="Only start when short speed test exceeds this")
    start_at: str | None = Field(default=None, description="Scheduled start time HH:MM")
    expected_digest: str | None = Field(
        default=None, description="Expected checksum (sha256:<hex>) for verification"
    )
    checksum: str | None = Field(
        default=None, description="Alias for expected_digest"
    )
    auto_extract: bool = Field(default=False, description="Auto-extract archives on completion")
    http3: bool = Field(default=True, description="Enable HTTP/3 (QUIC) when supported")
    proxy: str | None = Field(default=None, description="Proxy URL for aria2c transfers (--all-proxy)")
    is_audio: bool = Field(default=True, description="Media: extract audio only (vs video)")
    audio_codec: str = Field(default="aac", description="Media audio codec")
    video_quality: str = Field(default="best", description="Media video quality preset")
    playlist: bool = Field(default=False, description="Media: download entire playlist")
    subs: bool = Field(default=False, description="Media: download & embed subtitles")
    embed_thumbnail: bool = Field(default=True, description="Media: embed thumbnail")


class CreateTaskResponse(BaseModel):
    id: str
    status: str
    message: str


class TaskInfo(BaseModel):
    id: str
    target: str
    target_type: str
    status: str  # queued, running, paused, completed, failed
    out_dir: str
    filename: str | None = None
    connections: int = 16
    priority: int = 0
    bytes_downloaded: int = 0
    total_bytes: int | None = None
    speed_bps: float = 0.0
    file_path: str | None = None
    error: str | None = None
    created_at: str = ""
    updated_at: str = ""

    @property
    def progress_percent(self) -> float | None:
        if self.total_bytes and self.total_bytes > 0:
            return round(self.bytes_downloaded * 100.0 / self.total_bytes, 2)
        return None

    @property
    def speed_human(self) -> str:
        return format_speed(self.speed_bps) if self.speed_bps > 0 else "-"


class TaskListResponse(BaseModel):
    tasks: list[TaskInfo]
    count: int


class TaskActionResponse(BaseModel):
    id: str
    status: str
    message: str


# ===========================================================================
# In-memory task state store (thread-safe)
# ===========================================================================

@dataclass
class _ApiTask:
    id: str                       # API-facing task id (uuid or sqlite job id as str)
    target: str
    target_type: str
    queue_job_id: int | None = None   # SQLite queue_jobs.id when persisted there
    status: str = "queued"  # queued, running, paused, completed, failed
    out_dir: str = ""
    filename: str | None = None
    connections: int = 16
    priority: int = 0
    bytes_downloaded: int = 0
    total_bytes: int | None = None
    speed_bps: float = 0.0
    file_path: str | None = None
    error: str | None = None
    created_at: str = ""
    updated_at: str = ""
    options: dict[str, Any] = field(default_factory=dict)

    def to_api(self) -> TaskInfo:
        return TaskInfo(
            id=self.id,
            target=self.target,
            target_type=self.target_type,
            status=self.status,
            out_dir=self.out_dir,
            filename=self.filename,
            connections=self.connections,
            priority=self.priority,
            bytes_downloaded=self.bytes_downloaded,
            total_bytes=self.total_bytes,
            speed_bps=round(self.speed_bps, 2),
            file_path=self.file_path,
            error=self.error,
            created_at=self.created_at,
            updated_at=self.updated_at,
        )


class _TaskStore:
    """Thread-safe in-memory task store with per-task stop signals."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._tasks: dict[str, _ApiTask] = {}
        self._stop_events: dict[str, threading.Event] = {}
        self._threads: dict[str, threading.Thread] = {}

    def _now(self) -> str:
        return datetime.now().strftime("%Y-%m-%d %H:%M:%S")

    def create(self, task: _ApiTask) -> None:
        with self._lock:
            self._tasks[task.id] = task
            self._stop_events[task.id] = threading.Event()

    def get(self, task_id: str) -> _ApiTask | None:
        with self._lock:
            return self._tasks.get(task_id)

    def list_all(self) -> list[_ApiTask]:
        with self._lock:
            return sorted(self._tasks.values(), key=lambda t: t.created_at, reverse=True)

    def update(self, task_id: str, **fields: Any) -> bool:
        with self._lock:
            task = self._tasks.get(task_id)
            if task is None:
                return False
            for k, v in fields.items():
                if hasattr(task, k):
                    setattr(task, k, v)
            task.updated_at = self._now()
            return True

    def delete(self, task_id: str) -> bool:
        with self._lock:
            removed = self._tasks.pop(task_id, None) is not None
            self._stop_events.pop(task_id, None)
            self._threads.pop(task_id, None)
            return removed

    def set_stop_event(self, task_id: str, event: threading.Event) -> None:
        with self._lock:
            self._stop_events[task_id] = event

    def get_stop_event(self, task_id: str) -> threading.Event | None:
        with self._lock:
            return self._stop_events.get(task_id)

    def set_thread(self, task_id: str, thread: threading.Thread) -> None:
        with self._lock:
            self._threads[task_id] = thread

    def get_thread(self, task_id: str) -> threading.Thread | None:
        with self._lock:
            return self._threads.get(task_id)

    def is_stopped(self, task_id: str) -> bool:
        ev = self.get_stop_event(task_id)
        return ev is not None and ev.is_set()

    def request_stop(self, task_id: str) -> None:
        ev = self.get_stop_event(task_id)
        if ev is not None:
            ev.set()


# ===========================================================================
# Background task runner
# ===========================================================================

class _TaskRunner:
    """Picks up queued tasks from the store + SQLite queue and runs them via Aria2Engine."""

    def __init__(
        self,
        store: _TaskStore,
        config: PDLConfig,
        max_concurrent: int = 2,
    ) -> None:
        self.store = store
        self.config = config
        self.max_concurrent = max(1, max_concurrent)
        self._lock = threading.Lock()
        self._active = 0
        self._condition = threading.Condition(self._lock)
        self._stop = threading.Event()

    # -- internal helpers ---------------------------------------------------

    def _now(self) -> str:
        return datetime.now().strftime("%Y-%m-%d %H:%M:%S")

    def _active_count(self) -> int:
        with self._lock:
            return self._active

    def _inc_active(self) -> None:
        with self._lock:
            self._active += 1

    def _dec_active(self) -> None:
        with self._lock:
            self._active = max(0, self._active - 1)
            self._condition.notify_all()

    def _wait_for_slot(self) -> bool:
        """Block until a concurrency slot is free or the runner is stopped."""
        with self._condition:
            while self._active >= self.max_concurrent and not self._stop.is_set():
                self._condition.wait(0.5)
            return not self._stop.is_set()

    def _make_progress_cb(self, task_id: str) -> Callable[[int, int | None, float, int], None]:
        def cb(downloaded: int, total: int | None, speed: float, conns: int) -> None:
            self.store.update(
                task_id,
                bytes_downloaded=downloaded,
                total_bytes=total,
                speed_bps=speed,
                status="running",
            )
        return cb

    # -- single task execution --------------------------------------------

    def _execute(self, task: _ApiTask) -> None:
        """Run one download task to completion (or until stopped/failed)."""
        task_id = task.id
        self._inc_active()
        stop_event = self.store.get_stop_event(task_id)
        try:
            # Respect external stop before starting
            if stop_event is not None and stop_event.is_set():
                self.store.update(task_id, status="paused")
                return

            out_dir = Path(task.out_dir).expanduser().resolve() if task.out_dir else Path(
                self.config.output_dir
            ).expanduser().resolve()
            out_dir.mkdir(parents=True, exist_ok=True)

            # Classify the target
            t_info = probe_target(task.target)
            if task.filename:
                t_info.filename_hint = task.filename
            mirrors = list(task.options.get("mirrors", []))
            if t_info.mirrors:
                t_info.mirrors = mirrors + list(t_info.mirrors)
            elif mirrors:
                t_info.mirrors = mirrors

            # Parse watchdog mode
            try:
                wd_mode = WatchdogMode(str(task.options.get("watchdog_mode", "v2")).lower())
            except ValueError:
                wd_mode = WatchdogMode.V2

            result: TransferResult = Aria2Engine.download(
                t_info,
                out_dir,
                connections=task.connections,
                limit_rate=task.options.get("limit_rate"),
                anti_throttle=task.options.get("anti_throttle", "4MBps"),
                grace_period=int(task.options.get("grace_period", 5)),
                watchdog_mode=wd_mode,
                poll_interval=0.2,
                min_cooldown_sec=2.0,
                caffeinate=bool(task.options.get("caffeinate", self.config.caffeinate)),
                net_watch=bool(task.options.get("net_watch", self.config.net_watch)),
                stall_timeout=float(task.options.get("stall_timeout", self.config.stall_timeout)),
                require_ac=bool(task.options.get("require_ac", task.require_ac)),
                pause_on_low_power=bool(
                    task.options.get("pause_on_low_power", task.pause_on_low_power)
                ),
                mirrors=mirrors,
                when_fast_bps=float(task.options.get("when_fast_bps", task.when_fast_bps)),
                http3=bool(task.options.get("http3", True)),
                proxy=task.options.get("proxy"),
                on_progress=self._make_progress_cb(task_id),
            )

            # Check if we were stopped externally (pause/delete)
            if stop_event is not None and stop_event.is_set():
                self.store.update(task_id, status="paused")
                return

            if result.success:
                fp = str(result.file_path) if result.file_path else None
                self.store.update(
                    task_id,
                    status="completed",
                    bytes_downloaded=result.bytes_downloaded or 0,
                    file_path=fp,
                    error=None,
                )
                # Persist to history
                try:
                    HistoryManager.record(
                        target=task.target,
                        target_type=t_info.target_type.value,
                        file_path=fp,
                        size=result.bytes_downloaded,
                        duration=result.duration,
                        success=True,
                        sha256=result.checksum_digest if result.checksum_algo == "sha256" else None,
                        history_limit=self.config.history_limit,
                    )
                except Exception:
                    logger.debug("Failed to record history for task %s", task_id)
            else:
                self.store.update(
                    task_id,
                    status="failed",
                    bytes_downloaded=result.bytes_downloaded or 0,
                    error=result.error or "Download failed",
                )

        except Exception as exc:  # pragma: no cover - defensive
            self.store.update(task_id, status="failed", error=str(exc))
        finally:
            self._dec_active()

    # -- runner loop --------------------------------------------------------

    def start(self) -> None:
        """Start the background runner thread."""
        self._thread = threading.Thread(target=self._loop, name="pdl-api-runner", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        """Signal the runner to stop and wait briefly."""
        self._stop.set()
        with self._condition:
            self._condition.notify_all()

    def _loop(self) -> None:
        """Main runner loop: dispatch queued tasks to worker threads."""
        while not self._stop.is_set():
            if not self._wait_for_slot():
                break

            # Collect runnable tasks from the in-memory store (status == queued)
            with self.store._lock:
                queued = [t for t in self.store._tasks.values() if t.status == "queued"]

            # Also recover any jobs stuck in 'running' from a previous crash
            with contextlib.suppress(Exception):
                QueueManager.recover_crashed_jobs()

            # Sync any SQLite queue jobs that are queued but not yet in our store
            try:
                existing_ids = {t.id for t in queued}
                for qjob in QueueManager.list_jobs(status="queued"):
                    if str(qjob.id) in existing_ids:
                        continue
                    api_task = _ApiTask(
                        id=str(qjob.id),
                        queue_job_id=qjob.id,
                        target=qjob.target,
                        target_type=qjob.target_type,
                        status="queued",
                        out_dir=qjob.out_dir,
                        filename=qjob.filename,
                        connections=qjob.connections,
                        priority=qjob.priority,
                        created_at=qjob.created_at,
                        updated_at=qjob.updated_at,
                        options=dict(qjob.options or {}),
                    )
                    # Override with job-specific fields
                    if qjob.when_fast_bps:
                        api_task.options["when_fast_bps"] = qjob.when_fast_bps
                    if qjob.start_at:
                        api_task.options["start_at"] = qjob.start_at
                    if qjob.require_ac is not None:
                        api_task.require_ac = qjob.require_ac
                    if qjob.pause_on_low_power is not None:
                        api_task.pause_on_low_power = qjob.pause_on_low_power
                    if qjob.expected_digest:
                        api_task.options["expected_digest"] = qjob.expected_digest
                    if qjob.mirrors:
                        api_task.options["mirrors"] = list(qjob.mirrors)
                    self.store.create(api_task)
                    queued.append(api_task)
                    existing_ids.add(str(qjob.id))
            except Exception:
                pass

            if not queued:
                # Nothing to do; sleep briefly before polling again
                self._stop.wait(0.5)
                continue

            # Sort by priority DESC then created_at ASC
            queued.sort(key=lambda t: (-t.priority, t.created_at))

            for task in queued:
                if self._stop.is_set():
                    break
                if self._active_count() >= self.max_concurrent:
                    break
                # Skip if already assigned to a running thread
                if self.store.get_thread(task.id) is not None and self.store.get_thread(task.id).is_alive():
                    continue
                # Check stop event before dispatching
                if self.store.is_stopped(task.id):
                    continue

                # Mark running optimistically; _execute will correct on failure
                self.store.update(task.id, status="running", updated_at=self._now())
                stop_event = threading.Event()
                self.store.set_stop_event(task.id, stop_event)

                worker = threading.Thread(
                    target=self._execute,
                    args=(task,),
                    name=f"pdl-api-worker-{task.id[:8]}",
                    daemon=True,
                )
                self.store.set_thread(task.id, worker)
                worker.start()

        # Drain: wait for active workers to finish (best effort)
        with self.store._lock:
            threads = list(self.store._threads.values())
        for th in threads:
            th.join(timeout=5.0)


# ===========================================================================
# Global API state (singleton pattern for simple server lifecycle)
# ===========================================================================

_api_state: dict[str, Any] = {}


def _get_state() -> dict[str, Any]:
    if not _api_state:
        _api_state["config"] = PDLConfig.load()
        _api_state["store"] = _TaskStore()
        _api_state["runner"] = _TaskRunner(
            _api_state["store"],
            _api_state["config"],
            max_concurrent=2,
        )
        _api_state["runner"].start()
        _api_state["app"] = _build_app(_api_state["store"], _api_state["config"])
    return _api_state


# ===========================================================================
# FastAPI app construction
# ===========================================================================

def _build_app(store: _TaskStore, config: PDLConfig) -> FastAPI:
    app = FastAPI(
        title="pdl API",
        description="REST API for parallel-dl (paradl) download management. Mirrors Gopeed's MCP tool set.",
        version="1.0.0",
    )

    # ------------------------------------------------------------------
    # POST /resolve
    # ------------------------------------------------------------------
    @app.post("/resolve", response_model=ResolveResponse, tags=["tasks"])
    def resolve(req: ResolveRequest) -> ResolveResponse:
        """Classify a download target without starting a transfer."""
        try:
            t_info = probe_target(req.target)
        except Exception as exc:
            raise HTTPException(status_code=400, detail=f"Failed to resolve target: {exc}") from exc

        return ResolveResponse(
            target=t_info.raw,
            target_type=t_info.target_type.value,
            clean_url=t_info.clean_url or None,
            filename_hint=t_info.filename_hint,
            content_length=t_info.content_length,
            mime_type=t_info.mime_type,
            accept_ranges=t_info.accept_ranges,
            etag_sha256=t_info.etag_sha256,
            expected_digest=t_info.expected_digest,
            status_code=t_info.status_code,
            probe_error=t_info.probe_error,
        )

    # ------------------------------------------------------------------
    # POST /tasks
    # ------------------------------------------------------------------
    @app.post("/tasks", response_model=CreateTaskResponse, status_code=202, tags=["tasks"])
    def create_task(req: CreateTaskRequest) -> CreateTaskResponse:
        """Create and queue a new download task."""
        task_id = str(uuid.uuid4())
        out_dir = req.out_dir or config.output_dir

        # Determine target type up front for status reporting
        try:
            probe = probe_target(req.target)
            target_type = probe.target_type.value
        except Exception:
            target_type = TargetType.DIRECT.value

        api_task = _ApiTask(
            id=task_id,
            target=req.target,
            target_type=target_type,
            status="queued",
            out_dir=out_dir,
            filename=req.filename,
            connections=req.connections,
            priority=req.priority,
            created_at=datetime.now().strftime("%Y-%m-%d %H:%M:%S"),
            updated_at=datetime.now().strftime("%Y-%m-%d %H:%M:%S"),
            options={
                "limit_rate": req.limit_rate,
                "anti_throttle": req.anti_throttle,
                "grace_period": int(config.anti_throttle_grace),
                "watchdog_mode": req.watchdog,
                "caffeinate": config.caffeinate,
                "net_watch": config.net_watch,
                "stall_timeout": config.stall_timeout,
                "require_ac": req.require_ac,
                "pause_on_low_power": req.pause_on_low_power,
                "when_fast_bps": req.when_fast_bps,
                "start_at": req.start_at,
                "mirrors": req.mirrors,
                "http3": req.http3,
                "proxy": req.proxy,
                "is_audio": req.is_audio,
                "audio_codec": req.audio_codec,
                "video_quality": req.video_quality,
                "playlist": req.playlist,
                "subs": req.subs,
                "embed_thumbnail": req.embed_thumbnail,
                "auto_extract": req.auto_extract,
            },
        )
        if req.expected_digest:
            api_task.options["expected_digest"] = req.expected_digest
        elif req.checksum:
            api_task.options["expected_digest"] = req.checksum

        # Persist to SQLite queue
        qjob_id: int | None = None
        try:
            qjob = QueueManager.add_job(
                target=req.target,
                out_dir=Path(out_dir),
                target_type=target_type,
                filename=req.filename,
                mirrors=req.mirrors,
                priority=req.priority,
                connections=req.connections,
                require_ac=req.require_ac,
                pause_on_low_power=req.pause_on_low_power,
                when_fast_bps=req.when_fast_bps,
                start_at=req.start_at,
                expected_digest=req.expected_digest or req.checksum,
                options=dict(api_task.options),
            )
            qjob_id = int(qjob.id)
            # Use the stable SQLite job id as the API task id
            api_task.id = str(qjob_id)
            task_id = api_task.id
        except Exception:
            # Queue persist failed — still keep the in-memory task with uuid id
            pass

        api_task.queue_job_id = qjob_id
        store.create(api_task)
        # The runner loop will pick it up on its next tick

        return CreateTaskResponse(
            id=task_id,
            status="queued",
            message="Task created and queued.",
        )

    # ------------------------------------------------------------------
    # GET /tasks
    # ------------------------------------------------------------------
    @app.get("/tasks", response_model=TaskListResponse, tags=["tasks"])
    def list_tasks(
        status: str | None = Query(default=None, description="Filter by status: queued, running, paused, completed, failed"),
    ) -> TaskListResponse:
        """List all tasks, optionally filtered by status."""
        tasks = store.list_all()
        if status:
            tasks = [t for t in tasks if t.status == status]
        return TaskListResponse(
            tasks=[t.to_api() for t in tasks],
            count=len(tasks),
        )

    # ------------------------------------------------------------------
    # GET /tasks/{id}
    # ------------------------------------------------------------------
    @app.get("/tasks/{task_id}", response_model=TaskInfo, tags=["tasks"])
    def get_task(task_id: str = FastAPIPath(..., description="Task ID")) -> TaskInfo:
        """Get a single task's details and progress."""
        task = store.get(task_id)
        if task is None:
            raise HTTPException(status_code=404, detail=f"Task {task_id} not found")
        return task.to_api()

    # ------------------------------------------------------------------
    # POST /tasks/{id}/pause
    # ------------------------------------------------------------------
    @app.post("/tasks/{task_id}/pause", response_model=TaskActionResponse, tags=["tasks"])
    def pause_task(task_id: str = FastAPIPath(..., description="Task ID")) -> TaskActionResponse:
        """Pause a queued or running task."""
        task = store.get(task_id)
        if task is None:
            raise HTTPException(status_code=404, detail=f"Task {task_id} not found")
        if task.status in ("completed", "failed"):
            raise HTTPException(
                status_code=400,
                detail=f"Cannot pause a {task.status} task",
            )
        store.update(task_id, status="paused")
        store.request_stop(task_id)
        # Also mark in SQLite queue if we have a job id
        if task.queue_job_id is not None:
            with contextlib.suppress(Exception):
                QueueManager.update_job_status(task.queue_job_id, "paused")
        return TaskActionResponse(id=task_id, status="paused", message="Task paused.")

    # ------------------------------------------------------------------
    # POST /tasks/{id}/resume
    # ------------------------------------------------------------------
    @app.post("/tasks/{task_id}/resume", response_model=TaskActionResponse, tags=["tasks"])
    def resume_task(task_id: str = FastAPIPath(..., description="Task ID")) -> TaskActionResponse:
        """Resume (continue) a paused task."""
        task = store.get(task_id)
        if task is None:
            raise HTTPException(status_code=404, detail=f"Task {task_id} not found")
        if task.status not in ("paused", "failed"):
            raise HTTPException(
                status_code=400,
                detail=f"Only paused or failed tasks can be resumed (current: {task.status})",
            )
        # Clear stop event so a new worker can start
        store.update(task_id, status="queued")
        ev = store.get_stop_event(task_id)
        if ev is not None:
            ev.clear()
        # Re-persist to SQLite if we have a job id
        if task.queue_job_id is not None:
            with contextlib.suppress(Exception):
                QueueManager.update_job_status(task.queue_job_id, "queued", error=None)
        return TaskActionResponse(id=task_id, status="queued", message="Task queued for resume.")

    # ------------------------------------------------------------------
    # DELETE /tasks/{id}
    # ------------------------------------------------------------------
    @app.delete("/tasks/{task_id}", response_model=TaskActionResponse, tags=["tasks"])
    def delete_task(task_id: str = FastAPIPath(..., description="Task ID")) -> TaskActionResponse:
        """Cancel and remove a task."""
        task = store.get(task_id)
        if task is None:
            raise HTTPException(status_code=404, detail=f"Task {task_id} not found")

        # Signal any running worker to stop
        store.request_stop(task_id)

        # Remove from store
        store.delete(task_id)

        # Remove from SQLite queue if we have a job id
        if task.queue_job_id is not None:
            with contextlib.suppress(Exception):
                QueueManager.remove_jobs([task.queue_job_id])

        return TaskActionResponse(id=task_id, status="deleted", message="Task cancelled and removed.")

    return app


# ===========================================================================
# Server entrypoint
# ===========================================================================

def create_app() -> FastAPI:
    """Build and return the FastAPI application (starts the background runner)."""
    state = _get_state()
    return state["app"]


def run_server(
    host: str = "127.0.0.1",
    port: int = 8000,
    *,
    reload: bool = False,
    workers: int = 1,
) -> None:
    """Run the pdl REST API server (blocking)."""
    check_optional_dep("fastapi", "api", install_cmd="pip install 'parallel-dl[api]'")

    # Ensure the runner is started
    _get_state()

    UI.emit_event("api_server_started", host=host, port=port)
    uvicorn.run(  # type: ignore
        create_app(),
        host=host,
        port=port,
        reload=reload,
        workers=workers,
        log_level="warning",
    )
