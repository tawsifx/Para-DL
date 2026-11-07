"""System diagnostic and environment doctor for parallel-dl (paradl).

Inspects yt-dlp/yt-dlp-ejs versions, warns when yt-dlp is older than 30 days,
checks auth status for HF & GitHub, verifies macOS utilities (caffeinate, scutil,
pmset, security, codesign, spctl, xattr), and outputs exact remediation commands.
"""

from __future__ import annotations

import importlib.util
import re
import subprocess
from dataclasses import dataclass
from datetime import datetime

from pdl import VERSION
from pdl.auth import get_auth_status
from pdl.models import ExitCode
from pdl.platform import check_platform_utilities, is_windows
from pdl.toolchain import TOOLCHAIN
from pdl.ui import UI, print_plain

_YTDLP_VER_RE = re.compile(r"^(\d{4})\.(\d{2})\.(\d{2})")


@dataclass
class ToolCheck:
    name: str
    status: str  # "ready", "warning", "missing", "optional_missing"
    detail: str
    fix: str | None = None
    required: bool = True


def get_ytdlp_version_and_age() -> tuple[str | None, int | None, bool]:
    """Return (version_str, age_days, is_older_than_30_days)."""
    ytdlp_bin = TOOLCHAIN.ytdlp
    ver_str: str | None = None

    if ytdlp_bin:
        try:
            proc = subprocess.run(
                [ytdlp_bin, "--version"],
                capture_output=True,
                text=True,
                timeout=3.0,
                check=False,
            )
            if proc.returncode == 0 and proc.stdout.strip():
                ver_str = proc.stdout.strip().splitlines()[0]
        except (OSError, subprocess.SubprocessError):
            pass

    if not ver_str:
        # Fallback to python module if installed
        try:
            import yt_dlp.version

            ver_str = getattr(yt_dlp.version, "__version__", None)
        except ImportError:
            pass

    if not ver_str:
        return None, None, False

    m = _YTDLP_VER_RE.match(ver_str)
    if not m:
        return ver_str, None, False

    try:
        release_date = datetime(int(m.group(1)), int(m.group(2)), int(m.group(3)))
        age_days = (datetime.now() - release_date).days
        return ver_str, age_days, age_days > 30
    except (ValueError, OverflowError):
        return ver_str, None, False


