"""Target inspector — metadata classification without downloading.

Inspects a target URL or query and reports its classification, resolved URL,
and service-specific metadata (GitHub repo, Hugging Face model, IPSW firmware,
PyPI package, Homebrew formula, media stream, etc.) without starting a download.
"""

from __future__ import annotations

from typing import Any

from paradl.core.models import ExitCode, TargetType, format_size
from paradl.ui import UI


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
        from paradl.engines.github import GitHubEngine
        from paradl.engines.hf import HuggingFaceEngine
        from paradl.engines.media import MediaEngine
        from paradl.net.client import probe_target

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
                props["Accept-Ranges"] = (
                    "206 Partial Content" if t_info.accept_ranges else "200 Only"
                )
            if t_info.etag_sha256:
                props["ETag SHA-256"] = t_info.etag_sha256

        if UI.json_mode:
            UI.emit_event("info", target=target, properties=props)
            return int(ExitCode.OK)

        from rich import box
        from rich.table import Table

        table = Table(
            title=f"Target Inspector: {target}", box=box.HORIZONTALS, pad_edge=False
        )
        table.add_column("Property", style="bold cyan")
        table.add_column("Value", style="white")
        for k, v in props.items():
            table.add_row(str(k), str(v))

        UI.print()
        UI.print_renderable(table)
        UI.print()
        return int(ExitCode.OK)
