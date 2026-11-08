"""Main application orchestrator for pdl subcommands, engines, and post-processing."""

from __future__ import annotations

import argparse
import dataclasses
import logging
import shutil
import sys
import time
from pathlib import Path
from typing import Any

from pdl import VERSION
from pdl.config import PDLConfig
from pdl.history import HistoryManager
from pdl.models import (
    ExitCode,
    TargetInfo,
    TargetType,
    WatchdogMode,
    format_size,
    format_speed,
    parse_rate,
)
from pdl.ui import UI, print_plain

logger = logging.getLogger(__name__)


class TargetInspector:
    """Inspects target metadata and classification without downloading."""

    @classmethod
    def inspect(
        cls,
        target: str,
        *,
        ejs_remote: bool = False,
        player_client: str | None = None,
    ) -> int:
        from pdl.engines.github import GitHubEngine
        from pdl.engines.hf import HuggingFaceEngine
        from pdl.engines.media import MediaEngine
        from pdl.net import probe_target

        t_info = probe_target(target)
        UI.emit_event(
            "target_classified",
            target=target,
            target_type=t_info.target_type.value,
            clean_url=t_info.clean_url,
            filename_hint=t_info.filename_hint,
            content_length=t_info.content_length,
            accept_ranges=t_info.accept_ranges,
        )

        props: dict[str, Any] = {
            "Classification": t_info.target_type.value.upper(),
            "Resolved URL": t_info.clean_url,
        }

        if t_info.target_type == TargetType.GITHUB and t_info.github_meta:
            meta = t_info.github_meta
            owner = meta.get("owner", "")
            repo = meta.get("repo", "")
            props["GitHub Repo"] = f"{owner}/{repo}"
            props["Target Kind"] = meta.get("type", "repo")
            rel = GitHubEngine.fetch_release_info(owner, repo)
            if rel:
                props["Latest Tag"] = rel.get("tag_name", "-")
                props["Published At"] = str(rel.get("published_at", "-"))[:10]
                props["Asset Count"] = str(len(rel.get("assets", [])))

        elif t_info.target_type == TargetType.HF and t_info.hf_meta:
            meta = t_info.hf_meta
            repo_id = meta.get("repo_id", "")
            props["HF Model ID"] = repo_id
            info = HuggingFaceEngine.fetch_hf_info(repo_id, meta.get("repo_type", "model"))
            if info:
                props["Pipeline Type"] = info.get("pipeline_tag", "-")
                props["Downloads"] = str(info.get("downloads", "-"))
                props["Files Count"] = str(len(info.get("siblings", [])))

        elif t_info.target_type in (TargetType.STREAM, TargetType.SEARCH):
            m_info = MediaEngine.extract_metadata(
                t_info.clean_url,
                ejs_remote=ejs_remote,
                player_client=player_client,
            )
            if m_info:
                props["Media Title"] = m_info.get("title", "-")
                props["Uploader"] = m_info.get("uploader", "-")
                props["Duration"] = f"{m_info.get('duration', 0)}s"
                props["View Count"] = str(m_info.get("view_count", "-"))

        elif t_info.target_type == TargetType.IPSW and t_info.ipsw_meta:
            meta = t_info.ipsw_meta
            props["Apple Device"] = meta.get("device_name", "-")
            props["Model Identifier"] = meta.get("identifier", "-")
            props["iOS Version"] = meta.get("version", "-")
            props["Build ID"] = meta.get("buildid", "-")
            if meta.get("filesize"):
                props["File Size"] = format_size(meta["filesize"])
            if meta.get("sha256"):
                props["SHA-256 Hash"] = meta["sha256"]

        elif t_info.target_type == TargetType.PYPI and t_info.pypi_meta:
            meta = t_info.pypi_meta
            props["Python Package"] = meta.get("package", "-")
            props["Version"] = meta.get("version", "-")
            props["Artifact Type"] = meta.get("packagetype", "-")
            if meta.get("filename"):
                props["Filename Hint"] = meta["filename"]
            if meta.get("filesize"):
                props["File Size"] = format_size(meta["filesize"])
            if meta.get("sha256"):
                props["SHA-256 Hash"] = meta["sha256"]

        elif t_info.target_type == TargetType.BREW and t_info.brew_meta:
            meta = t_info.brew_meta
            props["Homebrew Formula"] = meta.get("formula", "-")
            props["Version"] = meta.get("version", "-")
            props["Bottle Tag"] = meta.get("tag", "-")
            if meta.get("filename"):
                props["Bottle File"] = meta["filename"]
            if meta.get("filesize"):
                props["File Size"] = format_size(meta["filesize"])
            if meta.get("sha256"):
                props["SHA-256 Hash"] = meta["sha256"]

        elif t_info.target_type == TargetType.DIRECT:
            if t_info.filename_hint:
                props["Filename Hint"] = t_info.filename_hint
            if t_info.content_length:
                props["Content Length"] = format_size(t_info.content_length)
            if t_info.mime_type:
                props["MIME Type"] = t_info.mime_type
            if t_info.accept_ranges is not None:
                props["Accept-Ranges"] = "206 Partial Content" if t_info.accept_ranges else "200 Only"
            if t_info.etag_sha256:
                props["ETag SHA-256"] = t_info.etag_sha256

        if UI.json_mode:
            UI.emit_event("info", target=target, properties=props)
            return int(ExitCode.OK)

        from rich import box
        from rich.table import Table

        table = Table(title=f"Target Inspector: {target}", box=box.HORIZONTALS, pad_edge=False)
        table.add_column("Property", style="bold cyan")
        table.add_column("Value", style="white")
        for k, v in props.items():
            table.add_row(str(k), str(v))

        UI.print()
        UI.print_renderable(table)
        UI.print()
        return int(ExitCode.OK)


