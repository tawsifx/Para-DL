"""GitHub engine for releases, assets, trees, archives, and verified binary installation."""

from __future__ import annotations

import fnmatch
import logging
import os
import platform
import re
import shutil
import subprocess
import tempfile
from pathlib import Path
from typing import Any

from pdl import VERSION
from pdl.engines.aria2 import Aria2Engine
from pdl.models import (
    DownloadItem,
    ExitCode,
    TargetInfo,
    TargetType,
    TransferResult,
    WatchdogMode,
    format_size,
)
from pdl.net import fetch_json_with_etag, request_with_retry
from pdl import platform as pdl_platform
from pdl.platform import macos
from pdl.postproc.archive import ArchiveExtractor
from pdl.postproc.checksum import ChecksumEngine
from pdl.toolchain import TOOLCHAIN
from pdl.ui import UI, print_plain

logger = logging.getLogger(__name__)

_GH_TOKEN_CACHE: str | None = None
_GH_TOKEN_PROBED: bool = False


def _matches_pattern(name: str, pattern: str) -> bool:
    n_low = name.lower()
    p_low = pattern.lower()
    if fnmatch.fnmatchcase(n_low, p_low) or p_low in n_low:
        return True
    try:
        return bool(re.search(pattern, name, re.IGNORECASE))
    except re.error:
        return False


def _is_binary_executable(file_path: Path) -> bool:
    """Return True if file_path has Mach-O or ELF magic bytes or executable bit."""
    try:
        with open(file_path, "rb") as f:
            magic = f.read(4)
        if magic in (
            b"\xcf\xfa\xed\xfe",  # Mach-O 64-bit LE
            b"\xfe\xed\xfa\xcf",  # Mach-O 64-bit BE
            b"\xce\xfa\xed\xfe",  # Mach-O 32-bit LE
            b"\xca\xfe\xba\xbe",  # Mach-O Universal Fat Binary
            b"\x7fELF",  # ELF binary
        ):
            return True
    except OSError:
        return False
    return os.access(file_path, os.X_OK)


