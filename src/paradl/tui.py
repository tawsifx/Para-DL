"""Textual-based TUI for pdl download management.

An in-terminal download manager with task lists, progress bars,
pause/resume, categories, and search — keeping the CLI-first ethos
while adding a visual layer.

Run with::

    pdl tui
"""

from __future__ import annotations

import contextlib
import os
import threading
import time
from datetime import datetime
from pathlib import Path
from typing import Any

from paradl.config import PDLConfig
from paradl.engines.aria2 import Aria2Engine, WatchdogMode
from paradl.history import HistoryManager
from paradl.models import (
    TargetType,
    TransferResult,
    format_size,
    format_speed,
)
from paradl.net import probe_target
from paradl.queue import QueueJob, QueueManager
from paradl.ui import check_optional_dep

# ---------------------------------------------------------------------------
# Optional Textual import (lazy-checked in run_tui)
# ---------------------------------------------------------------------------

try:
    from textual.app import App as _AppBase
    from textual.app import ComposeResult
    from textual.containers import Container, Horizontal, Vertical
    from textual.reactive import reactive
    from textual.screen import Screen
    from textual.widgets import (
        Button,
        DataTable,
        Footer,
        Header,
        Input,
        Select,
        Static,
    )

    _TEXTUAL_AVAILABLE = True
except ImportError:  # pragma: no cover - depends on optional install
    _TEXTUAL_AVAILABLE = False
    _AppBase: Any = object  # type: ignore[no-redef]
    ComposeResult: Any = None  # type: ignore[no-redef]
    reactive: Any = None  # type: ignore[no-redef]
    Screen: Any = None  # type: ignore[no-redef]
    Button: Any = None  # type: ignore[no-redef]
    DataTable: Any = None  # type: ignore[no-redef]
    Footer: Any = None  # type: ignore[no-redef]
    Header: Any = None  # type: ignore[no-redef]
    Input: Any = None  # type: ignore[no-redef]
    Select: Any = None  # type: ignore[no-redef]
    Static: Any = None  # type: ignore[no-redef]
    Container: Any = None  # type: ignore[no-redef]
    Horizontal: Any = None  # type: ignore[no-redef]
    Vertical: Any = None  # type: ignore[no-redef]

# ---------------------------------------------------------------------------
# Semantic color tokens (Operate mode: restrained, meaning-bearing)
# ---------------------------------------------------------------------------

STATUS_COLORS: dict[str, str] = {
    "queued": "#888888",      # muted neutral
    "running": "#00ff88",     # green — active progress
    "paused": "#ffcc00",      # amber — suspendable
    "completed": "#00cc66",   # green — done
    "failed": "#ff4444",      # red — error
}

STATUS_LABELS: dict[str, str] = {
    "queued": "queued",
    "running": "running",
    "paused": "paused",
    "completed": "completed",
    "failed": "failed",
}

# ---------------------------------------------------------------------------
# Reactive task model shared between the UI and background download runner
# ---------------------------------------------------------------------------


class _TaskState:
    """Thread-safe snapshot of a single task's runtime state for the TUI."""

    def __init__(self, job: QueueJob) -> None:
        self.id = job.id
        self.target = job.target
        self.target_type = job.target_type
        self.category = job.category
        self.out_dir = job.out_dir
        self.filename = job.filename
        self.status = job.status
        self.connections = job.connections
        self.priority = job.priority
        self.bytes_downloaded = job.bytes_downloaded
        self.total_bytes = job.expected_length
        self.speed_bps = 0.0
        self.error: str | None = job.error
        self.file_path: str | None = None
        self.created_at = job.created_at
        self.updated_at = job.updated_at

    def progress_pct(self) -> float:
        """Return completion percentage (0-100)."""
        if not self.total_bytes or self.total_bytes <= 0:
            return 0.0
        return round(self.bytes_downloaded * 100.0 / self.total_bytes, 1)

    def progress_bar(self, width: int = 24) -> str:
        """Return a Unicode block progress bar string."""
        pct = self.progress_pct()
        if pct == 0.0:
            return "░" * width
        filled = max(1, int(width * pct / 100))
        rest = max(0, width - filled)
        return "█" * filled + "░" * rest

    def status_text(self) -> str:
        """Return human-readable status label."""
        return STATUS_LABELS.get(self.status, self.status)

    def to_row(self) -> list[Any]:
        """Return a DataTable row for this task."""
        pct = self.progress_pct()
        prog = (
            f"{format_size(self.bytes_downloaded)} / {format_size(self.total_bytes)} ({pct}%)"
            if self.total_bytes
            else format_size(self.bytes_downloaded)
        )
        return [
            str(self.id),
            (self.filename or self.target)[:46],
            self.category or "—",
            prog,
            self.status_text(),
            format_speed(self.speed_bps) if self.speed_bps > 0 else "—",
            str(self.connections),
        ]


