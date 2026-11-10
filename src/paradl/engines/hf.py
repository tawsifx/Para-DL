"""Hugging Face Hub acceleration engine with Auth headers, LFS SHA-256 verification, and hf_xet/Aria2 RPC."""

from __future__ import annotations

import fnmatch
import importlib.util
import logging
import os
import re
import time
from pathlib import Path
from typing import Any

from paradl import VERSION, platform
from paradl.engines.aria2 import Aria2Engine
from paradl.models import (
    DownloadItem,
    ExitCode,
    TargetInfo,
    TargetType,
    TransferResult,
    WatchdogMode,
)
from paradl.net import fetch_json_with_etag, probe_http_url
from paradl.postproc.checksum import ChecksumEngine
from paradl.ui import UI, print_plain

logger = logging.getLogger(__name__)


def _matches_asset_pattern(filename: str, pattern: str) -> bool:
    """Match filename against glob pattern (e.g. '*.gguf', '*Q4_K_M*') or regex."""
    fn_lower = filename.lower()
    pat_lower = pattern.lower()
    if fnmatch.fnmatchcase(fn_lower, pat_lower) or fnmatch.fnmatchcase(
        os.path.basename(fn_lower), pat_lower
    ):
        return True
    if pat_lower in fn_lower:
        return True
    try:
        return bool(re.search(pattern, filename, re.IGNORECASE))
    except re.error:
        return False


def _has_hf_xet() -> bool:
    return (
        importlib.util.find_spec("huggingface_hub") is not None
        and importlib.util.find_spec("hf_xet") is not None
    )