class PDLApp:
    """Main application controller for pdl."""

    def __init__(self, config: PDLConfig) -> None:
        self.config = config

    def render_banner(self) -> None:
        if UI.json_mode or UI.quiet:
            return
        con = UI.get_console()
        if con is None:
            return
        from rich.text import Text

        header = Text()
        header.append("pdl ", style="bold bright_cyan")
        header.append(f"v{VERSION}", style="dim")
        header.append("  •  parallel multi-channel accelerator", style="dim white")
        con.print()
        con.print(header)
        con.print(Text("─" * 48, style="dim"))

    def print_diagnostics(self) -> int:
        from pdl.doctor import run_doctor

        return run_doctor()

    def clean_orphans(self, target_dir: Path) -> int:
        target_dir = Path(target_dir).expanduser().resolve()
        patterns = ["*.aria2", ".pdl_raw_*", "*.part", "*.ytdl"]
        found: list[Path] = []
        for pat in patterns:
            found.extend(list(target_dir.glob(pat)))

        if not found:
            print_plain(f"  [green]✓ Clean![/green] [dim]No temporary or orphaned download files in {target_dir}[/dim]")
            UI.emit_event("clean", directory=str(target_dir), removed=0)
            return int(ExitCode.OK)

        removed = 0
        for f in found:
            try:
                if f.is_dir():
                    shutil.rmtree(f, ignore_errors=True)
                else:
                    f.unlink()
                removed += 1
            except OSError as exc:
                logger.debug("Failed to remove orphan %s: %s", f, exc)
        print_plain(f"  [green]✓ Cleaned {removed} orphaned file(s) in {target_dir}[/green]")
        UI.emit_event("clean", directory=str(target_dir), removed=removed)
        return int(ExitCode.OK)

    def handle_config_command(
        self,
        action: str | None,
        key: str | None,
        value: str | None,
    ) -> int:
        act = (action or "show").lower()
        if act == "show":
            cfg_dict = dataclasses.asdict(self.config)
            if UI.json_mode:
                UI.emit_event("config", config=cfg_dict)
                return int(ExitCode.OK)

            if not UI.quiet:
                rows = [(str(k), str(v)) for k, v in cfg_dict.items()]
                col1_w = max(len("Setting"), *(len(r[0]) for r in rows))
                col2_w = max(len("Value"), *(len(r[1]) for r in rows))
                sep = "─" * (col1_w + col2_w + 4)
                out_lines = [
                    "",
                    "  pdl Configuration",
                    f"  {sep}",
                    f"  {'Setting':<{col1_w}}    {'Value':<{col2_w}}",
                    f"  {sep}",
                ]
                for k_s, v_s in rows:
                    out_lines.append(f"  {k_s:<{col1_w}}    {v_s}")
                out_lines.append(f"  {sep}\n")
                sys.stdout.write("\n".join(out_lines))
                sys.stdout.flush()
            return int(ExitCode.OK)

        if act == "set":
            if not key or value is None:
                sys.stderr.write("  Error: Usage: pdl config set <key> <value>\n")
                return int(ExitCode.USAGE)
            ok, msg = self.config.set(key, value)
            if ok:
                if not UI.json_mode and not UI.quiet:
                    sys.stdout.write(f"  ✓ {msg}\n")
                UI.emit_event("config_updated", key=key, value=getattr(self.config, key))
                return int(ExitCode.OK)
            if not UI.json_mode:
                sys.stderr.write(f"  ✗ {msg}\n")
            UI.emit_event("error", message=msg)
            return int(ExitCode.USAGE)

        if act == "reset":
            self.config = PDLConfig()
            self.config.save()
            if not UI.json_mode and not UI.quiet:
                sys.stdout.write("  ✓ Configuration reset to defaults.\n")
            UI.emit_event("config_reset", config=dataclasses.asdict(self.config))
            return int(ExitCode.OK)

        if act == "reset-hosts":
            cleared = HistoryManager.reset_hosts()
            if not UI.json_mode and not UI.quiet:
                sys.stdout.write(f"  ✓ Cleared {cleared} saved per-host watchdog strategy profile(s).\n")
            UI.emit_event("hosts_reset", cleared=cleared)
            return int(ExitCode.OK)

        if not UI.json_mode:
            sys.stderr.write(
                f"  Error: Unknown config action: {action!r} (expected show, set, reset, or reset-hosts)\n"
            )
        return int(ExitCode.USAGE)

    def handle_profiles_command(
        self,
        action: str | None = "list",
        host: str | None = None,
    ) -> int:
        act = (action or "list").strip().lower()
        if act in ("list", "show"):
            HistoryManager.display_profiles(host=host)
            return int(ExitCode.OK)

        if act == "reset":
            target_h = None if (host is None or host.strip().lower() in ("all", "--all", "*")) else host.strip().lower()
            cleared = HistoryManager.reset_profiles(host=target_h)
            if not UI.json_mode and not UI.quiet:
                scope = f"for '{target_h}'" if target_h else "(all hosts)"
                sys.stdout.write(f"  ✓ Cleared {cleared} learned profile(s) {scope}.\n")
            UI.emit_event("profiles_reset", host=target_h, cleared=cleared)
            return int(ExitCode.OK)

        # If a hostname was passed directly as `pdl profiles <host>`, list profiles for that host
        HistoryManager.display_profiles(host=act)
        return int(ExitCode.OK)

    def _resolve_when_fast_bps(self, args: argparse.Namespace) -> float:
        wf_raw = getattr(args, "when_fast", None)
        if wf_raw is None:
            return 0.0
        if wf_raw == "DEFAULT":
            fallback_rate = getattr(args, "anti_throttle", None) or self.config.anti_throttle or "4MBps"
            return parse_rate(fallback_rate)
        return parse_rate(str(wf_raw))

    def handle_queue_command(
        self,
        action: str | None,
        q_args: list[str],
        args: argparse.Namespace,
    ) -> int:
        from pdl.net import probe_target
        from pdl.queue import (
            QueueManager,
            format_start_at_time,
            generate_launchd_plist,
        )

        act = (action or "list").strip().lower()

        if act in ("list", "show"):
            QueueManager.display_queue()
            return int(ExitCode.OK)

        if act == "add":
            if not q_args:
                print_plain("  [error]Usage: pdl queue add <target>... [options][/error]", force=True)
                return int(ExitCode.USAGE)
            try:
                wf_bps = self._resolve_when_fast_bps(args)
                at_norm = format_start_at_time(getattr(args, "start_at", None))
            except ValueError as exc:
                print_plain(f"  [error]{exc}[/error]", force=True)
                UI.emit_event("error", message=str(exc))
                return int(ExitCode.USAGE)

            out_dir = Path(getattr(args, "output", None) or self.config.output_dir).expanduser().resolve()
            raw_conn = getattr(args, "connections", None) or self.config.connections
            connections = min(max(int(raw_conn), 1), 16)
            priority = int(getattr(args, "priority", 0) or 0)
            req_ac_arg = getattr(args, "require_ac", None)
            # B2: Queued jobs default to --require-ac ON unless explicitly overridden with --no-require-ac
            require_ac = True if req_ac_arg is None else bool(req_ac_arg)
            pause_on_low_power = bool(getattr(args, "pause_on_low_power", self.config.pause_on_low_power))
            cli_mirrors = list(getattr(args, "mirrors", []) or [])

            opts = {
                "limit_rate": getattr(args, "limit_rate", None) or self.config.max_download_limit,
                "anti_throttle": getattr(args, "anti_throttle", self.config.anti_throttle),
                "grace_period": int(getattr(args, "anti_throttle_grace", self.config.anti_throttle_grace) or 5),
                "watchdog_mode": str(getattr(args, "watchdog", None) or self.config.watchdog or "v2").lower(),
                "caffeinate": bool(getattr(args, "caffeinate", self.config.caffeinate)),
                "net_watch": bool(getattr(args, "net_watch", self.config.net_watch)),
                "stall_timeout": float(getattr(args, "stall_timeout", self.config.stall_timeout)),
                "checksum": getattr(args, "checksum", None),
            }

            added_jobs = []
            for t_str in q_args:
                t_info = probe_target(t_str, allow_network_probe=False)
                job = QueueManager.add_job(
                    target=t_str,
                    out_dir=out_dir,
                    target_type=t_info.target_type.value,
                    filename=t_info.filename_hint,
                    mirrors=cli_mirrors,
                    priority=priority,
                    connections=connections,
                    require_ac=require_ac,
                    pause_on_low_power=pause_on_low_power,
                    when_fast_bps=wf_bps,
                    start_at=at_norm,
                    expected_length=t_info.content_length,
                    expected_digest=getattr(args, "checksum", None) or t_info.expected_digest,
                    options=opts,
                )
                added_jobs.append(job)
                if not UI.json_mode and not UI.quiet:
                    print_plain(
                        f"  [green]✓ Queued Job #{job.id}:[/green] [bold white]{job.target}[/bold white] "
                        f"[dim](priority={job.priority}, require_ac={job.require_ac})[/dim]"
                    )
                UI.emit_event(
                    "queue_job_added",
                    job_id=job.id,
                    target=job.target,
                    priority=job.priority,
                    require_ac=job.require_ac,
                    when_fast_bps=job.when_fast_bps,
                    start_at=job.start_at,
                )
            return int(ExitCode.OK)

        if act in ("remove", "rm", "delete"):
            remove_all = bool(getattr(args, "all_flag", False)) or ("all" in [x.lower() for x in q_args])
            if not remove_all and not q_args:
                print_plain("  [error]Usage: pdl queue remove <id>... (or --all)[/error]", force=True)
                return int(ExitCode.USAGE)
            try:
                ids = [] if remove_all else [int(x) for x in q_args]
            except ValueError:
                print_plain(f"  [error]Invalid queue job ID(s): {q_args!r}[/error]", force=True)
                return int(ExitCode.USAGE)
            removed = QueueManager.remove_jobs(ids, remove_all=remove_all)
            if not UI.json_mode and not UI.quiet:
                print_plain(f"  [green]✓ Removed {removed} job(s) from queue.[/green]")
            UI.emit_event("queue_removed", removed=removed)
            return int(ExitCode.OK)

        if act == "pause":
            pause_all = bool(getattr(args, "all_flag", False)) or ("all" in [x.lower() for x in q_args])
            if not pause_all and not q_args:
                print_plain("  [error]Usage: pdl queue pause <id>... (or --all)[/error]", force=True)
                return int(ExitCode.USAGE)
            try:
                ids = [] if pause_all else [int(x) for x in q_args]
            except ValueError:
                print_plain(f"  [error]Invalid queue job ID(s): {q_args!r}[/error]", force=True)
                return int(ExitCode.USAGE)
            paused = QueueManager.pause_jobs(ids, pause_all=pause_all)
            if not UI.json_mode and not UI.quiet:
                print_plain(f"  [green]✓ Paused {paused} queued job(s).[/green]")
            UI.emit_event("queue_paused", paused=paused)
            return int(ExitCode.OK)

        if act == "resume":
            resume_all = bool(getattr(args, "all_flag", False)) or ("all" in [x.lower() for x in q_args]) or not q_args
            try:
                ids = [] if resume_all else [int(x) for x in q_args]
            except ValueError:
                print_plain(f"  [error]Invalid queue job ID(s): {q_args!r}[/error]", force=True)
                return int(ExitCode.USAGE)
            resumed = QueueManager.resume_jobs(ids, resume_all=resume_all)
            if not UI.json_mode and not UI.quiet:
                print_plain(f"  [green]✓ Resumed {resumed} queued job(s).[/green]")
            UI.emit_event("queue_resumed", resumed=resumed)
            return int(ExitCode.OK)

        if act == "run":
            try:
                wf_bps = self._resolve_when_fast_bps(args)
                at_norm = format_start_at_time(getattr(args, "start_at", None))
            except ValueError as exc:
                print_plain(f"  [error]{exc}[/error]", force=True)
                UI.emit_event("error", message=str(exc))
                return int(ExitCode.USAGE)

            raw_conn = getattr(args, "connections", None) or self.config.connections
            global_conn = min(max(int(raw_conn), 1), 16)
            req_ac_override = getattr(args, "require_ac", None)
            code, _ = QueueManager.run_queue(
                global_connections=global_conn,
                when_fast_override_bps=wf_bps if wf_bps > 0 else None,
                at_override=at_norm,
                require_ac_override=req_ac_override,
                caffeinate=bool(getattr(args, "caffeinate", self.config.caffeinate)),
                net_watch=bool(getattr(args, "net_watch", self.config.net_watch)),
                stall_timeout=float(getattr(args, "stall_timeout", self.config.stall_timeout)),
                notify=bool(getattr(args, "notify", False)),
            )
            return code

        if act in ("launchd-plist", "plist"):
            try:
                at_norm = format_start_at_time(getattr(args, "start_at", None))
            except ValueError as exc:
                print_plain(f"  [error]{exc}[/error]", force=True)
                return int(ExitCode.USAGE)

            dest_str = q_args[0] if q_args else getattr(args, "output", None)
            dest_p = Path(dest_str).expanduser().resolve() if dest_str else None
            interval_sec = int(getattr(args, "interval", 900) or 900)
            try:
                generate_launchd_plist(
                    dest_path=dest_p,
                    start_at=at_norm,
                    interval_sec=interval_sec,
                )
                return int(ExitCode.OK)
            except ValueError as exc:
                print_plain(f"  [error]{exc}[/error]", force=True)
                return int(ExitCode.FAILURE)

        print_plain(
            f"  [error]Unknown queue action: {action!r} (expected add, list, remove, pause, resume, run, or launchd-plist)[/error]",
            force=True,
        )
        return int(ExitCode.USAGE)

    def handle_resume_command(
        self,
        r_args: list[str],
        args: argparse.Namespace,
    ) -> int:
        from pdl.queue import QueueManager

        resume_all = bool(getattr(args, "all_flag", False)) or ("all" in [x.lower() for x in r_args]) or not r_args
        try:
            ids = [] if resume_all else [int(x) for x in r_args]
        except ValueError:
            print_plain("  [error]Usage: pdl resume --all (or pdl resume <id>...)[/error]", force=True)
            return int(ExitCode.USAGE)

        resumed = QueueManager.resume_jobs(ids, resume_all=resume_all)
        UI.emit_event("queue_resumed", resumed=resumed, all=resume_all)
        if not UI.json_mode and not UI.quiet:
            print_plain(f"  [green]✓ Resumed {resumed} job(s); running queue in foreground...[/green]")

        raw_conn = getattr(args, "connections", None) or self.config.connections
        global_conn = min(max(int(raw_conn), 1), 16)
        req_ac_override = getattr(args, "require_ac", None)
        code, _ = QueueManager.run_queue(
            global_connections=global_conn,
            require_ac_override=req_ac_override,
            caffeinate=bool(getattr(args, "caffeinate", self.config.caffeinate)),
            net_watch=bool(getattr(args, "net_watch", self.config.net_watch)),
            stall_timeout=float(getattr(args, "stall_timeout", self.config.stall_timeout)),
            notify=bool(getattr(args, "notify", False)),
        )
        return code

    def execute(self, args: argparse.Namespace) -> int:
        if getattr(args, "hash_file", None):
            from pdl.postproc.checksum import ChecksumEngine

            ok = ChecksumEngine.display_hash_table(Path(args.hash_file))
            return int(ExitCode.OK if ok else ExitCode.FAILURE)

        if getattr(args, "auth_action", None):
            from pdl.auth import handle_auth_command

            return handle_auth_command(
                args.auth_action,
                getattr(args, "auth_service", None),
                getattr(args, "auth_token", None),
            )

        if getattr(args, "doctor", False) or getattr(args, "diagnose", False):
            from pdl.doctor import run_doctor

            return run_doctor()

        if getattr(args, "history", False):
            HistoryManager.display(limit=getattr(args, "history_limit", 15) or 15)
            return int(ExitCode.OK)

        if getattr(args, "clear_history", False):
            HistoryManager.clear()
            print_plain("  [green]✓ Download history cleared.[/green]")
            return int(ExitCode.OK)

        if getattr(args, "clean", False):
            return self.clean_orphans(Path(getattr(args, "output", None) or self.config.output_dir))

        if getattr(args, "info_target", None):
            return TargetInspector.inspect(
                args.info_target,
                ejs_remote=getattr(args, "ejs_remote", False),
                player_client=self.config.ytdlp_player_client,
            )

        if getattr(args, "extract_file", None):
            from pdl.postproc.archive import ArchiveExtractor, ArchiveSecurityError

            target_out = Path(getattr(args, "output", None) or self.config.output_dir)
            try:
                succ, _, _ = ArchiveExtractor.extract(Path(args.extract_file), target_out)
                return int(ExitCode.OK if succ else ExitCode.FAILURE)
            except ArchiveSecurityError:
                return int(ExitCode.VERIFICATION_FAILED)

        if getattr(args, "install_target", None):
            raw_target = str(args.install_target).strip()
            conn = min(max(getattr(args, "connections", 16) or 16, 1), 16)
            assume_yes = bool(getattr(args, "yes", False))

            if raw_target.lower().startswith(("pip:", "pip/", "pypi:", "pypi/")):
                from pdl.engines.pypi import PyPIEngine

                inst_res = PyPIEngine.install(
                    raw_target,
                    connections=conn,
                    assume_yes=assume_yes,
                )
            elif raw_target.lower().startswith(("brew:", "brew/", "homebrew:", "homebrew/")):
                from pdl.engines.brew import BrewEngine

                inst_res = BrewEngine.install(
                    raw_target,
                    connections=conn,
                    assume_yes=assume_yes,
                )
            elif "/" not in raw_target and not raw_target.startswith("http"):
                import shutil

                from pdl.engines.brew import BrewEngine

                if shutil.which("brew") and BrewEngine.get_brew_cache_path(raw_target):
                    inst_res = BrewEngine.install(
                        f"brew:{raw_target}",
                        connections=conn,
                        assume_yes=assume_yes,
                    )
                else:
                    from pdl.engines.github import BinaryInstaller

                    inst_res = BinaryInstaller.install(
                        raw_target,
                        Path(self.config.bin_dir),
                        assume_yes=assume_yes,
                        connections=conn,
                        sign=bool(getattr(args, "sign", False)),
                        write_xattr=not bool(getattr(args, "no_xattr", False)),
                    )
            else:
                from pdl.engines.github import BinaryInstaller

                inst_res = BinaryInstaller.install(
                    raw_target,
                    Path(self.config.bin_dir),
                    assume_yes=assume_yes,
                    connections=conn,
                    sign=bool(getattr(args, "sign", False)),
                    write_xattr=not bool(getattr(args, "no_xattr", False)),
                )
            UI.emit_event(
                "result",
                subcommand="install",
                target=args.install_target,
                success=inst_res.success,
                file_path=str(inst_res.file_path) if inst_res.file_path else None,
                checksum_verified=inst_res.checksum_verified,
                error=inst_res.error,
                exit_code=inst_res.exit_code,
            )
            from pdl.hooks import HookRunner

            HookRunner.run_result_actions(
                {
                    "event": "result",
                    "subcommand": "install",
                    "target": args.install_target,
                    "success": inst_res.success,
                    "file_path": str(inst_res.file_path) if inst_res.file_path else None,
                    "checksum_verified": inst_res.checksum_verified,
                    "error": inst_res.error,
                    "exit_code": inst_res.exit_code,
                },
                is_success=inst_res.success,
                notify=bool(getattr(args, "notify", False)),
            )
            return inst_res.exit_code

        if getattr(args, "netprobe_url", None):
            from pdl.netprobe import run_netprobe

            code, _ = run_netprobe(
                args.netprobe_url,
                duration_sec=float(getattr(args, "duration", 60.0) or 60.0),
                http3_probe=bool(getattr(args, "http3_probe", False)),
            )
            return code

        if getattr(args, "bench_url", None):
            from pdl.bench import run_bench

            code, _ = run_bench(
                args.bench_url,
                modes_str=str(getattr(args, "watchdog", None) or "v1,v2,off"),
                duration_sec=float(getattr(args, "duration", 60.0) or 60.0),
                connections=min(max(getattr(args, "connections", 16) or 16, 1), 16),
                anti_throttle=getattr(args, "anti_throttle", self.config.anti_throttle),
                grace_period=int(getattr(args, "anti_throttle_grace", self.config.anti_throttle_grace)),
            )
            return code

        # Validate rate flags up front (Phase 0 #7: reject unparseable values with a clear error)
        limit_rate = getattr(args, "limit_rate", None) or self.config.max_download_limit
        if limit_rate:
            try:
                parse_rate(limit_rate)
            except ValueError as exc:
                print_plain(f"  [error]Invalid --limit-rate value: {exc}[/error]", force=True)
                UI.emit_event("error", message=str(exc))
                return int(ExitCode.USAGE)

        anti_throttle = getattr(args, "anti_throttle", self.config.anti_throttle)
        if anti_throttle:
            try:
                parse_rate(anti_throttle)
            except ValueError as exc:
                print_plain(f"  [error]Invalid --anti-throttle value: {exc}[/error]", force=True)
                UI.emit_event("error", message=str(exc))
                return int(ExitCode.USAGE)

        watchdog_str = (getattr(args, "watchdog", None) or self.config.watchdog or "v2").lower()
        try:
            watchdog_mode = WatchdogMode(watchdog_str)
        except ValueError:
            print_plain(
                f"  [error]Invalid --watchdog mode: {watchdog_str!r} (expected v1, v2, or off)[/error]",
                force=True,
            )
            return int(ExitCode.USAGE)

        stall_timeout = float(getattr(args, "stall_timeout", self.config.stall_timeout))
        if stall_timeout < 0:
            msg = f"Invalid --stall-timeout value: {stall_timeout} (must be >= 0)"
            print_plain(f"  [error]{msg}[/error]", force=True)
            UI.emit_event("error", message=msg)
            return int(ExitCode.USAGE)

        caffeinate = bool(getattr(args, "caffeinate", self.config.caffeinate))
        net_watch = bool(getattr(args, "net_watch", self.config.net_watch))
        req_ac_arg = getattr(args, "require_ac", None)
        require_ac = self.config.require_ac if req_ac_arg is None else bool(req_ac_arg)
        pause_on_low_power = bool(getattr(args, "pause_on_low_power", self.config.pause_on_low_power))
        cli_mirrors = list(getattr(args, "mirrors", []) or [])

        try:
            when_fast_bps = self._resolve_when_fast_bps(args)
        except ValueError as exc:
            print_plain(f"  [error]Invalid --when-fast value: {exc}[/error]", force=True)
            UI.emit_event("error", message=str(exc))
            return int(ExitCode.USAGE)

        start_at = getattr(args, "start_at", None)
        if start_at is not None:
            from pdl.queue import is_schedule_due, parse_start_at_time

            try:
                parse_start_at_time(start_at)
            except ValueError as exc:
                print_plain(f"  [error]{exc}[/error]", force=True)
                UI.emit_event("error", message=str(exc))
                return int(ExitCode.USAGE)
            due, wait_s = is_schedule_due(start_at)
            if not due:
                msg = f"Transfer scheduled for --at {start_at} ({int(wait_s)}s from now)."
                print_plain(f"  [warning]⏸ {msg}[/warning]", force=True)
                UI.emit_event("deferred", reason="start_at", start_at=start_at, wait_seconds=round(wait_s, 1))
                return int(ExitCode.FAILURE)

        if getattr(args, "from_curl", None) is not None or getattr(args, "clipboard", False):
            return self.handle_curl(args)

        targets: list[str] = list(getattr(args, "targets", []) or [])
        if getattr(args, "input_file", None):
            inp_path = Path(args.input_file).expanduser().resolve()
            if not inp_path.exists():
                msg = f"Input file '{args.input_file}' not found."
                print_plain(f"  [error]Error: {msg}[/error]", force=True)
                UI.emit_event("error", message=msg)
                return int(ExitCode.USAGE)
            with open(inp_path, encoding="utf-8") as f:
                for line in f:
                    line = line.strip()
                    if line and not line.startswith("#"):
                        targets.append(line)

        if not targets:
            print_plain("  [yellow]No targets specified. Use 'pdl --help' for recipes & options.[/yellow]\n")
            return int(ExitCode.USAGE)

        out_dir = Path(getattr(args, "output", None) or self.config.output_dir).expanduser().resolve()
        raw_conn = getattr(args, "connections", None) or self.config.connections
        connections = min(max(int(raw_conn), 1), 16)
        if raw_conn > 16:
            print_plain(
                f"  [dim]• Note: {raw_conn} streams requested; automatically clamped to maximum {connections} allowed by aria2.[/dim]"
            )

        is_audio = not bool(getattr(args, "video", False))
        if getattr(args, "mp3", False):
            audio_codec = "mp3"
        elif getattr(args, "audio_codec", None):
            audio_codec = str(args.audio_codec).lower()
            if audio_codec == "lossless":
                audio_codec = "aac"  # Clamped to 256kbps AAC max
        elif getattr(args, "no_lossless", False):
            audio_codec = "aac"
        else:
            audio_codec = self.config.audio_codec or "aac"
            if audio_codec == "lossless":
                audio_codec = "aac"

        video_quality = getattr(args, "quality", None) or self.config.video_quality
        auto_extract = bool(getattr(args, "extract", False) or self.config.auto_extract)
        push_adb = bool(getattr(args, "push_adb", False) or self.config.auto_adb_push)
        grace_period = max(int(getattr(args, "anti_throttle_grace", self.config.anti_throttle_grace) or 5), 3)

        from pdl.engines.aria2 import Aria2Engine
        from pdl.engines.github import GitHubEngine
        from pdl.engines.hf import HuggingFaceEngine
        from pdl.engines.ipsw import IpswEngine
        from pdl.engines.media import MediaEngine
        from pdl.events import check_free_disk_space, check_power_policy
        from pdl.models import TransferResult
        from pdl.net import probe_targets_parallel
        from pdl import platform
        from pdl.postproc.adb import ADBDeviceSync
        from pdl.postproc.archive import ArchiveExtractor, ArchiveSecurityError
        from pdl.postproc.checksum import ChecksumEngine

        # Start-up guard: Power policy check (--require-ac / --pause-on-low-power)
        pwr_ok, pwr_err = check_power_policy(
            require_ac=require_ac,
            pause_on_low_power=pause_on_low_power,
        )
        if not pwr_ok:
            err_msg = pwr_err or "Power policy check failed."
            print_plain(f"  [error]✗ {err_msg}[/error]", force=True)
            UI.emit_event("error", reason="power_policy", message=err_msg)
            return int(ExitCode.FAILURE)

        probed_targets: list[TargetInfo] = probe_targets_parallel(targets)
        results: list[dict[str, Any]] = []
        total_items = len(probed_targets)
        overall_exit_code = int(ExitCode.OK)

        with platform.get_sleep_assertion(enabled=caffeinate):
            for idx, (target, t_info) in enumerate(zip(targets, probed_targets, strict=True), 1):
                if total_items > 1:
                    print_plain(f"\n  [accent]── [{idx}/{total_items}][/accent] [bold white]{target}[/bold white]")

                UI.emit_event(
                    "target_classified",
                    target=target,
                    target_type=t_info.target_type.value,
                    clean_url=t_info.clean_url,
                    filename_hint=t_info.filename_hint,
                    content_length=t_info.content_length,
                    accept_ranges=t_info.accept_ranges,
                )

                t0 = time.time()
                # Start-up guard: Free-disk check when content_length is already known
                disk_ok, disk_err = check_free_disk_space(out_dir, t_info.content_length)

                if t_info.probe_error:
                    print_plain(f"  [error]✗ {t_info.probe_error}[/error]", force=True)
                    UI.emit_event(
                        "error",
                        reason="http_error",
                        status_code=t_info.status_code,
                        message=t_info.probe_error,
                        target=target,
                    )
                    tx_res = TransferResult(
                        success=False,
                        error=t_info.probe_error,
                        exit_code=int(ExitCode.FAILURE),
                    )
                elif not disk_ok:
                    err_msg = disk_err or "Insufficient free disk space."
                    print_plain(f"  [error]✗ {err_msg}[/error]", force=True)
                    UI.emit_event("error", reason="insufficient_disk_space", message=err_msg)
                    tx_res = TransferResult(
                        success=False,
                        error=err_msg,
                        exit_code=int(ExitCode.FAILURE),
                    )
                else:
                    common_opts: dict[str, Any] = {
                        "limit_rate": limit_rate,
                        "anti_throttle": anti_throttle,
                        "grace_period": grace_period,
                        "watchdog_mode": watchdog_mode,
                        "caffeinate": caffeinate,
                        "net_watch": net_watch,
                        "stall_timeout": stall_timeout,
                        "require_ac": require_ac,
                        "pause_on_low_power": pause_on_low_power,
                        "mirrors": cli_mirrors,
                        "when_fast_bps": when_fast_bps,
                        "asset": getattr(args, "asset", None),
                        "all_assets": bool(getattr(args, "all_assets", False)),
                        "select": bool(getattr(args, "select", False)),
                        "source": bool(getattr(args, "source", False)),
                        "branch": getattr(args, "branch", None),
                        "is_audio": is_audio,
                        "audio_codec": audio_codec,
                        "video_quality": video_quality,
                        "playlist": bool(getattr(args, "playlist", False)),
                        "subs": bool(getattr(args, "subs", False)),
                        "embed_thumbnail": bool(getattr(args, "thumbnail", True)),
                        "cookies_from_browser": getattr(args, "cookies_from_browser", None),
                        "sponsorblock": bool(getattr(args, "sponsorblock", False)),
                        "ejs_remote": bool(getattr(args, "ejs_remote", False)),
                        "player_client": self.config.ytdlp_player_client,
                    }

                    if t_info.target_type == TargetType.HF:
                        tx_res = HuggingFaceEngine().execute(t_info, out_dir, connections=connections, **common_opts)
                    elif t_info.target_type == TargetType.GITHUB:
                        tx_res = GitHubEngine().execute(t_info, out_dir, connections=connections, **common_opts)
                    elif t_info.target_type == TargetType.IPSW:
                        tx_res = IpswEngine().execute(t_info, out_dir, connections=connections, **common_opts)
                    elif t_info.target_type == TargetType.PYPI:
                        from pdl.engines.pypi import PyPIEngine

                        tx_res = PyPIEngine().execute(t_info, out_dir, connections=connections, **common_opts)
                    elif t_info.target_type == TargetType.BREW:
                        from pdl.engines.brew import BrewEngine

                        tx_res = BrewEngine().execute(t_info, out_dir, connections=connections, **common_opts)
                    elif t_info.target_type in (TargetType.DIRECT, TargetType.TORRENT):
                        tx_res = Aria2Engine().execute(t_info, out_dir, connections=connections, **common_opts)
                    else:
                        mode_str = f"Audio ({audio_codec.upper()})" if is_audio else f"Video ({video_quality.upper()})"
                        label = "Searching" if t_info.target_type == TargetType.SEARCH else "Media Target"
                        print_plain(
                            f"  [bold white]{label}:[/bold white] [cyan]{target}[/cyan] [dim]({mode_str})[/dim]"
                        )
                        tx_res = MediaEngine().execute(t_info, out_dir, connections=connections, **common_opts)

                out_file = tx_res.file_path

                # Explicit user checksum or companion verification (--checksum / --verify)
                if tx_res.success and out_file and out_file.is_file():
                    if getattr(args, "checksum", None):
                        matched, algo, digest = ChecksumEngine.verify(out_file, args.checksum)
                        tx_res.checksum_verified = matched
                        tx_res.checksum_algo = algo
                        tx_res.checksum_digest = digest
                        UI.emit_event(
                            "verification",
                            file=str(out_file),
                            algorithm=algo,
                            expected=args.checksum,
                            computed=digest,
                            verified=matched,
                            source="cli_checksum",
                        )
                        if matched:
                            print_plain(f"  [green]✓ Checksum verified ({algo.upper()})[/green]")
                        else:
                            print_plain(
                                f"  [error]✗ Checksum mismatch! Expected {args.checksum}, got {digest}[/error]",
                                force=True,
                            )
                            tx_res.success = False
                            tx_res.exit_code = int(ExitCode.VERIFICATION_FAILED)
                    elif getattr(args, "verify", False):
                        companion = ChecksumEngine.find_companion_checksum(out_file)
                        if companion:
                            algo, exp = companion
                            matched, _, digest = ChecksumEngine.verify(out_file, f"{algo}:{exp}")
                            tx_res.checksum_verified = matched
                            tx_res.checksum_algo = algo
                            tx_res.checksum_digest = digest
                            UI.emit_event(
                                "verification",
                                file=str(out_file),
                                algorithm=algo,
                                expected=exp,
                                computed=digest,
                                verified=matched,
                                source="companion_file",
                            )
                            if matched:
                                print_plain(f"  [green]✓ Companion checksum verified ({algo.upper()})[/green]")
                            else:
                                print_plain("  [error]✗ Companion checksum mismatch![/error]", force=True)
                                tx_res.success = False
                                tx_res.exit_code = int(ExitCode.VERIFICATION_FAILED)

                # Auto-Archive Extraction (--extract)
                if (
                    tx_res.success
                    and out_file
                    and out_file.is_file()
                    and auto_extract
                    and ArchiveExtractor.is_archive(out_file)
                ):
                    ext_dir = out_dir / out_file.stem
                    try:
                        succ, _, _ = ArchiveExtractor.extract(out_file, ext_dir)
                        if succ:
                            out_file = ext_dir
                            tx_res.file_path = ext_dir
                    except ArchiveSecurityError as sec_err:
                        tx_res.success = False
                        tx_res.error = str(sec_err)
                        tx_res.exit_code = int(ExitCode.VERIFICATION_FAILED)

                # Mobile ADB Sync (--push / --adb)
                adb_pushed = False
                if tx_res.success and out_file and push_adb and out_file.is_file():
                    dest_phone_dir = self.config.adb_music_dir if is_audio else self.config.adb_download_dir
                    adb_pushed = ADBDeviceSync.push_file(out_file, target_dir=dest_phone_dir)

                # Finder Extended Attribute (--no-xattr to disable)
                if tx_res.success and out_file and out_file.exists() and not getattr(args, "no_xattr", False):
                    source_url = t_info.clean_url or t_info.raw
                    if source_url:
                        platform.set_where_froms(out_file, [source_url])

                elapsed = time.time() - t0
                file_sz = out_file.stat().st_size if (out_file and out_file.is_file()) else tx_res.bytes_downloaded
                HistoryManager.record(
                    target=target,
                    target_type=t_info.target_type.value,
                    file_path=str(out_file) if out_file else None,
                    size=file_sz,
                    duration=elapsed,
                    success=tx_res.success,
                    adb=adb_pushed,
                    sha256=tx_res.checksum_digest if tx_res.checksum_algo == "sha256" else None,
                    history_limit=self.config.history_limit,
                )

                if tx_res.success and tx_res.avg_speed_bps > 0 and (file_sz or 0) >= 32768:
                    try:
                        from urllib.parse import urlparse as _urlparse

                        from pdl.throttle.store import ThrottleStore

                        t_host = (_urlparse(t_info.clean_url or t_info.raw).hostname or "unknown").lower()
                        ThrottleStore.record_passive_download(
                            host=t_host,
                            speed_bps=tx_res.avg_speed_bps,
                            bytes_downloaded=int(file_sz or 0),
                        )
                    except Exception:
                        pass

                UI.emit_event(
                    "result",
                    target=target,
                    target_type=t_info.target_type.value,
                    success=tx_res.success,
                    file_path=str(out_file) if out_file else None,
                    bytes_downloaded=file_sz or 0,
                    duration=round(elapsed, 3),
                    avg_speed_bps=round(tx_res.avg_speed_bps, 2),
                    recycle_count=tx_res.recycle_count,
                    audio_format_produced=tx_res.audio_format_produced,
                    checksum_verified=tx_res.checksum_verified,
                    checksum_algo=tx_res.checksum_algo,
                    checksum_digest=tx_res.checksum_digest,
                    adb_synced=adb_pushed,
                    error=tx_res.error,
                    exit_code=tx_res.exit_code,
                )

                from pdl.hooks import HookRunner

                HookRunner.run_result_actions(
                    {
                        "event": "result",
                        "target": target,
                        "target_type": t_info.target_type.value,
                        "success": tx_res.success,
                        "file_path": str(out_file) if out_file else None,
                        "bytes_downloaded": file_sz or 0,
                        "duration": round(elapsed, 3),
                        "avg_speed_bps": round(tx_res.avg_speed_bps, 2),
                        "recycle_count": tx_res.recycle_count,
                        "audio_format_produced": tx_res.audio_format_produced,
                        "checksum_verified": tx_res.checksum_verified,
                        "checksum_algo": tx_res.checksum_algo,
                        "checksum_digest": tx_res.checksum_digest,
                        "adb_synced": adb_pushed,
                        "error": tx_res.error,
                        "exit_code": tx_res.exit_code,
                    },
                    is_success=tx_res.success,
                    notify=bool(getattr(args, "notify", False)),
                )

                if not tx_res.success and overall_exit_code == int(ExitCode.OK):
                    overall_exit_code = tx_res.exit_code if tx_res.exit_code != 0 else int(ExitCode.FAILURE)

                results.append(
                    {
                        "target": target,
                        "success": tx_res.success,
                        "file": out_file,
                        "adb": adb_pushed,
                        "audio_format_produced": tx_res.audio_format_produced,
                        "avg_speed_bps": tx_res.avg_speed_bps,
                        "recycle_count": tx_res.recycle_count,
                    }
                )

        self.render_summary(results)
        return overall_exit_code

    def render_summary(self, results: list[dict[str, Any]]) -> None:
        if UI.json_mode or UI.quiet:
            return
        con = UI.get_console()
        if con is None:
            return

        if len(results) == 1:
            res = results[0]
            con.print()
            if res["success"]:
                out_f: Path | None = res["file"]
                size_str = ""
                fname = ""
                if out_f and out_f.exists():
                    fname = out_f.name
                    if out_f.is_file():
                        size_str = f" [dim]({format_size(out_f.stat().st_size)})[/dim]"

                con.print(f"  [bold green]✓ Download complete:[/bold green] [bold white]{fname}[/bold white]{size_str}")
                if fname:
                    con.print(f"    [dim]Destination:[/dim] [cyan]{out_f}[/cyan]")
                if res.get("avg_speed_bps"):
                    con.print(f"    [dim]Speed:[/dim]       [green]{format_speed(res['avg_speed_bps'])}[/green]")
                if res.get("audio_format_produced"):
                    con.print(f"    [dim]Audio:[/dim]       [dim]{res['audio_format_produced']}[/dim]")
                if res.get("adb"):
                    con.print("    [dim]Sync:[/dim]        [green]Copied to Android device[/green]")
            else:
                con.print(f"  [bold red]✗ Download failed:[/bold red] [dim]{res['target']}[/dim]")
            con.print()
            return

        from rich import box
        from rich.table import Table

        table = Table(title="Batch Summary", box=box.HORIZONTALS, pad_edge=False, expand=True)
        table.add_column("Target", style="dim", no_wrap=True, ratio=3)
        table.add_column("Status", justify="center", ratio=1)
        table.add_column("Filename", style="bold white", ratio=4)
        table.add_column("Size", justify="right", style="dim cyan", ratio=1)
        table.add_column("Device", justify="center", style="dim", ratio=1)

        for res in results:
            st = "[green]✓ ok[/green]" if res["success"] else "[red]✗ error[/red]"
            out_f = res["file"]
            size_str = "-"
            file_name_str = "-"
            if out_f and out_f.exists():
                file_name_str = out_f.name
                if out_f.is_file():
                    size_str = format_size(out_f.stat().st_size)
                else:
                    size_str = "dir"

            adb_status = "[green]synced[/green]" if res["adb"] else "[dim]-[/dim]"
            table.add_row(str(res["target"])[:35], st, file_name_str[:38], size_str, adb_status)

        con.print()
        con.print(table)
        con.print()

    def handle_curl(self, args: Any) -> int:
        """Execute a download parsed from a 'Copy as cURL' command string or clipboard."""
        import os
        import subprocess
        import sys
        from urllib.parse import urlparse

        from pdl.curl_parser import create_netscape_cookie_jar, parse_curl_command
        from pdl.engines.aria2 import Aria2Engine
        from pdl.models import ExitCode, TargetInfo, TargetType
        from pdl.ui import UI, print_plain

        curl_cmd = ""
        if getattr(args, "clipboard", False):
            try:
                proc = subprocess.run(["pbpaste"], capture_output=True, text=True, check=False)
                curl_cmd = proc.stdout.strip()
            except Exception as exc:
                print_plain(f"  [error]Failed to read clipboard via pbpaste: {exc}[/error]", force=True)
                return int(ExitCode.USAGE)
            if not curl_cmd:
                print_plain("  [error]Clipboard is empty or does not contain text.[/error]", force=True)
                return int(ExitCode.USAGE)
        else:
            from_arg = getattr(args, "from_curl", "-")
            if from_arg == "-" or not from_arg:
                if not sys.stdin.isatty():
                    curl_cmd = sys.stdin.read().strip()
                else:
                    curl_cmd = sys.stdin.readline().strip()
            elif os.path.isfile(from_arg):
                curl_cmd = Path(from_arg).read_text(encoding="utf-8").strip()
            else:
                curl_cmd = str(from_arg).strip()

        if not curl_cmd:
            print_plain("  [error]No cURL command provided via --from-curl or stdin.[/error]", force=True)
            return int(ExitCode.USAGE)

        try:
            parsed = parse_curl_command(curl_cmd)
        except ValueError as exc:
            print_plain(f"  [error]Failed to parse cURL command: {exc}[/error]", force=True)
            UI.emit_event("error", reason="curl_parse_error", message=str(exc))
            return int(ExitCode.USAGE)

        parsed_u = urlparse(parsed.url)
        host = parsed_u.netloc or ""
        cookie_jar_path = None
        cleanup_jar = None
        if parsed.cookies:
            cookie_jar_path, cleanup_jar = create_netscape_cookie_jar(
                parsed.cookies,
                host=host,
                is_secure=(parsed_u.scheme == "https"),
            )

        out_dir = Path(getattr(args, "output", None) or self.config.output_dir).expanduser().resolve()
        raw_conn = getattr(args, "connections", None) or self.config.connections
        connections = min(max(int(raw_conn), 1), 16)

        t_info = TargetInfo(
            raw=parsed.url,
            target_type=TargetType.DIRECT,
            clean_url=parsed.url,
            filename_hint=parsed.output_filename,
            headers=dict(parsed.headers),
            cookie_file=str(cookie_jar_path) if cookie_jar_path else None,
            referer=parsed.referer,
            user_agent=parsed.user_agent,
        )

        try:
            res = Aria2Engine.download(
                t_info,
                out_dir,
                connections=connections,
                limit_rate=getattr(args, "limit_rate", None),
                anti_throttle=getattr(args, "anti_throttle", self.config.anti_throttle),
                grace_period=getattr(args, "anti_throttle_grace", self.config.anti_throttle_grace),
                watchdog_mode=str(getattr(args, "watchdog_mode", None) or self.config.watchdog or "v2"),
                caffeinate=not getattr(args, "no_caffeinate", False),
                net_watch=not getattr(args, "no_net_watch", False),
                stall_timeout=getattr(args, "stall_timeout", 30.0),
                require_ac=getattr(args, "require_ac", False),
                pause_on_low_power=getattr(args, "pause_on_low_power", False),
                write_xattr=not getattr(args, "no_xattr", False),
                notify=getattr(args, "notify", False),
            )
            return int(res.exit_code)
        finally:
            if cleanup_jar:
                cleanup_jar()