# ---------------------------------------------------------------------------
# Thread-safe task store
# ---------------------------------------------------------------------------


class _TaskStore:
    """Thread-safe store of task states keyed by task id."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._tasks: dict[int, _TaskState] = {}

    def upsert(self, job: QueueJob) -> None:
        with self._lock:
            self._tasks[job.id] = _TaskState(job)

    def get(self, task_id: int) -> _TaskState | None:
        with self._lock:
            return self._tasks.get(task_id)

    def update_progress(
        self,
        task_id: int,
        *,
        bytes_downloaded: int | None = None,
        total_bytes: int | None = None,
        speed_bps: float | None = None,
        status: str | None = None,
        error: str | None = None,
        file_path: str | None = None,
    ) -> None:
        with self._lock:
            task = self._tasks.get(task_id)
            if task is None:
                return
            if bytes_downloaded is not None:
                task.bytes_downloaded = bytes_downloaded
            if total_bytes is not None:
                task.total_bytes = total_bytes
            if speed_bps is not None:
                task.speed_bps = speed_bps
            if status is not None:
                task.status = status
            if error is not None:
                task.error = error
            if file_path is not None:
                task.file_path = file_path
            task.updated_at = datetime.now().strftime("%Y-%m-%d %H:%M:%S")

    def list_all(self) -> list[_TaskState]:
        with self._lock:
            return sorted(self._tasks.values(), key=lambda t: t.created_at, reverse=True)

    def set_status(self, task_id: int, status: str) -> None:
        with self._lock:
            task = self._tasks.get(task_id)
            if task:
                task.status = status
                task.updated_at = datetime.now().strftime("%Y-%m-%d %H:%M:%S")

    def remove(self, task_id: int) -> None:
        with self._lock:
            self._tasks.pop(task_id, None)

    def clear(self) -> None:
        with self._lock:
            self._tasks.clear()


_task_store = _TaskStore()


# ---------------------------------------------------------------------------
# Background download runner (threaded, non-blocking the TUI)
# ---------------------------------------------------------------------------


class _DownloadRunner:
    """Processes queued jobs in a background thread using Aria2Engine."""

    def __init__(self, store: _TaskStore, config: PDLConfig, app: PdlTuiApp) -> None:
        self.store = store
        self.config = config
        self.app = app
        self._thread: threading.Thread | None = None
        self._stop = threading.Event()
        self._lock = threading.Lock()

    def start(self) -> None:
        if self._thread is not None and self._thread.is_alive():
            return
        self._stop.clear()
        self._thread = threading.Thread(
            target=self._loop, name="pdl-tui-runner", daemon=True
        )
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()

    def _loop(self) -> None:
        while not self._stop.is_set():
            jobs = QueueManager.list_jobs(status="queued")
            if not jobs:
                time.sleep(0.5)
                continue

            for job in jobs:
                if self._stop.is_set():
                    break
                state = self.store.get(job.id)
                if state and state.status == "running":
                    continue

                self._run_job(job)

    def _run_job(self, job: QueueJob) -> None:
        """Run a single job to completion (or until stopped/failed)."""
        task_id = job.id

        self.store.update_progress(task_id, status="running")
        QueueManager.update_job_status(task_id, "running")

        def on_progress(
            downloaded: int, total: int | None, speed: float, conns: int
        ) -> None:
            self.app.call_from_thread(
                self.store.update_progress,
                task_id,
                bytes_downloaded=downloaded,
                total_bytes=total,
                speed_bps=speed,
                status="running",
            )

        try:
            t_info = probe_target(job.target)
            if job.filename:
                t_info.filename_hint = job.filename
            if job.mirrors:
                t_info.mirrors = list(job.mirrors)

            try:
                wd_mode = WatchdogMode(
                    str(job.options.get("watchdog_mode", "v2")).lower()
                )
            except ValueError:
                wd_mode = WatchdogMode.V2

            result: TransferResult = Aria2Engine.download(
                t_info,
                Path(job.out_dir),
                connections=job.connections,
                limit_rate=job.options.get("limit_rate"),
                anti_throttle=job.options.get("anti_throttle", "4MBps"),
                grace_period=int(job.options.get("grace_period", 5)),
                watchdog_mode=wd_mode,
                poll_interval=0.2,
                min_cooldown_sec=2.0,
                caffeinate=bool(job.options.get("caffeinate", self.config.caffeinate)),
                net_watch=bool(job.options.get("net_watch", self.config.net_watch)),
                stall_timeout=float(job.options.get("stall_timeout", self.config.stall_timeout)),
                require_ac=bool(job.options.get("require_ac", job.require_ac)),
                pause_on_low_power=bool(
                    job.options.get("pause_on_low_power", job.pause_on_low_power)
                ),
                mirrors=list(job.mirrors),
                when_fast_bps=float(job.options.get("when_fast_bps", job.when_fast_bps)),
                http3=bool(job.options.get("http3", True)),
                proxy=job.options.get("proxy"),
                on_progress=on_progress,
            )

            if result.success:
                fp = str(result.file_path) if result.file_path else None
                self.store.update_progress(
                    task_id,
                    status="completed",
                    bytes_downloaded=result.bytes_downloaded or 0,
                    file_path=fp,
                )
                QueueManager.update_job_status(
                    task_id,
                    "completed",
                    bytes_downloaded=result.bytes_downloaded or 0,
                    filename=os.path.basename(fp) if fp else job.filename,
                )
                with contextlib.suppress(Exception):
                    HistoryManager.record(
                        target=job.target,
                        target_type=t_info.target_type.value,
                        file_path=fp,
                        size=result.bytes_downloaded,
                        duration=result.duration,
                        success=True,
                        sha256=result.checksum_digest
                        if result.checksum_algo == "sha256"
                        else None,
                        history_limit=self.config.history_limit,
                    )
            else:
                self.store.update_progress(
                    task_id,
                    status="failed",
                    error=result.error or "Download failed",
                )
                QueueManager.update_job_status(
                    task_id,
                    "failed",
                    error=result.error or "Download failed",
                )
        except Exception as exc:  # pragma: no cover - defensive
            self.store.update_progress(task_id, status="failed", error=str(exc))
            QueueManager.update_job_status(task_id, "failed", error=str(exc))


# ---------------------------------------------------------------------------
# Textual App
# ---------------------------------------------------------------------------


class PdlTuiApp(_AppBase[None]):
    """pdl TUI — download manager with task list, progress, pause/resume."""

    CSS = """
    Screen {
        layout: vertical;
        background: $surface;
    }

    /* Header — docked top, app identity */
    #header {
        dock: top;
        height: 3;
        background: $panel;
        color: $text;
        padding: 0 1;
    }

    #header-title {
        text-style: bold;
        text-align: left;
    }

    #header-subtitle {
        color: $text-muted;
        text-align: left;
    }

    /* Filter bar — search + category, grouped */
    #filter-container {
        height: 3;
        margin: 1 1;
        padding: 0 1;
        background: $panel;
    }

    /* Task table — fills remaining space */
    #task-table {
        height: 1fr;
        margin: 0 1;
    }

    /* Action bar — single row of primary actions */
    #action-bar {
        height: 3;
        margin: 1 1;
    }

    /* Status bar — counts + keyboard hints, docked bottom */
    #status-bar {
        dock: bottom;
        height: 1;
        margin: 0 1;
        color: $text-muted;
    }

    /* Empty state — centered, teaches the interface */
    #empty-state {
        align: center middle;
        height: 1fr;
    }

    #empty-title {
        text-style: bold;
        color: $text;
        text-align: center;
    }

    #empty-hint {
        color: $text-muted;
        text-align: center;
    }

    /* Semantic status colors — color encodes state, never decoration */
    .status-queued   { color: #888888; }
    .status-running  { color: #00ff88; }
    .status-paused   { color: #ffcc00; }
    .status-completed{ color: #00cc66; }
    .status-failed   { color: #ff4444; }

    /* Row focus */
    DataTable > .datatable--cursor {
        background: $primary 20%;
    }
    """

    TITLE = "pdl — Parallel Download Manager"
    SUB_TITLE = "Textual TUI"

    tasks: reactive[list[_TaskState]] = reactive([])
    refresh_interval: reactive[float] = reactive(1.0)

    def __init__(self, config: PDLConfig | None = None) -> None:
        super().__init__()
        self.config = config or PDLConfig.load()
        self.runner = _DownloadRunner(_task_store, self.config, self)
        self._table: DataTable[Any] | None = None

    # -- composition -------------------------------------------------------

    def compose(self) -> ComposeResult:
        yield Header()
        yield Container(
            # Header
            Container(
                Static("pdl", id="header-title", classes="header-title"),
                Static("Parallel Download Manager", id="header-subtitle", classes="header-subtitle"),
                id="header",
            ),
            # Filter bar
            Container(
                Input(placeholder="Search targets…", id="search-input"),
                Select(
                    [
                        ("All categories", ""),
                        ("software", "software"),
                        ("media", "media"),
                        ("documents", "documents"),
                        ("archives", "archives"),
                    ],
                    value="",
                    id="category-filter",
                    allow_blank=True,
                ),
                id="filter-container",
            ),
            # Task table
            DataTable(id="task-table", zebra_stripes=True, show_header=True),
            # Empty state overlay (shown when no tasks)
            Container(
                Static("No downloads yet", id="empty-title"),
                Static("Press 'a' to add your first download", id="empty-hint"),
                id="empty-state",
            ),
            # Action bar — primary actions in a single row
            Horizontal(
                Button("Add", id="btn-add", variant="primary"),
                Button("Start Queue", id="btn-start", variant="success"),
                Button("Pause", id="btn-pause"),
                Button("Resume", id="btn-resume"),
                Button("Delete", id="btn-delete", variant="error"),
                Button("Refresh", id="btn-refresh"),
                Button("Quit", id="btn-quit"),
                id="action-bar",
            ),
            # Status bar — docked bottom, separate from buttons
            Static("Ready", id="status-bar"),
            id="main-container",
        )
        yield Footer()

    # -- lifecycle ---------------------------------------------------------

    def on_mount(self) -> None:
        self._table = self.query_one("#task-table", DataTable)
        self._table.add_columns(
            "ID", "Target / Filename", "Category", "Progress", "Status", "Speed", "Conns"
        )
        self._load_tasks()
        self._refresh_table()
        self.set_interval(self.refresh_interval, self._refresh_table)

        # Keyboard shortcuts
        self._bind_shortcuts()

    def _bind_shortcuts(self) -> None:
        """Register keyboard shortcuts for core actions."""
        # Shortcuts are handled via action_* methods; display hints in status bar

    # -- task loading ------------------------------------------------------

    def _load_tasks(self) -> None:
        """Load all jobs from SQLite into the in-memory store."""
        _task_store.clear()
        for job in QueueManager.list_jobs():
            _task_store.upsert(job)

    def _refresh_table(self) -> None:
        """Redraw the DataTable from the in-memory store."""
        table = self._table
        if table is None:
            return

        search = self.query_one("#search-input", Input).value.strip().lower()
        category = self.query_one("#category-filter", Select).value or ""
        category = str(category) if category else ""

        tasks = _task_store.list_all()
        filtered = [
            t
            for t in tasks
            if (search in t.target.lower() or search in (t.filename or "").lower())
            and (not category or t.category == category)
        ]

        self.tasks = filtered
        table.clear()
        for t in filtered:
            row = t.to_row()
            table.add_row(
                *row,
                key=str(t.id),
                label=f"task-{t.id}",
            )

        # Show/hide empty state
        empty = self.query_one("#empty-state", Container)
        if filtered:
            empty.display = False
        else:
            empty.display = True

        # Update status bar with counts + keyboard hints
        counts: dict[str, int] = {}
        for t in tasks:
            counts[t.status] = counts.get(t.status, 0) + 1
        parts = [f"{k}: {v}" for k, v in sorted(counts.items())]
        status_text = " | ".join(parts) if parts else "No tasks"
        hints = "  a:add  p:pause  r:resume  d:delete  q:quit"
        self.query_one("#status-bar", Static).update(status_text + hints)

    # -- actions -----------------------------------------------------------

    def action_add(self) -> None:
        """Open the add-job dialog."""
        self.push_screen(_AddJobScreen(self.config), self._on_job_added)

    def _on_job_added(self, job: QueueJob | None) -> None:
        """Callback when a new job is added from the dialog."""
        if job is None:
            return
        _task_store.upsert(job)
        self._refresh_table()
        self.notify(f"Added job #{job.id}", timeout=2)

    def action_pause(self) -> None:
        task_id = self._selected_id()
        if task_id is None:
            self.notify("No task selected", timeout=2)
            return
        _task_store.set_status(task_id, "paused")
        QueueManager.update_job_status(task_id, "paused")
        self.notify(f"Paused job #{task_id}", timeout=2)
        self._refresh_table()

    def action_resume(self) -> None:
        task_id = self._selected_id()
        if task_id is None:
            self.notify("No task selected", timeout=2)
            return
        _task_store.set_status(task_id, "queued")
        QueueManager.update_job_status(task_id, "queued")
        self.notify(f"Resumed job #{task_id}", timeout=2)
        self._refresh_table()

    def action_delete(self) -> None:
        task_id = self._selected_id()
        if task_id is None:
            self.notify("No task selected", timeout=2)
            return
        QueueManager.remove_jobs([task_id])
        _task_store.remove(task_id)
        self.notify(f"Deleted job #{task_id}", timeout=2)
        self._refresh_table()

    def action_start_queue(self) -> None:
        self.runner.start()
        self.notify("Queue runner started", timeout=2)

    def _selected_id(self) -> int | None:
        table = self._table
        if table is None:
            return None
        row = table.cursor_row
        if row is None:
            return None
        try:
            keys = list(table.rows.keys())
            if row >= len(keys):
                return None
            row_key = keys[row]
        except Exception:
            return None
        if row_key is None:
            return None
        key_value = row_key.value
        if key_value is None:
            return None
        try:
            return int(key_value)
        except ValueError:
            return None

    # -- event handlers ----------------------------------------------------

    def on_button_pressed(self, event: Button.Pressed) -> None:
        btn_id = event.button.id
        if btn_id == "btn-add":
            self.action_add()
        elif btn_id == "btn-pause":
            self.action_pause()
        elif btn_id == "btn-resume":
            self.action_resume()
        elif btn_id == "btn-delete":
            self.action_delete()
        elif btn_id == "btn-start":
            self.action_start_queue()
        elif btn_id == "btn-refresh":
            self._load_tasks()
            self._refresh_table()
        elif btn_id == "btn-quit":
            self.runner.stop()
            self.exit()

    def on_input_changed(self, event: Input.Changed) -> None:
        if event.input.id == "search-input":
            self._refresh_table()

    def on_select_changed(self, event: Select.Changed) -> None:
        if event.select.id == "category-filter":
            self._refresh_table()


# ---------------------------------------------------------------------------
# Add job dialog
# ---------------------------------------------------------------------------


class _AddJobScreen(Screen[QueueJob | None]):
    """Modal dialog for adding a new download job."""

    def __init__(self, config: PDLConfig) -> None:
        super().__init__()
        self.config = config

    CSS = """
    #add-dialog {
        align: center middle;
        width: 72;
        height: auto;
        background: $panel;
        border: thick $primary;
        padding: 1 2;
    }

    #add-dialog > Container {
        height: auto;
    }

    #dialog-title {
        text-style: bold;
        color: $text;
        text-align: left;
        margin-bottom: 1;
    }

    .field-label {
        color: $text-muted;
        text-align: left;
        margin-top: 1;
        margin-bottom: 0;
    }

    #dialog-buttons {
        height: 3;
        margin-top: 1;
    }
    """

    def compose(self) -> ComposeResult:
        yield Container(
            Static("Add Download Job", id="dialog-title"),
            Static("Target (URL or shortcut)", classes="field-label"),
            Input(placeholder="e.g. https://… or gh:owner/repo", id="target-input"),
            Static("Category", classes="field-label"),
            Input(placeholder="e.g. software, media", id="category-input"),
            Static("Output dir", classes="field-label"),
            Input(placeholder="default: .", id="outdir-input"),
            Static("Connections (1-16)", classes="field-label"),
            Input(placeholder="16", id="conns-input"),
            Horizontal(
                Button("Add", id="btn-dialog-add", variant="primary"),
                Button("Cancel", id="btn-dialog-cancel"),
                id="dialog-buttons",
            ),
            id="add-dialog",
        )

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "btn-dialog-add":
            self._submit()
        elif event.button.id == "btn-dialog-cancel":
            self.dismiss(None)

    def _submit(self) -> None:
        target = self.query_one("#target-input", Input).value.strip()
        if not target:
            self.notify("Target is required", timeout=2)
            return

        category = self.query_one("#category-input", Input).value.strip() or ""
        out_dir = self.query_one("#outdir-input", Input).value.strip() or "."
        conns_raw = self.query_one("#conns-input", Input).value.strip()
        connections = 16
        if conns_raw:
            try:
                connections = min(max(int(conns_raw), 1), 16)
            except ValueError:
                connections = 16

        try:
            t_info = probe_target(target)
        except Exception:
            t_info = None

        target_type = t_info.target_type.value if t_info else TargetType.DIRECT.value

        job = QueueManager.add_job(
            target=target,
            out_dir=Path(out_dir),
            target_type=target_type,
            category=category,
            connections=connections,
        )

        self.dismiss(job)


def run_tui(config: PDLConfig | None = None) -> None:
    """Run the pdl TUI."""
    check_optional_dep("textual", "tui")
    app = PdlTuiApp(config=config)
    app.run()