class GitHubEngine:
    """Handles GitHub release assets, repo archives, directory trees, and rate-limited API calls."""

    @classmethod
    def reset_token_cache(cls) -> None:
        global _GH_TOKEN_CACHE, _GH_TOKEN_PROBED
        _GH_TOKEN_CACHE = None
        _GH_TOKEN_PROBED = False

    @classmethod
    def get_token(cls) -> str | None:
        global _GH_TOKEN_CACHE, _GH_TOKEN_PROBED
        env_tok = os.environ.get("GITHUB_TOKEN") or os.environ.get("GH_TOKEN")
        if env_tok and env_tok.strip():
            tok = env_tok.strip()
            UI.register_secret(tok)
            return tok

        if not _GH_TOKEN_PROBED:
            _GH_TOKEN_PROBED = True
            kc_tok = pdl_platform.get_keychain_token("github")
            if kc_tok and kc_tok.strip():
                _GH_TOKEN_CACHE = kc_tok.strip()
                UI.register_secret(_GH_TOKEN_CACHE)
                return _GH_TOKEN_CACHE

            gh_bin = TOOLCHAIN.gh
            if gh_bin:
                try:
                    proc = subprocess.run(
                        [gh_bin, "auth", "token"],
                        capture_output=True,
                        text=True,
                        timeout=2.5,
                        check=False,
                    )
                    if proc.returncode == 0 and proc.stdout.strip():
                        _GH_TOKEN_CACHE = proc.stdout.strip()
                        UI.register_secret(_GH_TOKEN_CACHE)
                except (OSError, subprocess.SubprocessError) as exc:
                    logger.debug("gh auth token probe failed: %s", exc)
        return _GH_TOKEN_CACHE

    @classmethod
    def get_headers(cls) -> dict[str, str]:
        headers = {
            "User-Agent": f"pdl-accelerator/{VERSION}",
            "Accept": "application/vnd.github.v3+json",
        }
        token = cls.get_token()
        if token:
            headers["Authorization"] = f"Bearer {token}"
        return headers

    @classmethod
    def get_download_headers(cls) -> dict[str, str]:
        headers = {"User-Agent": f"pdl-accelerator/{VERSION}"}
        token = cls.get_token()
        if token:
            headers["Authorization"] = f"Bearer {token}"
        return headers

    @staticmethod
    def _check_rate_limit(headers_lower: dict[str, str], status_code: int) -> None:
        if status_code in (403, 429):
            rem = headers_lower.get("x-ratelimit-remaining")
            reset_ts = headers_lower.get("x-ratelimit-reset")
            if rem == "0" or status_code == 429:
                reset_msg = f" (resets at epoch {reset_ts})" if reset_ts else ""
                print_plain(
                    f"  [warning]GitHub API rate limit exceeded{reset_msg}. "
                    "Set GITHUB_TOKEN or sign in with 'gh auth login' for higher limits.[/warning]",
                    force=True,
                )

    @staticmethod
    def is_github(target: str) -> bool:
        t = target.strip().lower()
        return (
            t.startswith(("gh:", "github:"))
            or "github.com" in t
            or "raw.githubusercontent.com" in t
            or "gist.github.com" in t
        )

    @staticmethod
    def parse_github_target(target: str) -> dict[str, Any]:
        """Classify GitHub target into: asset, release, blob, tree, repo, gist."""
        t = target.strip()

        if t.lower().startswith(("gh:", "github:")):
            parts = [p for p in t.split(":", 1)[1].strip().split("/") if p]
            if len(parts) >= 2:
                owner, repo = parts[0], parts[1]
                return {
                    "type": "repo",
                    "owner": owner,
                    "repo": repo,
                    "url": f"https://github.com/{owner}/{repo}",
                }

        if "gist.github.com" in t:
            m = re.search(r"gist\.github\.com/([^/]+)/([a-f0-9]+)", t)
            if m:
                return {"type": "gist", "user": m.group(1), "gist_id": m.group(2), "url": t}

        m = re.search(r"github\.com/([^/]+)/([^/]+)/releases/download/([^/]+)/(.+)", t)
        if m:
            owner, repo, tag, fname = m.groups()
            fname_clean = fname.split("?", 1)[0]
            return {
                "type": "asset",
                "owner": owner,
                "repo": repo,
                "tag": tag,
                "filename": fname_clean,
                "url": t,
            }

        m = re.search(r"github\.com/([^/]+)/([^/]+)/releases(?:/tag/([^/?#]+)|/latest)?(?:/)?(?:\?[^#]*)?(?:#.*)?$", t)
        if m:
            owner, repo, tag = m.groups()
            return {
                "type": "release",
                "owner": owner,
                "repo": repo,
                "tag": tag or "latest",
                "url": t,
            }

        m = re.search(r"github\.com/([^/]+)/([^/]+)/blob/([^/]+)/(.+)", t)
        if m:
            owner, repo, branch, path = m.groups()
            raw_url = f"https://raw.githubusercontent.com/{owner}/{repo}/{branch}/{path}"
            return {
                "type": "blob",
                "owner": owner,
                "repo": repo,
                "branch": branch,
                "path": path,
                "url": raw_url,
                "filename": os.path.basename(path.split("?", 1)[0]),
            }

        m = re.search(r"raw\.githubusercontent\.com/([^/]+)/([^/]+)/([^/]+)/(.+)", t)
        if m:
            owner, repo, branch, path = m.groups()
            return {
                "type": "blob",
                "owner": owner,
                "repo": repo,
                "branch": branch,
                "path": path,
                "url": t,
                "filename": os.path.basename(path.split("?", 1)[0]),
            }

        m = re.search(r"github\.com/([^/]+)/([^/]+)/tree/([^/]+)/(.+)", t)
        if m:
            owner, repo, branch, path = m.groups()
            return {
                "type": "tree",
                "owner": owner,
                "repo": repo,
                "branch": branch,
                "path": path,
                "url": t,
            }

        m = re.search(r"github\.com/([^/]+)/([^/?#]+?)(?:\.git)?(?:/)?(?:\?[^#]*)?(?:#.*)?$", t)
        if m:
            owner, repo = m.groups()
            return {
                "type": "repo",
                "owner": owner,
                "repo": repo,
                "url": f"https://github.com/{owner}/{repo}",
            }

        return {"type": "generic", "url": t}

    @classmethod
    def score_asset_for_host(cls, filename: str, *, install_mode: bool = False) -> int:
        """Score release asset filename for current host OS (macOS or Windows, ARM64 or x86_64 priority)."""
        fn = filename.lower()
        if any(
            fn.endswith(ext) for ext in (".sha256", ".sha256sum", ".sha512", ".md5", ".sig", ".asc", ".sbom", ".txt")
        ):
            return 0

        score = 0
        sys_plat = platform.system().lower()
        machine = platform.machine().lower()
        is_arm = machine in ("arm64", "aarch64")

        if sys_plat == "windows":
            is_win = any(x in fn for x in ("windows", "win64", "win32", "pc-windows", "msvc", ".exe", ".msi"))
            if is_win:
                score += 40
                if fn.endswith(".zip"):
                    score += 25
                elif fn.endswith(".exe"):
                    score += 20 if not install_mode else 15
                elif fn.endswith(".msi"):
                    score += 15 if not install_mode else 0
                elif fn.endswith((".tar.gz", ".tgz")):
                    score += 10

                if is_arm:
                    if any(x in fn for x in ("arm64", "aarch64")):
                        score += 35
                    elif any(x in fn for x in ("x86_64", "x64", "amd64", "win64")):
                        score += 5
                else:
                    if any(x in fn for x in ("x86_64", "x64", "amd64", "win64", "msvc")):
                        score += 35
                    elif any(x in fn for x in ("i686", "x86", "win32")):
                        score += 15
            else:
                if any(x in fn for x in ("darwin", "macos", "apple", "linux", ".deb", ".rpm", ".dmg", ".pkg")):
                    score = 0
                elif fn.endswith(".zip"):
                    score += 10
            return score

        is_mac = any(x in fn for x in ("darwin", "macos", "apple", "osx", ".dmg", ".pkg"))
        if is_mac:
            score += 40
            if fn.endswith(".dmg"):
                score += 25
            elif fn.endswith(".pkg"):
                score += 20
            elif fn.endswith((".tar.gz", ".tgz", ".zip", ".tar.xz")):
                score += 15

            if is_arm:
                if any(x in fn for x in ("arm64", "aarch64", "apple-darwin")):
                    score += 35
                elif "universal" in fn:
                    score += 30
                elif any(x in fn for x in ("x86_64", "x64", "amd64")):
                    score += 5
            else:
                if any(x in fn for x in ("x86_64", "x64", "amd64")):
                    score += 35
                elif "universal" in fn:
                    score += 30
        else:
            if any(
                x in fn for x in ("linux", "windows", "win32", "win64", ".exe", ".msi", ".deb", ".rpm", ".apk", "msvc")
            ):
                score = 0
            elif fn.endswith((".zip", ".tar.gz", ".tgz")):
                score += 5

        return score

    score_release_asset = score_asset_for_host

    @classmethod
    def fetch_release_info(cls, owner: str, repo: str, tag: str = "latest") -> dict[str, Any] | None:
        url = (
            f"https://api.github.com/repos/{owner}/{repo}/releases/latest"
            if tag in ("latest", None, "")
            else f"https://api.github.com/repos/{owner}/{repo}/releases/tags/{tag}"
        )
        data, hdrs, status = fetch_json_with_etag(url, headers=cls.get_headers(), timeout=6.0)
        cls._check_rate_limit(hdrs, status)
        if status == 200 and isinstance(data, dict):
            return data
        return None

    @staticmethod
    def extract_asset_digest(asset: dict[str, Any]) -> str | None:
        """Extract SHA-256 digest from GitHub asset object ('digest': 'sha256:...')."""
        digest = asset.get("digest")
        if isinstance(digest, str) and digest.strip():
            return digest.strip()
        return None

    @classmethod
    def find_release_companion_checksum(
        cls,
        assets: list[dict[str, Any]],
        target_asset_name: str,
    ) -> str | None:
        """Look for a companion checksum file in the release assets and parse the digest for target_asset_name."""
        target_lower = target_asset_name.lower()
        checksum_assets: list[dict[str, Any]] = []

        for a in assets:
            aname = a.get("name", "")
            alow = aname.lower()
            if alow in (
                f"{target_lower}.sha256",
                f"{target_lower}.sha256sum",
                f"{target_lower}.sha512",
                "sha256sums",
                "sha256sums.txt",
                "checksums.txt",
                "shasums256.txt",
            ) or alow.endswith((".sha256", ".sha256sum", "checksums.txt", "sha256sums.txt")):
                checksum_assets.append(a)

        for ca in checksum_assets:
            dl_url = ca.get("browser_download_url")
            if not dl_url:
                continue
            try:
                resp = request_with_retry("GET", dl_url, headers=cls.get_download_headers(), timeout=5.0)
                if resp.status_code == 200 and resp.text:
                    parsed = ChecksumEngine.parse_checksum_text(resp.text, target_filename=target_asset_name)
                    if not parsed and ca.get("name", "").lower().startswith(target_lower):
                        parsed = ChecksumEngine.parse_checksum_text(resp.text, target_filename=None)
                    if parsed:
                        algo, hex_dig = parsed
                        return f"{algo}:{hex_dig}"
            except Exception as exc:
                logger.debug("Failed to fetch companion checksum asset %s: %s", dl_url, exc)
        return None

    @classmethod
    def download_release_asset(
        cls,
        owner: str,
        repo: str,
        tag: str,
        out_dir: Path,
        connections: int,
        asset_pattern: str | None = None,
        download_all: bool = False,
        interactive: bool = False,
        limit_rate: str | None = None,
        anti_throttle: str | None = "4MBps",
        grace_period: int = 5,
        watchdog_mode: WatchdogMode | str = WatchdogMode.V2,
        install_mode: bool = False,
    ) -> tuple[TransferResult, dict[str, Any] | None, list[dict[str, Any]]]:
        """Download GitHub release asset(s). Returns (TransferResult, chosen_asset_dict, all_assets)."""
        safe_conn = min(max(int(connections), 1), 16)
        print_plain(f"  [dim]•[/dim] [white]GitHub Release:[/white] [cyan]{owner}/{repo}[/cyan] [dim]({tag})[/dim]")
        info = cls.fetch_release_info(owner, repo, tag)
        if not info:
            msg = f"Release not found for {owner}/{repo}"
            print_plain(f"  [error]✗ {msg}[/error]", force=True)
            return TransferResult(success=False, error=msg, exit_code=int(ExitCode.FAILURE)), None, []

        assets: list[dict[str, Any]] = info.get("assets", [])
        tag_name = info.get("tag_name", tag)
        dl_headers = cls.get_download_headers()

        if not assets:
            tarball_url = (
                info.get("tarball_url") or f"https://github.com/{owner}/{repo}/archive/refs/tags/{tag_name}.tar.gz"
            )
            fname = f"{repo}-{tag_name}.tar.gz"
            print_plain(f"  [dim]• Source archive: {fname}[/dim]")
            t_info = TargetInfo(
                raw=tarball_url,
                target_type=TargetType.DIRECT,
                clean_url=tarball_url,
                filename_hint=fname,
                headers=dl_headers,
            )
            res = Aria2Engine.download(
                t_info,
                out_dir,
                connections=safe_conn,
                limit_rate=limit_rate,
                anti_throttle=anti_throttle,
                grace_period=grace_period,
                watchdog_mode=watchdog_mode,
            )
            return res, None, []

        # Phase 2: Multi-file download-all routed through Aria2 RPC engine with global connection budget
        if download_all:
            print_plain(
                f"  [dim]• Downloading {len(assets)} release assets via Aria2 RPC ({safe_conn} global conns)...[/dim]"
            )
            batch_items = [
                DownloadItem(
                    url=a["browser_download_url"],
                    out_dir=out_dir,
                    filename=a["name"],
                    headers=dl_headers,
                    expected_length=a.get("size"),
                    expected_digest=cls.extract_asset_digest(a),
                )
                for a in assets
            ]
            res = Aria2Engine.download_batch(
                items=batch_items,
                out_dir=out_dir,
                connections=safe_conn,
                limit_rate=limit_rate,
                anti_throttle=anti_throttle,
                grace_period=grace_period,
                watchdog_mode=watchdog_mode,
            )
            return res, None, assets

        candidate_assets = assets
        if asset_pattern:
            candidate_assets = [a for a in assets if _matches_pattern(a["name"], asset_pattern)]

        if not candidate_assets:
            print_plain(f"  [warning]No assets matched '{asset_pattern}'. Showing all:[/warning]")
            candidate_assets = assets

        scored = sorted(
            [(cls.score_asset_for_host(a["name"], install_mode=install_mode), a) for a in candidate_assets],
            key=lambda x: x[0],
            reverse=True,
        )

        chosen_asset: dict[str, Any]
        if not interactive and scored and scored[0][0] >= 40:
            chosen_asset = scored[0][1]
            print_plain(
                f"  [green]✓[/green] [white]Auto-selected:[/white] [bold cyan]{chosen_asset['name']}[/bold cyan] "
                f"[dim]({format_size(chosen_asset.get('size'))})[/dim]"
            )
        else:
            if not UI.json_mode and not UI.quiet:
                from rich import box
                from rich.table import Table

                table = Table(
                    title=f"Release Assets: {owner}/{repo} ({tag_name})",
                    box=box.HORIZONTALS,
                    pad_edge=False,
                )
                table.add_column("#", justify="right", style="cyan")
                table.add_column("Asset", style="bold white")
                table.add_column("Size", justify="right", style="dim")
                table.add_column("Match", justify="center")

                for idx, (sc, a) in enumerate(scored, 1):
                    badge = (
                        "[green]Recommended[/green]"
                        if sc >= 60
                        else ("[cyan]Compatible[/cyan]" if sc >= 30 else "[dim]-[/dim]")
                    )
                    table.add_row(str(idx), a["name"], format_size(a.get("size")), badge)

                UI.print()
                UI.print_renderable(table)
                UI.print()

            if UI.can_prompt(select_requested=interactive) and len(scored) > 1:
                choice = UI.ask_choice(
                    "  Select asset number",
                    choices=[str(i) for i in range(1, len(scored) + 1)],
                    default="1",
                )
                chosen_asset = scored[int(choice) - 1][1]
            else:
                chosen_asset = scored[0][1]

        target_url = chosen_asset["browser_download_url"]
        fname = chosen_asset["name"]
        asset_digest = cls.extract_asset_digest(chosen_asset)
        t_info = TargetInfo(
            raw=target_url,
            target_type=TargetType.DIRECT,
            clean_url=target_url,
            filename_hint=fname,
            content_length=chosen_asset.get("size"),
            expected_digest=asset_digest,
            headers=dl_headers,
        )
        res = Aria2Engine.download(
            t_info,
            out_dir,
            connections=safe_conn,
            limit_rate=limit_rate,
            anti_throttle=anti_throttle,
            grace_period=grace_period,
            watchdog_mode=watchdog_mode,
        )
        return res, chosen_asset, assets

    @classmethod
    def download_tree_directory(
        cls,
        owner: str,
        repo: str,
        branch: str,
        path: str,
        out_dir: Path,
        connections: int,
        limit_rate: str | None = None,
        anti_throttle: str | None = "4MBps",
        grace_period: int = 5,
        watchdog_mode: WatchdogMode | str = WatchdogMode.V2,
    ) -> TransferResult:
        safe_conn = min(max(int(connections), 1), 16)
        print_plain(
            f"  [dim]•[/dim] [white]GitHub Directory:[/white] [cyan]{path}[/cyan] [dim]({owner}/{repo} @ {branch})[/dim]"
        )
        target_dir = out_dir / os.path.basename(path.rstrip("/"))
        target_dir.mkdir(parents=True, exist_ok=True)

        api_url = f"https://api.github.com/repos/{owner}/{repo}/contents/{path}?ref={branch}"
        headers = cls.get_headers()
        dl_headers = cls.get_download_headers()

        batch_items: list[DownloadItem] = []

        def crawl(url: str, current_local: Path) -> None:
            data, hdrs, status = fetch_json_with_etag(url, headers=headers, timeout=6.0)
            cls._check_rate_limit(hdrs, status)
            if status != 200 or not isinstance(data, list):
                return
            for it in data:
                if not isinstance(it, dict):
                    continue
                if it.get("type") == "file" and it.get("download_url"):
                    batch_items.append(
                        DownloadItem(
                            url=it["download_url"],
                            out_dir=current_local,
                            filename=it["name"],
                            headers=dl_headers,
                            expected_length=it.get("size"),
                        )
                    )
                elif it.get("type") == "dir" and it.get("url"):
                    sub_dir = current_local / it["name"]
                    sub_dir.mkdir(parents=True, exist_ok=True)
                    crawl(it["url"], sub_dir)

        crawl(api_url, target_dir)

        if not batch_items:
            msg = f"No files found in directory {path}"
            print_plain(f"  [error]✗ {msg}[/error]", force=True)
            return TransferResult(success=False, error=msg, exit_code=int(ExitCode.FAILURE))

        print_plain(f"  [dim]• Downloading {len(batch_items)} files via Aria2 RPC ({safe_conn} global conns)...[/dim]")
        res = Aria2Engine.download_batch(
            items=batch_items,
            out_dir=target_dir,
            connections=safe_conn,
            limit_rate=limit_rate,
            anti_throttle=anti_throttle,
            grace_period=grace_period,
            watchdog_mode=watchdog_mode,
        )
        res.file_path = target_dir
        return res

    @classmethod
    def download_repo_archive(
        cls,
        owner: str,
        repo: str,
        out_dir: Path,
        connections: int,
        branch: str | None = None,
        limit_rate: str | None = None,
        anti_throttle: str | None = "4MBps",
        grace_period: int = 5,
        watchdog_mode: WatchdogMode | str = WatchdogMode.V2,
    ) -> TransferResult:
        safe_conn = min(max(int(connections), 1), 16)
        print_plain(f"  [dim]•[/dim] [white]GitHub Archive:[/white] [cyan]{owner}/{repo}[/cyan]")
        chosen_branch = branch
        if not chosen_branch:
            data, hdrs, status = fetch_json_with_etag(
                f"https://api.github.com/repos/{owner}/{repo}",
                headers=cls.get_headers(),
                timeout=5.0,
            )
            cls._check_rate_limit(hdrs, status)
            if status == 200 and isinstance(data, dict):
                chosen_branch = data.get("default_branch", "main")
            else:
                chosen_branch = "main"

        archive_url = f"https://github.com/{owner}/{repo}/archive/refs/heads/{chosen_branch}.zip"
        fname = f"{repo}-{chosen_branch}.zip"
        t_info = TargetInfo(
            raw=archive_url,
            target_type=TargetType.DIRECT,
            clean_url=archive_url,
            filename_hint=fname,
            headers=cls.get_download_headers(),
        )
        return Aria2Engine.download(
            t_info,
            out_dir,
            connections=safe_conn,
            limit_rate=limit_rate,
            anti_throttle=anti_throttle,
            grace_period=grace_period,
            watchdog_mode=watchdog_mode,
        )

    def execute(
        self,
        target_info: TargetInfo,
        out_dir: Path,
        *,
        connections: int = 16,
        **options: Any,
    ) -> TransferResult:
        meta = target_info.github_meta or self.parse_github_target(target_info.raw)
        mtype = meta.get("type")
        common_opts = {
            "limit_rate": options.get("limit_rate"),
            "anti_throttle": options.get("anti_throttle", "4MBps"),
            "grace_period": options.get("grace_period", 5),
            "watchdog_mode": options.get("watchdog_mode", WatchdogMode.V2),
        }

        if mtype == "release":
            res, _, _ = self.download_release_asset(
                owner=meta["owner"],
                repo=meta["repo"],
                tag=meta["tag"],
                out_dir=out_dir,
                connections=connections,
                asset_pattern=options.get("asset"),
                download_all=bool(options.get("all_assets", False)),
                interactive=bool(options.get("select", False)),
                **common_opts,
            )
            return res

        if mtype == "tree":
            return self.download_tree_directory(
                owner=meta["owner"],
                repo=meta["repo"],
                branch=meta["branch"],
                path=meta["path"],
                out_dir=out_dir,
                connections=connections,
                **common_opts,
            )

        if mtype == "repo":
            rel = self.fetch_release_info(meta["owner"], meta["repo"])
            if rel and rel.get("assets") and not options.get("source"):
                res, _, _ = self.download_release_asset(
                    owner=meta["owner"],
                    repo=meta["repo"],
                    tag=rel.get("tag_name", "latest"),
                    out_dir=out_dir,
                    connections=connections,
                    asset_pattern=options.get("asset"),
                    download_all=bool(options.get("all_assets", False)),
                    interactive=bool(options.get("select", False)),
                    **common_opts,
                )
                return res
            return self.download_repo_archive(
                owner=meta["owner"],
                repo=meta["repo"],
                out_dir=out_dir,
                connections=connections,
                branch=options.get("branch"),
                **common_opts,
            )

        return Aria2Engine.download(target_info, out_dir, connections=connections, **common_opts)


