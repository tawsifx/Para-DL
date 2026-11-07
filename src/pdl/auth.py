"""Authentication management for Hugging Face and GitHub tokens (macOS Keychain backed).

Never exposes secrets in process argv, command logs, or JSONL events.
Strict lookup order: Environment Variable -> macOS Keychain -> Cache/None.
"""

from __future__ import annotations

import getpass
import logging
import os
import sys
from pathlib import Path
from typing import Any

from pdl.models import ExitCode
from pdl.platform import (
    delete_keychain_token,
    has_keychain_token,
    is_windows,
    set_keychain_token,
)
from pdl.toolchain import TOOLCHAIN
from pdl.ui import UI, print_plain

logger = logging.getLogger(__name__)

SUPPORTED_SERVICES: tuple[str, ...] = ("hf", "github")


def get_auth_status() -> dict[str, dict[str, Any]]:
    """Return dictionary of configured authentication status without exposing secret tokens."""
    status: dict[str, dict[str, Any]] = {}

    # Hugging Face
    keychain_source_name = "credential_manager" if is_windows() else "keychain"
    hf_source = "none"
    if os.environ.get("HF_TOKEN") or os.environ.get("HUGGING_FACE_HUB_TOKEN"):
        hf_source = "env"
    elif has_keychain_token("hf"):
        hf_source = keychain_source_name
    elif (Path.home() / ".cache" / "huggingface" / "token").exists():
        hf_source = "cache_file"

    status["hf"] = {
        "service": "hf",
        "label": "Hugging Face Hub",
        "configured": hf_source != "none",
        "source": hf_source,
    }

    # GitHub
    gh_source = "none"
    if os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN"):
        gh_source = "env"
    elif has_keychain_token("github"):
        gh_source = keychain_source_name
    elif TOOLCHAIN.gh:
        gh_source = "gh_cli"

    status["github"] = {
        "service": "github",
        "label": "GitHub API",
        "configured": gh_source != "none",
        "source": gh_source,
    }

    return status


def set_auth_token(service: str, token: str | None = None) -> bool:
    """Prompt for and store service token in macOS Login Keychain via security -i (no argv leak)."""
    srv = service.lower().strip()
    if srv not in SUPPORTED_SERVICES:
        print_plain(
            f"  [error]Unknown service '{service}'. Supported services: {', '.join(SUPPORTED_SERVICES)}[/error]",
            force=True,
        )
        return False

    tok = token
    if tok is None:
        try:
            if sys.stdin.isatty():
                tok = getpass.getpass(f"Enter token for {srv}: ")
            else:
                tok = sys.stdin.readline().strip()
        except (EOFError, KeyboardInterrupt):
            print_plain("\n  [warning]Cancelled token input.[/warning]", force=True)
            return False

    if not tok:
        print_plain("  [error]Token cannot be empty.[/error]", force=True)
        return False

    # Mask token from logs and output immediately
    UI.register_secret(tok)

    ok = set_keychain_token(account=srv, token=tok, service="pdl")
    store_name = "Windows Credential Store" if is_windows() else "macOS Login Keychain"
    if ok:
        print_plain(f"  [green]✓ Successfully saved {srv} token in {store_name} (service: pdl).[/green]")
        return True

    print_plain(
        f"  [error]✗ Failed to save {srv} token in {store_name}.[/error]",
        force=True,
    )
    return False


def remove_auth_token(service: str) -> bool:
    """Remove service token from platform credential store."""
    srv = service.lower().strip()
    if srv not in SUPPORTED_SERVICES:
        print_plain(
            f"  [error]Unknown service '{service}'. Supported services: {', '.join(SUPPORTED_SERVICES)}[/error]",
            force=True,
        )
        return False

    ok = delete_keychain_token(account=srv, service="pdl")
    store_name = "Windows Credential Store" if is_windows() else "macOS Login Keychain"
    if ok:
        print_plain(f"  [green]✓ Removed {srv} token from {store_name}.[/green]")
        return True

    print_plain(
        f"  [yellow]• No {srv} token found in {store_name} or already removed.[/yellow]",
        force=True,
    )
    return True


def handle_auth_command(
    action: str | None,
    service: str | None = None,
    token_input: str | None = None,
) -> int:
    """Execute 'pdl auth set|status|remove' subcommands."""
    act = (action or "status").lower().strip()

    if act == "status":
        status = get_auth_status()
        if UI.json_mode:
            UI.emit_event("auth_status", services=status)
            return int(ExitCode.OK)

        from rich import box
        from rich.table import Table

        table = Table(title="Authentication Status", box=box.HORIZONTALS, pad_edge=False)
        table.add_column("Service", style="bold white")
        table.add_column("Status", style="bold")
        table.add_column("Source", style="dim")

        for _srv_name, srv_info in status.items():
            if srv_info["configured"]:
                st = "[green]configured[/green]"
                src = f"{srv_info['source']}"
            else:
                st = "[dim]not configured[/dim]"
                src = "none"
            table.add_row(srv_info["label"], st, src)

        UI.print()
        UI.print_renderable(table)
        UI.print()
        return int(ExitCode.OK)

    if act == "set":
        if not service:
            print_plain(
                f"  [error]Usage: pdl auth set <{'|'.join(SUPPORTED_SERVICES)}>[/error]",
                force=True,
            )
            return int(ExitCode.USAGE)
        srv = service.lower().strip()
        if srv not in SUPPORTED_SERVICES:
            print_plain(
                f"  [error]Invalid service '{service}'. Must be one of: {', '.join(SUPPORTED_SERVICES)}[/error]",
                force=True,
            )
            return int(ExitCode.USAGE)
        ok = set_auth_token(srv, token_input)
        if UI.json_mode:
            UI.emit_event("auth_set", service=srv, success=ok)
        return int(ExitCode.OK if ok else ExitCode.FAILURE)

    if act == "remove":
        if not service:
            print_plain(
                f"  [error]Usage: pdl auth remove <{'|'.join(SUPPORTED_SERVICES)}>[/error]",
                force=True,
            )
            return int(ExitCode.USAGE)
        srv = service.lower().strip()
        if srv not in SUPPORTED_SERVICES:
            print_plain(
                f"  [error]Invalid service '{service}'. Must be one of: {', '.join(SUPPORTED_SERVICES)}[/error]",
                force=True,
            )
            return int(ExitCode.USAGE)
        ok = remove_auth_token(srv)
        if UI.json_mode:
            UI.emit_event("auth_remove", service=srv, success=ok)
        return int(ExitCode.OK if ok else ExitCode.FAILURE)

    print_plain(
        f"  [error]Unknown auth action '{action}'. Use: pdl auth set|status|remove[/error]",
        force=True,
    )
    return int(ExitCode.USAGE)
