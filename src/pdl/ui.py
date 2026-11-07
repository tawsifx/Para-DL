"""Lazy-loaded Rich UI, secret redaction, non-interactive prompt guard, and JSONL event emitter."""

from __future__ import annotations

import json
import logging
import os
import re
import sys
import time
from typing import Any

from pdl.models import format_size, format_speed

logger = logging.getLogger(__name__)

_TAG_STRIP_RE = re.compile(r"\[/?[a-zA-Z0-9_ =#.,/-]+\]")
_BEARER_RE = re.compile(r"(Bearer\s+)[A-Za-z0-9._\-+/=]+", re.IGNORECASE)
_TOKEN_PARAM_RE = re.compile(r"(token[:=]\s*)[A-Za-z0-9._\-+/=]+", re.IGNORECASE)


def redact_secrets(text: str, extra_secrets: list[str] | None = None) -> str:
    """Redact tokens, Bearer headers, and explicit secrets from strings before logging or output."""
    if not text:
        return text
    out = _BEARER_RE.sub(r"\1***REDACTED***", text)
    out = _TOKEN_PARAM_RE.sub(r"\1***REDACTED***", out)
    env_keys = ("HF_TOKEN", "HUGGING_FACE_HUB_TOKEN", "GITHUB_TOKEN", "GH_TOKEN")
    for k in env_keys:
        val = os.environ.get(k)
        if val and len(val) >= 4 and val in out:
            out = out.replace(val, "***REDACTED***")
    for sec in extra_secrets or []:
        if sec and len(sec) >= 4 and sec in out:
            out = out.replace(sec, "***REDACTED***")
    return out


def _redact_obj(obj: Any, extra_secrets: list[str] | None = None) -> Any:
    if isinstance(obj, str):
        return redact_secrets(obj, extra_secrets)
    if isinstance(obj, dict):
        result: dict[str, Any] = {}
        for k, v in obj.items():
            if isinstance(k, str) and any(s in k.lower() for s in ("secret", "token", "authorization", "password")):
                result[k] = "***REDACTED***"
            else:
                result[k] = _redact_obj(v, extra_secrets)
        return result
    if isinstance(obj, (list, tuple)):
        return [_redact_obj(x, extra_secrets) for x in obj]
    return obj


class SecretRedactingFilter(logging.Filter):
    """Logging filter ensuring no secrets appear in log records."""

    def filter(self, record: logging.LogRecord) -> bool:
        if isinstance(record.msg, str):
            record.msg = redact_secrets(record.msg)
        if record.args:
            if isinstance(record.args, dict):
                record.args = {k: redact_secrets(v) if isinstance(v, str) else v for k, v in record.args.items()}
            elif isinstance(record.args, tuple):
                record.args = tuple(redact_secrets(a) if isinstance(a, str) else a for a in record.args)
        return True


class UIState:
    """Global UI configuration and lazy Rich console holder."""

    def __init__(self) -> None:
        self.quiet: bool = False
        self.json_mode: bool = False
        self.non_interactive: bool = False
        self._console: Any = None
        self._console_initialized: bool = False
        self._registered_secrets: list[str] = []

    def register_secret(self, secret: str) -> None:
        if secret and secret not in self._registered_secrets:
            self._registered_secrets.append(secret)

    def get_console(self) -> Any:
        if self.json_mode:
            return None
        if not self._console_initialized:
            self._console_initialized = True
            try:
                from rich.console import Console
                from rich.theme import Theme

                custom_theme = Theme(
                    {
                        "info": "bright_cyan",
                        "warning": "yellow",
                        "error": "bold red",
                        "success": "green",
                        "accent": "cyan",
                        "dimmed": "dim",
                        "highlight": "bold white",
                        "muted": "dim white",
                        "path": "bright_blue",
                        "size": "dim cyan",
                    }
                )
                self._console = Console(theme=custom_theme)
            except ImportError:
                self._console = None
        return self._console

    def print(self, msg: str = "", *, force: bool = False, **kwargs: Any) -> None:
        """Print formatted text unless suppressed by --json or --quiet."""
        if self.json_mode:
            return
        if self.quiet and not force:
            return
        safe_msg = redact_secrets(msg, self._registered_secrets)
        con = self.get_console()
        if con is not None:
            con.print(safe_msg, **kwargs)
        else:
            clean = _TAG_STRIP_RE.sub("", safe_msg)
            print(clean)

    def print_renderable(self, renderable: Any, *, force: bool = False) -> None:
        if self.json_mode:
            return
        if self.quiet and not force:
            return
        con = self.get_console()
        if con is not None:
            con.print(renderable)

    def emit_event(self, event_type: str, **payload: Any) -> None:
        """Emit a JSONL event to stdout when --json is active."""
        if not self.json_mode:
            return
        record: dict[str, Any] = {
            "event": event_type,
            "timestamp": round(time.time(), 3),
        }
        record.update(_redact_obj(payload, self._registered_secrets))
        sys.stdout.write(json.dumps(record, default=str) + "\n")
        sys.stdout.flush()

    def can_prompt(self, select_requested: bool = False) -> bool:
        """Prompts are allowed ONLY when --select is explicitly passed on an interactive TTY
        and neither --non-interactive, --json, nor --quiet is active.
        """
        if not select_requested:
            return False
        if self.non_interactive or self.json_mode or self.quiet:
            return False
        return bool(sys.stdin.isatty() and sys.stdout.isatty())

    def ask_choice(self, prompt_text: str, choices: list[str], default: str = "1") -> str:
        """Prompt user for a choice if allowed, otherwise return default immediately."""
        if not self.can_prompt(select_requested=True):
            return default
        try:
            from rich.prompt import Prompt

            return str(Prompt.ask(prompt_text, choices=choices, default=default))
        except (ImportError, EOFError, KeyboardInterrupt):
            return default