class BinaryInstaller:
    """Downloads, verifies SHA-256, extracts, and installs GitHub release binaries."""

    @classmethod
    def install(
        cls,
        target: str,
        bin_dir: Path,
        *,
        assume_yes: bool = False,
        connections: int = 16,
        sign: bool = False,
        write_xattr: bool = True,
    ) -> TransferResult:
        bin_dir = Path(bin_dir).expanduser().resolve()
        bin_dir.mkdir(parents=True, exist_ok=True)

        meta = GitHubEngine.parse_github_target(target)
        owner = meta.get("owner")
        repo = meta.get("repo")

        if not owner or not repo:
            if "/" in target and not target.startswith("http"):
                parts = target.split("/", 1)
                owner, repo = parts[0].strip(), parts[1].strip()
            else:
                msg = f"Invalid GitHub repository format for install: {target}"
                print_plain(f"  [error]{msg}[/error]", force=True)
                return TransferResult(success=False, error=msg, exit_code=int(ExitCode.USAGE))

        print_plain(f"  [bold white]Installing:[/bold white] [cyan]{owner}/{repo}[/cyan] -> [dim]{bin_dir}[/dim]")

        with tempfile.TemporaryDirectory(prefix="pdl_install_") as tmp_dir:
            tmp_path = Path(tmp_dir)
            dl_res, chosen_asset, all_assets = GitHubEngine.download_release_asset(
                owner=owner,
                repo=repo,
                tag="latest",
                out_dir=tmp_path,
                connections=min(max(connections, 1), 16),
                interactive=False,
                install_mode=True,
            )

            if not dl_res.success or not dl_res.file_path or not dl_res.file_path.exists():
                msg = dl_res.error or "Failed to download release asset."
                print_plain(f"  [error]{msg}[/error]", force=True)
                return TransferResult(
                    success=False,
                    error=msg,
                    exit_code=dl_res.exit_code or int(ExitCode.FAILURE),
                )

            downloaded_asset = dl_res.file_path

            # Phase 0 #3: Verify SHA-256 against release asset digest field if present,
            # else a companion checksum file. If neither exists, warn clearly and require --yes.
            # Never run the downloaded binary before verification.
            expected_digest: str | None = None
            digest_source = "asset_digest"

            if chosen_asset:
                expected_digest = GitHubEngine.extract_asset_digest(chosen_asset)
                if not expected_digest and all_assets:
                    expected_digest = GitHubEngine.find_release_companion_checksum(all_assets, chosen_asset["name"])
                    if expected_digest:
                        digest_source = "companion_checksum"

            verified_by_pdl = False
            if expected_digest:
                if not dl_res.checksum_verified:
                    matched, algo, computed = ChecksumEngine.verify(downloaded_asset, expected_digest)
                    UI.emit_event(
                        "verification",
                        file=str(downloaded_asset),
                        algorithm=algo,
                        expected=expected_digest,
                        computed=computed,
                        verified=matched,
                        source=digest_source,
                    )
                    if not matched:
                        msg = (
                            f"SHA-256 verification failed for {downloaded_asset.name}: "
                            f"expected {expected_digest}, got {computed}. Aborting install."
                        )
                        print_plain(f"  [error]✗ {msg}[/error]", force=True)
                        return TransferResult(
                            success=False,
                            error=msg,
                            checksum_verified=False,
                            checksum_algo=algo,
                            checksum_digest=computed,
                            exit_code=int(ExitCode.VERIFICATION_FAILED),
                        )
                    print_plain(
                        f"  [green]✓ Verified release asset checksum ({algo.upper()} via {digest_source})[/green]"
                    )
                    verified_by_pdl = True
                else:
                    matched = True
                    verified_by_pdl = True
            else:
                if not assume_yes:
                    msg = (
                        f"No release asset digest or companion checksum file found for '{downloaded_asset.name}'. "
                        "Refusing to install unverified binary without --yes."
                    )
                    print_plain(f"  [warning]⚠ {msg}[/warning]", force=True)
                    UI.emit_event(
                        "verification",
                        file=str(downloaded_asset),
                        verified=False,
                        source="missing",
                        error=msg,
                    )
                    return TransferResult(
                        success=False,
                        error=msg,
                        checksum_verified=False,
                        exit_code=int(ExitCode.VERIFICATION_FAILED),
                    )
                print_plain(
                    f"  [warning]⚠ No checksum published for '{downloaded_asset.name}'; "
                    "proceeding because --yes was specified.[/warning]",
                    force=True,
                )

            extract_dir = tmp_path / "extracted"
            extract_dir.mkdir()

            if ArchiveExtractor.is_archive(downloaded_asset):
                succ, _, _extracted_files = ArchiveExtractor.extract(downloaded_asset, extract_dir)
                if not succ:
                    return TransferResult(
                        success=False,
                        error="Archive extraction failed during binary install",
                        exit_code=int(ExitCode.FAILURE),
                    )
                search_root = extract_dir
            else:
                search_root = tmp_path

            candidate_binaries: list[Path] = []
            skip_exts = (
                ".md",
                ".txt",
                ".json",
                ".yaml",
                ".yml",
                ".sha256",
                ".sig",
                ".asc",
                ".1",
                ".fish",
                ".zsh",
                ".bash",
                ".ps1",
            )
            for root, _, files in os.walk(search_root):
                for f in files:
                    if f.startswith(".") or f.lower().endswith(skip_exts):
                        continue
                    fp = Path(root) / f
                    if fp == downloaded_asset and ArchiveExtractor.is_archive(downloaded_asset):
                        continue
                    is_exec = _is_binary_executable(fp)
                    matches_name = f.lower() in (repo.lower(), repo.lower().replace("-cli", ""))
                    if is_exec or matches_name or "." not in f:
                        candidate_binaries.append(fp)

            if not candidate_binaries:
                msg = "No executable binary found in package."
                print_plain(f"  [error]{msg}[/error]", force=True)
                return TransferResult(success=False, error=msg, exit_code=int(ExitCode.FAILURE))

            candidate_binaries.sort(
                key=lambda p: (
                    0 if p.name.lower() == repo.lower() else 1,
                    0 if _is_binary_executable(p) else 1,
                )
            )
            target_bin = candidate_binaries[0]

            dest_bin = bin_dir / target_bin.name
            shutil.copy2(target_bin, dest_bin)
            dest_bin.chmod(0o755)

            # C2: Codesign verification & Gatekeeper / Authenticode assessment
            if pdl_platform.is_windows():
                cs_valid = False
                cs_detail = "unsigned"
                try:
                    ps_cmd = f"(Get-AuthenticodeSignature -LiteralPath '{dest_bin}').Status.ToString()"
                    proc = subprocess.run(
                        ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", ps_cmd],
                        capture_output=True,
                        text=True,
                        check=False,
                        timeout=5.0,
                    )
                    if proc.returncode == 0 and proc.stdout.strip():
                        auth_status = proc.stdout.strip()
                        cs_valid = auth_status.lower() == "valid"
                        cs_detail = f"Authenticode: {auth_status}"
                except Exception:
                    cs_detail = "Authenticode: check skipped"
                spctl_ok = cs_valid
                spctl_detail = "Windows SmartScreen / Authenticode"
            else:
                cs_valid, cs_detail = macos.codesign_verify(dest_bin)
                spctl_ok, spctl_detail = macos.spctl_assess(dest_bin)

            UI.emit_event(
                "install_security_check",
                file=str(dest_bin),
                codesign_valid=cs_valid,
                codesign_detail=cs_detail,
                gatekeeper_accepted=spctl_ok,
                gatekeeper_detail=spctl_detail,
                verified_by_pdl=verified_by_pdl,
            )

            cs_style = "green" if cs_valid else "yellow"
            spctl_style = "green" if spctl_ok else "yellow"
            print_plain(f"  [dim]•[/dim] Codesign: [{cs_style}]{cs_detail}[/{cs_style}]")
            print_plain(f"  [dim]•[/dim] Gatekeeper (spctl): [{spctl_style}]{spctl_detail}[/{spctl_style}]")

            # Offer ad-hoc signing ("codesign --force --sign -") only on macOS with an explicit --sign flag
            if sign:
                if pdl_platform.is_windows():
                    print_plain("  [dim]• Ad-hoc signing is only applicable on macOS.[/dim]")
                else:
                    sign_ok, sign_detail = macos.codesign_adhoc_sign(dest_bin)
                    if sign_ok:
                        print_plain("  [green]✓ Applied ad-hoc code signature ('codesign --force --sign -')[/green]")
                    else:
                        print_plain(f"  [warning]⚠ Ad-hoc signing warning: {sign_detail}[/warning]")
            else:
                if not cs_valid or not spctl_ok:
                    if pdl_platform.is_macos():
                        print_plain("    [dim]Tip: Pass '--sign' to 'pdl install' to apply an ad-hoc signature.[/dim]")

            # Remove the quarantine attribute ONLY for files pdl itself verified
            if verified_by_pdl:
                pdl_platform.remove_quarantine(dest_bin)
                print_plain("  [green]✓ Removed quarantine attribute (verified by pdl)[/green]")
            else:
                print_plain(
                    "  [yellow]⚠ Preserved quarantine attribute (not cryptographically verified by pdl)[/yellow]"
                )

            # C2: Origin tracking (Zone.Identifier on Windows, kMDItemWhereFroms on macOS)
            if write_xattr:
                dl_url = chosen_asset.get("browser_download_url") if chosen_asset else None
                if dl_url:
                    pdl_platform.set_where_froms(dest_bin, [dl_url])

            # Safe post-verification version check
            ver_text = ""
            try:
                proc = subprocess.run(
                    [str(dest_bin), "--version"],
                    capture_output=True,
                    text=True,
                    timeout=2.0,
                    check=False,
                )
                if proc.stdout:
                    ver_text = f" [dim]({proc.stdout.strip().splitlines()[0]})[/dim]"
            except (OSError, subprocess.SubprocessError) as exc:
                logger.debug("Post-install --version check skipped: %s", exc)

            print_plain(
                f"  [green]✓[/green] [bold white]Successfully installed [cyan]{dest_bin.name}[/cyan]![/bold white]{ver_text}"
            )
            print_plain(f"    [dim]Binary Location:[/dim] [cyan]{dest_bin}[/cyan]")
            return TransferResult(
                success=True,
                file_path=dest_bin,
                bytes_downloaded=dl_res.bytes_downloaded,
                duration=dl_res.duration,
                checksum_verified=True if expected_digest else None,
                exit_code=int(ExitCode.OK),
            )
