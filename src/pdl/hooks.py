"""Completion and failure hook runner with 30s timeout isolation and built-in --notify action.

Executables in $PDL_HOME/hooks/on_complete and $PDL_HOME/hooks/on_fail receive
result JSON on stdin with a strict 30 s timeout. Hooks never block or crash pdl.
Native macOS notifications (--notify) are implemented as built-in actions on this runner.
"""

from __future__ import annotations

import json
import logging
import os
import subprocess
from pathlib import Path
from typing import Any

from pdl.models import format_size
from pdl.paths import get_pdl_home
from pdl.platform import macos
from pdl.ui import UI, print_plain

logger = logging.getLogger(__name__)

DEFAULT_HOOK_TIMEOUT_SEC: float = 30.0


class HookRunner:
    """Discovers and safely executes user hooks in $PDL_HOME/hooks/ with strict timeouts."""

    @classmethod
    def get_hooks_dir(cls) -> Path:
        return get_pdl_home() / "hooks"

    @classmethod
    def find_executables(cls, hook_type: str) -> list[Path]:
        """Find executable hooks for given hook_type ('on_complete' or 'on_fail')."""
        target = cls.get_hooks_dir() / hook_type
        if not target.exists():
            return []

        if target.is_file() and os.access(target, os.X_OK):
            return [target]

        executables: list[Path] = []
        if target.is_dir():
            for item in sorted(target.iterdir()):
                if item.is_file() and not item.name.startswith(".") and os.access(item, os.X_OK):
                    executables.append(item)
        return executables

    @classmethod
    def execute_hook(
        cls,
        hook_path: Path,
        payload_str: str,
        timeout: float = DEFAULT_HOOK_TIMEOUT_SEC,
    ) -> bool:
        """Run an external hook binary with JSON on stdin and a strict timeout. Never raises."""
        try:
            proc = subprocess.Popen(
                [str(hook_path)],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            try:
                out, err = proc.communicate(input=payload_str, timeout=timeout)
                if proc.returncode != 0:
                    logger.warning("Hook %s exited with non-zero code %s: %s", hook_path.name, proc.returncode, err)
                    UI.emit_event(
                        "hook_failed",
                        hook=str(hook_path),
                        exit_code=proc.returncode,
                        stderr=err.strip() if err else None,
                    )
                    return False
                UI.emit_event("hook_executed", hook=str(hook_path), success=True)
                return True
            except subprocess.TimeoutExpired:
                logger.warning("Hook %s timed out after %s seconds; terminating.", hook_path.name, timeout)
                try:
                    proc.kill()
                    proc.wait(timeout=1.0)
                except Exception:
                    pass
                print_plain(
                    f"  [warning]⚠ Hook '{hook_path.name}' timed out after {timeout}s (isolated, did not block pdl).[/warning]",
                    force=True,
                )
                UI.emit_event("hook_timeout", hook=str(hook_path), timeout_seconds=timeout)
                return False
        except Exception as exc:
            logger.warning("Failed to spawn hook %s: %s", hook_path, exc)
            UI.emit_event("hook_error", hook=str(hook_path), error=str(exc))
            return False

    @classmethod
    def run_result_actions(
        cls,
        result_payload: dict[str, Any],
        *,
        is_success: bool,
        notify: bool = False,
        timeout: float = DEFAULT_HOOK_TIMEOUT_SEC,
    ) -> None:
        """Execute built-in --notify and discovered user hooks for transfer outcome."""
        target_name = str(result_payload.get("target") or "download")
        file_path_str = result_payload.get("file_path")
        file_name = Path(file_path_str).name if file_path_str else target_name
        bytes_dl = int(result_payload.get("bytes_downloaded") or 0)
        error_msg = str(result_payload.get("error") or "Unknown error")

        # Built-in notification action
        if notify:
            if is_success:
                size_str = format_size(bytes_dl) if bytes_dl > 0 else ""
                msg = f"{file_name} ({size_str})" if size_str else file_name
                macos.display_notification(title="pdl - Download Complete", message=msg)
            else:
                macos.display_notification(
                    title="pdl - Download Failed",
                    message=f"{file_name}: {error_msg}",
                )

        # External hooks
        hook_type = "on_complete" if is_success else "on_fail"
        hooks = cls.find_executables(hook_type)
        if not hooks:
            return

        payload_json = json.dumps(result_payload)
        for h in hooks:
            cls.execute_hook(h, payload_json, timeout=timeout)

    @classmethod
    def notify_watchdog_giveup(cls, host: str, reason: str, enabled: bool = True) -> None:
        """Built-in notification action when Watchdog v2 exhausts mitigation tiers."""
        if not enabled:
            return
        macos.display_notification(
            title="pdl - Watchdog Alert",
            message=f"Watchdog gave up on {host}: {reason}",
        )

    @classmethod
    def notify_queue_summary(
        cls,
        total: int,
        succeeded: int,
        failed: int,
        enabled: bool = True,
    ) -> None:
        """Built-in notification action when foreground queue run finishes."""
        if not enabled:
            return
        msg = f"Processed {total} job(s): {succeeded} succeeded, {failed} failed"
        macos.display_notification(title="pdl - Queue Complete", message=msg)