UI = UIState()


def print_plain(msg: str, **kwargs: Any) -> None:
    """Backwards-compatible helper around UI.print."""
    UI.print(msg, **kwargs)


class LiveTransferDisplay:
    """Rich Live progress and watchdog status display for Aria2 RPC and Media transfers."""

    def __init__(self, title: str, total_bytes: int | None = None) -> None:
        self.title = title
        self.total_bytes = total_bytes or 0
        self.downloaded_bytes = 0
        self.speed_bps = 0.0
        self.connections = 0
        self.watchdog_state = "Active"
        self._live: Any = None
        self._last_json_emit = 0.0

    def __enter__(self) -> LiveTransferDisplay:
        if not UI.json_mode and not UI.quiet:
            con = UI.get_console()
            if con is not None and con.is_terminal:
                try:
                    from rich.live import Live

                    self._live = Live(
                        self._build_renderable(),
                        console=con,
                        refresh_per_second=4,
                        transient=True,
                    )
                    self._live.start(refresh=True)
                except ImportError:
                    self._live = None
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        if self._live is not None:
            try:
                self._live.stop()
            except Exception as exc:
                logger.debug("Live display exit error: %s", exc)
            self._live = None

    def update(
        self,
        *,
        downloaded_bytes: int | None = None,
        total_bytes: int | None = None,
        speed_bps: float | None = None,
        connections: int | None = None,
        watchdog_state: str | None = None,
        force_event: bool = False,
    ) -> None:
        if downloaded_bytes is not None:
            self.downloaded_bytes = downloaded_bytes
        if total_bytes is not None and total_bytes > 0:
            self.total_bytes = total_bytes
        if speed_bps is not None:
            self.speed_bps = speed_bps
        if connections is not None:
            self.connections = connections
        if watchdog_state is not None:
            self.watchdog_state = watchdog_state

        pct = (self.downloaded_bytes * 100.0 / self.total_bytes) if self.total_bytes > 0 else 0.0
        rem = max(self.total_bytes - self.downloaded_bytes, 0)
        eta_sec = (rem / self.speed_bps) if (self.speed_bps > 0 and self.total_bytes > 0) else None

        now = time.time()
        if UI.json_mode and (force_event or (now - self._last_json_emit >= 0.8)):
            self._last_json_emit = now
            UI.emit_event(
                "progress",
                title=self.title,
                downloaded_bytes=self.downloaded_bytes,
                total_bytes=self.total_bytes,
                percent=round(pct, 2),
                speed_bps=round(self.speed_bps, 2),
                eta_seconds=round(eta_sec, 1) if eta_sec is not None else None,
                connections=self.connections,
                watchdog_state=self.watchdog_state,
            )

        if self._live is not None:
            self._live.update(self._build_renderable())

    def _build_renderable(self) -> Any:
        from rich.text import Text

        pct = (self.downloaded_bytes * 100.0 / self.total_bytes) if self.total_bytes > 0 else 0.0
        rem = max(self.total_bytes - self.downloaded_bytes, 0)
        if self.speed_bps > 0 and self.total_bytes > 0:
            eta_s = int(rem / self.speed_bps)
            eta_str = f"{eta_s // 60:02d}:{eta_s % 60:02d}"
        else:
            eta_str = "--:--"

        bar_width = 24
        filled = int(min(max(pct / 100.0, 0.0), 1.0) * bar_width)
        bar = "█" * filled + "░" * (bar_width - filled)

        size_part = (
            f"{format_size(self.downloaded_bytes)} / {format_size(self.total_bytes)}"
            if self.total_bytes > 0
            else format_size(self.downloaded_bytes)
        )

        txt = Text()
        txt.append("  ", style="dim")
        txt.append(f"{self.title[:32]} ", style="bold white")
        txt.append(f"[{bar}] ", style="cyan")
        txt.append(f"{pct:5.1f}%  ", style="bold bright_cyan")
        txt.append(f"{size_part}  ", style="dim white")
        txt.append(f"{format_speed(self.speed_bps)}  ", style="green")
        txt.append(f"ETA {eta_str}  ", style="dim")
        if self.connections > 1:
            txt.append(f"({self.connections} conns)  ", style="cyan")
        if self.watchdog_state and self.watchdog_state.lower() not in ("active", "none", ""):
            txt.append(f"[{self.watchdog_state}]", style="yellow")
        return txt