class HuggingFaceEngine:
    """Downloads models and datasets from Hugging Face Hub with Bearer auth and digest checks."""

    @staticmethod
    def get_token() -> str | None:
        token = os.environ.get("HF_TOKEN") or os.environ.get("HUGGING_FACE_HUB_TOKEN")
        if not token:
            token = platform.get_keychain_token("hf")
        if not token:
            token_file = Path.home() / ".cache" / "huggingface" / "token"
            if token_file.exists():
                try:
                    token = token_file.read_text(encoding="utf-8").strip()
                except OSError as exc:
                    logger.debug("Could not read HF token file: %s", exc)
        if token:
            UI.register_secret(token)
        return token or None

    @classmethod
    def get_headers(cls) -> dict[str, str]:
        headers = {"User-Agent": f"pdl-hf-accelerator/{VERSION}"}
        token = cls.get_token()
        if token:
            headers["Authorization"] = f"Bearer {token}"
        return headers

    @staticmethod
    def is_hf(target: str) -> bool:
        t = target.strip().lower()
        return t.startswith("hf:") or "huggingface.co" in t

    @staticmethod
    def parse_hf_target(target: str) -> dict[str, Any]:
        t = target.strip()
        repo_type = "model"
        revision = "main"

        if t.lower().startswith("hf:"):
            raw_path = t[3:].strip()
            if raw_path.startswith("datasets/"):
                repo_type = "dataset"
                raw_path = raw_path[len("datasets/") :]

            parts = [p for p in raw_path.split("/") if p]
            if len(parts) >= 2:
                repo_id = f"{parts[0]}/{parts[1]}"
                subpath = "/".join(parts[2:]) if len(parts) > 2 else None
                return {
                    "repo_id": repo_id,
                    "repo_type": repo_type,
                    "subpath": subpath,
                    "revision": revision,
                }
            if len(parts) == 1:
                return {
                    "repo_id": parts[0],
                    "repo_type": repo_type,
                    "subpath": None,
                    "revision": revision,
                }

        m_file = re.search(
            r"huggingface\.co/(?:(datasets)/)?([^/]+/[^/]+)/(?:resolve|blob)/([^/]+)/(.+)",
            t,
        )
        if m_file:
            ds_prefix, repo_id, revision, filepath = m_file.groups()
            filepath = filepath.split("?", 1)[0].split("#", 1)[0]
            return {
                "repo_id": repo_id,
                "repo_type": "dataset" if ds_prefix else "model",
                "subpath": filepath,
                "revision": revision,
                "is_direct_file": True,
            }

        m_tree = re.search(
            r"huggingface\.co/(?:(datasets)/)?([^/]+/[^/]+)/tree/([^/]+)(?:/(.+))?",
            t,
        )
        if m_tree:
            ds_prefix, repo_id, revision, subpath = m_tree.groups()
            subpath = subpath.split("?", 1)[0].split("#", 1)[0] if subpath else None
            return {
                "repo_id": repo_id,
                "repo_type": "dataset" if ds_prefix else "model",
                "subpath": subpath,
                "revision": revision,
            }

        m_repo = re.search(
            r"huggingface\.co/(?:(datasets)/)?([^/]+/[^/?#]+)(?:/)?(?:\?[^#]*)?(?:#.*)?$", t
        )
        if m_repo:
            ds_prefix, repo_id = m_repo.groups()
            return {
                "repo_id": repo_id,
                "repo_type": "dataset" if ds_prefix else "model",
                "subpath": None,
                "revision": revision,
            }

        return {"repo_id": t, "repo_type": "model", "subpath": None, "revision": revision}

    @classmethod
    def fetch_hf_info(cls, repo_id: str, repo_type: str = "model") -> dict[str, Any] | None:
        endpoint = "datasets" if repo_type == "dataset" else "models"
        api_url = f"https://huggingface.co/api/{endpoint}/{repo_id}"
        data, _hdrs, status = fetch_json_with_etag(api_url, headers=cls.get_headers(), timeout=7.0)
        if status == 200 and isinstance(data, dict):
            return data
        return None

    @classmethod
    def _build_resolve_url(
        cls, repo_id: str, revision: str, rfilename: str, repo_type: str = "model"
    ) -> str:
        prefix = "datasets/" if repo_type == "dataset" else ""
        return f"https://huggingface.co/{prefix}{repo_id}/resolve/{revision}/{rfilename}"

    @classmethod
    def download(
        cls,
        meta: dict[str, Any],
        out_dir: Path,
        connections: int = 16,
        pattern: str | None = None,
        interactive: bool = False,
        limit_rate: str | None = None,
        anti_throttle: str | None = "4MBps",
        grace_period: int = 5,
        watchdog_mode: WatchdogMode | str = WatchdogMode.V2,
        http3: bool = True,
        proxy: str | None = None,
    ) -> TransferResult:
        repo_id = meta["repo_id"]
        revision = meta.get("revision", "main")
        subpath = meta.get("subpath")
        repo_type = meta.get("repo_type", "model")
        safe_conn = min(max(int(connections), 1), 16)
        auth_headers = cls.get_headers()

        out_dir.mkdir(parents=True, exist_ok=True)

        # Single direct file download (Phase 0 #1 & #12: sends Auth headers + verifies ETag SHA-256)
        if meta.get("is_direct_file") and subpath:
            direct_url = cls._build_resolve_url(repo_id, revision, subpath, repo_type)
            fname = os.path.basename(subpath)
            print_plain(
                f"  [bold white]Hugging Face File:[/bold white] [cyan]{fname}[/cyan] [dim]({repo_id})[/dim]"
            )
            p_res = probe_http_url(direct_url, headers=auth_headers)
            etag_sha = p_res.etag_sha256 if p_res.ok else None
            t_info = TargetInfo(
                raw=direct_url,
                target_type=TargetType.DIRECT,
                clean_url=direct_url,
                filename_hint=fname,
                content_length=p_res.content_length if p_res.ok else None,
                accept_ranges=p_res.accept_ranges if p_res.ok else None,
                etag_sha256=etag_sha,
                expected_digest=f"sha256:{etag_sha}" if etag_sha else None,
                headers=auth_headers,
            )
            return Aria2Engine.download(
                t_info,
                out_dir,
                connections=safe_conn,
                limit_rate=limit_rate,
                anti_throttle=anti_throttle,
                grace_period=grace_period,
                watchdog_mode=watchdog_mode,
                http3=http3,
                proxy=proxy,
            )

        print_plain(
            f"  [dim]•[/dim] [white]Querying Hugging Face Hub:[/white] [cyan]{repo_id}[/cyan]"
        )
        info = cls.fetch_hf_info(repo_id, repo_type)
        if not info:
            msg = f"Repository not found or private on Hugging Face: {repo_id}"
            print_plain(f"  [error]✗ {msg}[/error]", force=True)
            return TransferResult(success=False, error=msg, exit_code=int(ExitCode.FAILURE))

        siblings = info.get("siblings", [])
        if not siblings:
            msg = f"No files found in repository {repo_id}"
            print_plain(f"  [error]✗ {msg}[/error]", force=True)
            return TransferResult(success=False, error=msg, exit_code=int(ExitCode.FAILURE))

        valid_files = [s for s in siblings if not s.get("rfilename", "").startswith(".")]

        if subpath:
            valid_files = [
                s
                for s in valid_files
                if s.get("rfilename") == subpath or s.get("rfilename", "").endswith("/" + subpath)
            ]

        if pattern:
            valid_files = [
                s for s in valid_files if _matches_asset_pattern(s.get("rfilename", ""), pattern)
            ]

        if not valid_files:
            if pattern or subpath:
                filter_desc = f"pattern {pattern!r}" if pattern else f"subpath {subpath!r}"
                err_msg = f"No files matched {filter_desc} in repository {repo_id}."
                print_plain(f"  [error]✗ {err_msg}[/error]", force=True)
                return TransferResult(
                    success=False,
                    error=err_msg,
                    exit_code=int(ExitCode.FAILURE),
                )
            print_plain("  [warning]No files matched filter. Showing repository files:[/warning]")
            valid_files = [s for s in siblings if not s.get("rfilename", "").startswith(".")]

        # Display table when interactive or multiple files match
        if (interactive or len(valid_files) > 1) and not UI.json_mode and not UI.quiet:
            from rich import box
            from rich.table import Table

            table = Table(
                title=f"Hugging Face: {repo_id} ({len(valid_files)} files)",
                box=box.HORIZONTALS,
                pad_edge=False,
            )
            table.add_column("#", justify="right", style="cyan")
            table.add_column("Filename", style="bold white")
            table.add_column("Type", style="dim")

            for idx, f in enumerate(valid_files[:30], 1):
                fname = f.get("rfilename", "")
                ext = os.path.splitext(fname)[1] or "file"
                table.add_row(str(idx), fname, ext)

            UI.print()
            UI.print_renderable(table)
            if len(valid_files) > 30:
                UI.print(f"  [dim]... and {len(valid_files) - 30} more files[/dim]")
            UI.print()

        # Non-interactive safety: only prompt if --select was passed on an interactive TTY
        if UI.can_prompt(select_requested=interactive) and len(valid_files) > 1:
            choices = [str(i) for i in range(1, min(len(valid_files), 30) + 1)] + ["all"]
            ans = UI.ask_choice(
                "  Select file number to download (or 'all')", choices=choices, default="1"
            )
            if ans != "all":
                valid_files = [valid_files[int(ans) - 1]]

        # Single file path
        if len(valid_files) == 1:
            f_entry = valid_files[0]
            rfilename = f_entry["rfilename"]
            dl_url = cls._build_resolve_url(repo_id, revision, rfilename, repo_type)
            fname = os.path.basename(rfilename)
            sub_dest = out_dir / os.path.dirname(rfilename)
            sub_dest.mkdir(parents=True, exist_ok=True)
            print_plain(
                f"  [green]✓[/green] [white]Selected:[/white] [bold cyan]{fname}[/bold cyan]"
            )

            lfs_info = f_entry.get("lfs") if isinstance(f_entry.get("lfs"), dict) else None
            lfs_sha = lfs_info.get("oid") if lfs_info else None
            lfs_size = lfs_info.get("size") if lfs_info else f_entry.get("size")

            p_res = probe_http_url(dl_url, headers=auth_headers)
            etag_sha = lfs_sha or (p_res.etag_sha256 if p_res.ok else None)
            content_len = lfs_size or (p_res.content_length if p_res.ok else None)

            t_info = TargetInfo(
                raw=dl_url,
                target_type=TargetType.DIRECT,
                clean_url=dl_url,
                filename_hint=fname,
                content_length=content_len,
                accept_ranges=p_res.accept_ranges if p_res.ok else None,
                etag_sha256=etag_sha,
                expected_digest=f"sha256:{etag_sha}" if etag_sha else None,
                headers=auth_headers,
            )
            return Aria2Engine.download(
                t_info,
                sub_dest,
                connections=safe_conn,
                limit_rate=limit_rate,
                anti_throttle=anti_throttle,
                grace_period=grace_period,
                watchdog_mode=watchdog_mode,
                http3=http3,
                proxy=proxy,
            )

        # Phase 5: Multi-file download via huggingface_hub (with hf_xet) if available, else Aria2 RPC engine
        if _has_hf_xet():
            try:
                from huggingface_hub import hf_hub_download

                print_plain(
                    f"  [dim]• Downloading {len(valid_files)} files from {repo_id} via huggingface_hub (hf_xet)...[/dim]"
                )
                t0 = time.time()
                downloaded_paths: list[Path] = []
                total_bytes = 0
                token = cls.get_token()
                for f_entry in valid_files:
                    rfilename = f_entry["rfilename"]
                    local_p = Path(
                        hf_hub_download(
                            repo_id=repo_id,
                            filename=rfilename,
                            repo_type=repo_type,
                            revision=revision,
                            local_dir=str(out_dir),
                            token=token,
                        )
                    )
                    downloaded_paths.append(local_p)
                    if local_p.exists() and local_p.is_file():
                        total_bytes += local_p.stat().st_size
                        lfs_info = (
                            f_entry.get("lfs") if isinstance(f_entry.get("lfs"), dict) else None
                        )
                        lfs_sha = lfs_info.get("oid") if lfs_info else None
                        if lfs_sha:
                            matched, algo, digest = ChecksumEngine.verify(
                                local_p, f"sha256:{lfs_sha}"
                            )
                            UI.emit_event(
                                "verification",
                                file=str(local_p),
                                algorithm=algo,
                                expected=lfs_sha,
                                computed=digest,
                                verified=matched,
                                source="hf_lfs_oid",
                            )
                            if not matched:
                                return TransferResult(
                                    success=False,
                                    file_path=local_p,
                                    error=f"HF LFS SHA-256 mismatch on {local_p.name}",
                                    exit_code=int(ExitCode.VERIFICATION_FAILED),
                                )
                dur = max(time.time() - t0, 0.001)
                return TransferResult(
                    success=True,
                    file_path=out_dir,
                    files=downloaded_paths,
                    bytes_downloaded=total_bytes,
                    duration=dur,
                    avg_speed_bps=total_bytes / dur,
                )
            except Exception as hf_exc:
                logger.debug(
                    "hf_xet transfer failed (%s); falling back to Aria2 RPC engine", hf_exc
                )

        print_plain(
            f"  [dim]• Batch downloading {len(valid_files)} files from {repo_id} via Aria2 RPC ({safe_conn} global conns)...[/dim]"
        )
        batch_items: list[DownloadItem] = []
        for f_entry in valid_files:
            rfilename = f_entry["rfilename"]
            dl_url = cls._build_resolve_url(repo_id, revision, rfilename, repo_type)
            sub_dest = out_dir / os.path.dirname(rfilename)
            sub_dest.mkdir(parents=True, exist_ok=True)
            lfs_info = f_entry.get("lfs") if isinstance(f_entry.get("lfs"), dict) else None
            lfs_sha = lfs_info.get("oid") if lfs_info else None
            lfs_size = lfs_info.get("size") if lfs_info else f_entry.get("size")
            batch_items.append(
                DownloadItem(
                    url=dl_url,
                    out_dir=sub_dest,
                    filename=os.path.basename(rfilename),
                    headers=auth_headers,
                    expected_length=lfs_size,
                    expected_digest=f"sha256:{lfs_sha}" if lfs_sha else None,
                    etag_sha256=lfs_sha,
                )
            )

        return Aria2Engine.download_batch(
            items=batch_items,
            out_dir=out_dir,
            connections=safe_conn,
            limit_rate=limit_rate,
            anti_throttle=anti_throttle,
            grace_period=grace_period,
            watchdog_mode=watchdog_mode,
            http3=http3,
            proxy=proxy,
        )

    def execute(
        self,
        target_info: TargetInfo,
        out_dir: Path,
        *,
        connections: int = 16,
        **options: Any,
    ) -> TransferResult:
        meta = target_info.hf_meta or self.parse_hf_target(target_info.raw)
        return self.download(
            meta=meta,
            out_dir=out_dir,
            connections=connections,
            pattern=options.get("asset"),
            interactive=bool(options.get("select", False)),
            limit_rate=options.get("limit_rate"),
            anti_throttle=options.get("anti_throttle", "4MBps"),
            grace_period=options.get("grace_period", 5),
            watchdog_mode=options.get("watchdog_mode", WatchdogMode.V2),
            http3=bool(options.get("http3", True)),
            proxy=options.get("proxy"),
        )