def run_doctor() -> int:
    """Run pdl doctor diagnostic suite and report findings with exact fixes."""
    has_httpx = importlib.util.find_spec("httpx") is not None
    has_hf_xet = (
        importlib.util.find_spec("huggingface_hub") is not None and importlib.util.find_spec("hf_xet") is not None
    )

    ytdlp_ver, ytdlp_age, ytdlp_outdated = get_ytdlp_version_and_age()
    ejs_ver = TOOLCHAIN.yt_dlp_ejs_version
    js_rt = TOOLCHAIN.js_runtime
    auth_status = get_auth_status()
    plat_utils = check_platform_utilities()

    fixes: list[str] = []

    # 1. Core Tools
    core_items: list[ToolCheck] = []

    # httpx
    if has_httpx:
        core_items.append(ToolCheck("GitHub PowerSuite (httpx)", "ready", "ready"))
    else:
        fix = "pdl pip install httpx  # or: uv pip install httpx"
        fixes.append(fix)
        core_items.append(ToolCheck("GitHub PowerSuite (httpx)", "missing", "not found", fix=fix, required=True))

    # HF engine
    if has_hf_xet:
        core_items.append(ToolCheck("Hugging Face Hub Engine", "ready", "ready (hf_xet enabled)"))
    elif has_httpx:
        core_items.append(ToolCheck("Hugging Face Hub Engine", "ready", "ready (Aria2 RPC)"))
    else:
        core_items.append(ToolCheck("Hugging Face Hub Engine", "missing", "httpx required"))

    # aria2c
    if TOOLCHAIN.aria2c:
        core_items.append(ToolCheck("aria2c (JSON-RPC 2.0)", "ready", TOOLCHAIN.aria2c))
    else:
        fix = (
            "winget install aria2.aria2  # or: scoop install aria2"
            if is_windows()
            else "paradl brew install aria2  # or: brew install aria2"
        )
        fixes.append(fix)
        core_items.append(ToolCheck("aria2c (JSON-RPC 2.0)", "missing", "not found", fix=fix, required=True))

    # yt-dlp
    if TOOLCHAIN.ytdlp:
        if ytdlp_outdated and ytdlp_age is not None:
            fix = (
                "winget install yt-dlp.yt-dlp  # or: paradl pip install -U yt-dlp"
                if is_windows()
                else "paradl brew install yt-dlp  # or: paradl pip install -U yt-dlp / brew upgrade yt-dlp"
            )
            fixes.append(fix)
            core_items.append(
                ToolCheck(
                    "yt-dlp (Media Stream Resolver)",
                    "warning",
                    f"v{ytdlp_ver} ({ytdlp_age} days old — older than 30 days)",
                    fix=fix,
                    required=True,
                )
            )
        else:
            age_label = f" ({ytdlp_age}d old)" if ytdlp_age is not None else ""
            core_items.append(ToolCheck("yt-dlp (Media Stream Resolver)", "ready", f"v{ytdlp_ver}{age_label}"))
    else:
        fix = (
            "winget install yt-dlp.yt-dlp  # or: paradl pip install yt-dlp"
            if is_windows()
            else "paradl brew install yt-dlp  # or: paradl pip install yt-dlp / brew install yt-dlp"
        )
        fixes.append(fix)
        core_items.append(ToolCheck("yt-dlp (Media Stream Resolver)", "missing", "not found", fix=fix, required=True))

    # yt-dlp-ejs
    if ejs_ver:
        core_items.append(ToolCheck("yt-dlp-ejs (JS Challenge Pkg)", "ready", f"v{ejs_ver}"))
    else:
        fix = "paradl pip install yt-dlp-ejs  # or: npm install -g yt-dlp-ejs"
        fixes.append(fix)
        core_items.append(ToolCheck("yt-dlp-ejs (JS Challenge Pkg)", "missing", "not found", fix=fix, required=True))

    # JS runtime
    if js_rt:
        core_items.append(ToolCheck("JS Runtime (for EJS)", "ready", js_rt, required=False))
    else:
        fix = (
            "winget install OpenJS.NodeJS  # or: scoop install nodejs"
            if is_windows()
            else "paradl brew install node  # or: paradl brew install deno"
        )
        fixes.append(fix)
        core_items.append(ToolCheck("JS Runtime (for EJS)", "optional_missing", "not found", fix=fix, required=False))

    # ffmpeg
    if TOOLCHAIN.ffmpeg:
        core_items.append(ToolCheck("ffmpeg (AV Transcoder)", "ready", TOOLCHAIN.ffmpeg, required=False))
    else:
        fix = (
            "winget install Gyan.FFmpeg  # or: scoop install ffmpeg"
            if is_windows()
            else "paradl brew install ffmpeg  # or: brew install ffmpeg"
        )
        fixes.append(fix)
        core_items.append(ToolCheck("ffmpeg (AV Transcoder)", "optional_missing", "not found", fix=fix, required=False))

    # afconvert
    if TOOLCHAIN.afconvert:
        core_items.append(ToolCheck("afconvert (CoreAudio)", "ready", TOOLCHAIN.afconvert, required=False))

    # 2. Platform & Security Utilities
    plat_items: list[ToolCheck] = []
    platform_name = "Windows" if is_windows() else "macOS"
    for u_name, u_info in plat_utils.items():
        u_avail = u_info.get("available", False)
        u_path = u_info.get("path")
        u_fix = u_info.get("fix")
        u_req = u_info.get("required", True)
        if u_avail:
            plat_items.append(ToolCheck(f"{u_name} ({platform_name})", "ready", u_path or "built-in", required=u_req))
        else:
            if u_fix:
                fixes.append(u_fix)
            status_tag = "missing" if u_req else "optional_missing"
            plat_items.append(ToolCheck(f"{u_name} ({platform_name})", status_tag, "not found", fix=u_fix, required=u_req))

    # 3. Authentication Checks
    auth_items: list[ToolCheck] = []
    hf_info = auth_status.get("hf", {})
    if hf_info.get("configured"):
        auth_items.append(ToolCheck("Hugging Face Hub Token", "ready", f"configured ({hf_info['source']})"))
    else:
        fix = (
            'paradl auth set hf  # or: $env:HF_TOKEN="<token>"'
            if is_windows()
            else "paradl auth set hf  # or: export HF_TOKEN=<token>"
        )
        fixes.append(fix)
        auth_items.append(
            ToolCheck("Hugging Face Hub Token", "optional_missing", "not configured", fix=fix, required=False)
        )

    gh_info = auth_status.get("github", {})
    if gh_info.get("configured"):
        auth_items.append(ToolCheck("GitHub API Token", "ready", f"configured ({gh_info['source']})"))
    else:
        fix = (
            'paradl auth set github  # or: $env:GITHUB_TOKEN="<token>"'
            if is_windows()
            else "paradl auth set github  # or: export GITHUB_TOKEN=<token>"
        )
        fixes.append(fix)
        auth_items.append(ToolCheck("GitHub API Token", "optional_missing", "not configured", fix=fix, required=False))

    # JSONL event mode
    if UI.json_mode:
        payload = {
            "version": VERSION,
            "ytdlp": {
                "version": ytdlp_ver,
                "age_days": ytdlp_age,
                "outdated": ytdlp_outdated,
                "path": TOOLCHAIN.ytdlp,
            },
            "ytdlp_ejs": {"version": ejs_ver},
            "auth": auth_status,
            "platform_utilities": plat_utils,
            "macos_utilities": plat_utils,  # backward compatibility alias
            "core_tools": {c.name: {"status": c.status, "detail": c.detail, "fix": c.fix} for c in core_items},
            "fixes": list(dict.fromkeys(fixes)),
        }
        UI.emit_event("doctor", **payload)
        UI.emit_event("diagnose", **payload)  # backward compatibility alias
        return int(ExitCode.OK)

    # Human-readable Rich output
    from rich import box
    from rich.panel import Panel
    from rich.table import Table

    def _render_table(title: str, items: list[ToolCheck]) -> Table:
        t = Table(title=title, box=box.HORIZONTALS, pad_edge=False, expand=True)
        t.add_column("Component / Engine", style="bold white", ratio=3)
        t.add_column("Status", justify="center", ratio=1)
        t.add_column("Details", style="dim", ratio=4)
        for it in items:
            if it.status == "ready":
                st = "[bold green]ready[/bold green]"
            elif it.status == "warning":
                st = "[bold yellow]warning[/bold yellow]"
            elif it.status == "missing":
                st = "[bold red]missing[/bold red]"
            else:
                st = "[dim]optional[/dim]"
            t.add_row(it.name, st, it.detail)
        return t

    cred_store_name = "Windows Credential Manager" if is_windows() else "macOS Keychain"
    UI.print()
    UI.print_renderable(_render_table("Core Download & Media Engines", core_items))
    UI.print()
    UI.print_renderable(_render_table(f"{platform_name} Platform & Security Utilities", plat_items))
    UI.print()
    UI.print_renderable(_render_table(f"Authentication Status ({cred_store_name} / Env)", auth_items))
    UI.print()

    unique_fixes = list(dict.fromkeys(fixes))
    if unique_fixes:
        fix_lines = "\n".join(f"  • [cyan]{f}[/cyan]" for f in unique_fixes)
        panel = Panel(
            f"[bold yellow]Recommended Fix Commands:[/bold yellow]\n{fix_lines}",
            title="[bold red]Action Required / Recommended[/bold red]"
            if any(c.status == "missing" for c in core_items)
            else "[bold yellow]Recommendations[/bold yellow]",
            box=box.ROUNDED,
        )
        UI.print_renderable(panel)
        UI.print()
    else:
        print_plain(
            "  [bold green]✓ All systems optimal! No missing dependencies or security issues found.[/bold green]\n"
        )

    return int(ExitCode.OK)
