"""Homebrew package / bottle resolution, multi-stream download, and installation engine."""

from __future__ import annotations

import logging
import os
import platform
import re
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

from pdl.models import ExitCode, TargetInfo, TargetType, TransferResult, WatchdogMode, format_size
from pdl.net import fetch_json_with_etag, get_http_client
from pdl.postproc.checksum import ChecksumEngine
from pdl.ui import UI, print_plain

logger = logging.getLogger(__name__)


class BrewEngine:
    """Resolves Homebrew bottles via formulae.brew.sh & GHCR, accelerates downloads with 16 streams."""

    @classmethod
    def is_brew(cls, target: str) -> bool:
        """Return True if target represents a Homebrew formula or formula URL."""
        t = target.strip().lower()
        if t.startswith(("brew:", "brew/", "homebrew:", "homebrew/")):
            return True
        if "formulae.brew.sh/formula/" in t or "formulae.brew.sh/api/formula/" in t:
            return True
        return False

    @classmethod
    def parse_brew_target(cls, target: str) -> str:
        """Extract formula name from target string.

        Examples:
            'brew:wget' -> 'wget'
            'homebrew:jq' -> 'jq'
            'https://formulae.brew.sh/formula/ffmpeg' -> 'ffmpeg'
        """
        t = target.strip()
        if t.lower().startswith(("brew:", "brew/", "homebrew:", "homebrew/")):
            t = t.split(":", 1)[-1].split("/", 1)[-1].strip()
        elif "formulae.brew.sh/formula/" in t:
            parsed = urlparse(t)
            parts = [p for p in parsed.path.split("/") if p and p != "formula"]
            return parts[0] if parts else t
        elif "formulae.brew.sh/api/formula/" in t:
            parsed = urlparse(t)
            parts = [p for p in parsed.path.split("/") if p and p != "api" and p != "formula"]
            fname = parts[0] if parts else t
            return fname.replace(".json", "")
        return t

    @classmethod
    def get_brew_cache_path(cls, formula: str) -> Path | None:
        """Query `brew --cache <formula>` to locate exact destination Homebrew expects for the bottle."""
        brew_bin = shutil.which("brew")
        if not brew_bin:
            return None
        try:
            proc = subprocess.run(
                [brew_bin, "--cache", formula],
                capture_output=True,
                text=True,
                timeout=4.0,
                check=False,
            )
            if proc.returncode == 0:
                p = proc.stdout.strip()
                if p:
                    return Path(p)
        except Exception as exc:
            logger.debug("brew --cache %s lookup failed: %s", formula, exc)
        return None

    @classmethod
    def get_fallback_bottle_tags(cls) -> list[str]:
        """Compute prioritized list of matching Homebrew bottle tags for current OS and architecture."""
        tags: list[str] = []
        sys_plat = sys.platform
        mach = platform.machine().lower()
        arch = "arm64" if mach in ("arm64", "aarch64") else "x86_64"

        if sys_plat == "darwin":
            mac_ver = platform.mac_ver()[0]
            try:
                major = int(mac_ver.split(".")[0]) if mac_ver else 15
            except ValueError:
                major = 15

            mac_names = {
                27: ["golden_gate", "tahoe", "sequoia"],
                26: ["tahoe", "sequoia", "sonoma"],
                15: ["sequoia", "sonoma", "ventura"],
                14: ["sonoma", "ventura", "monterey"],
                13: ["ventura", "monterey", "big_sur"],
                12: ["monterey", "big_sur", "catalina"],
            }
            names = mac_names.get(major, ["sequoia", "sonoma", "ventura"])
            for name in names:
                if arch == "arm64":
                    tags.append(f"arm64_{name}")
                else:
                    tags.append(name)
                    tags.append(f"x86_64_{name}")
        elif sys_plat.startswith("linux"):
            tags.extend([f"{arch}_linux", "x86_64_linux"])

        tags.append("all")
        return tags

    @classmethod
    def resolve_ghcr_direct_url(cls, blob_url: str, formula: str) -> tuple[str, dict[str, str], int | None]:
        """Obtain anonymous pull token from GHCR and follow redirect to signed CDN URL."""
        token_url = f"https://ghcr.io/token?service=ghcr.io&scope=repository:homebrew/core/{formula}:pull"
        headers: dict[str, str] = {}
        data, _hdrs, status = fetch_json_with_etag(token_url, timeout=4.5)
        if status == 200 and isinstance(data, dict):
            token = data.get("token")
            if token:
                headers["Authorization"] = f"Bearer {token}"

        if not headers.get("Authorization"):
            return blob_url, {}, None

        # Issue GET request with follow_redirects=False to catch 307 redirect from ghcr.io to pkg-containers.githubusercontent.com
        try:
            client = get_http_client()
            resp = client.get(blob_url, headers=headers, follow_redirects=False, timeout=5.0)
            if resp.status_code in (301, 302, 307, 308):
                loc = resp.headers.get("location")
                if loc:
                    # Direct CDN URL is authenticated via query parameters and requires no Authorization header
                    content_length: int | None = None
                    try:
                        head_resp = client.head(loc, timeout=4.0)
                        cl = head_resp.headers.get("content-length")
                        if cl and cl.isdigit():
                            content_length = int(cl)
                    except Exception:
                        pass
                    return loc, {}, content_length
            elif resp.status_code in (200, 206):
                cl = resp.headers.get("content-length")
                content_length = int(cl) if cl and cl.isdigit() else None
                return blob_url, headers, content_length
        except Exception as exc:
            logger.debug("GHCR redirect resolution failed for %s: %s", blob_url, exc)

        return blob_url, headers, None

    @classmethod
    def resolve(cls, target: str, *, allow_network: bool = True) -> dict[str, Any] | None:
        """Resolve Homebrew formula into bottle metadata, direct CDN URL, and expected SHA-256."""
        formula = cls.parse_brew_target(target)
        if not formula:
            return None

        # Check local brew cache path if available to pinpoint exact tag
        cache_path = cls.get_brew_cache_path(formula)
        detected_tag: str | None = None
        if cache_path:
            # Format: ...--<formula>--<version>.<tag>.bottle...
            m = re.search(r"--" + re.escape(formula) + r"--[0-9A-Za-z\.\_]+\.([a-z0-9\_]+)\.bottle", cache_path.name)
            if m:
                detected_tag = m.group(1)

        api_url = f"https://formulae.brew.sh/api/formula/{formula}.json"
        meta: dict[str, Any] = {
            "formula": formula,
            "version": "",
            "tag": detected_tag or "",
            "filename": f"{formula}.bottle.tar.gz",
            "url": "",
            "filesize": None,
            "sha256": None,
            "desc": "",
            "cache_path": str(cache_path) if cache_path else None,
            "headers": {},
            "dependencies": [],
        }

        if not allow_network:
            return meta

        data, _hdrs, status = fetch_json_with_etag(api_url, timeout=4.5)
        if status != 200 or not isinstance(data, dict):
            logger.debug("Homebrew API lookup failed for %s (status %s)", formula, status)
            return None

        meta["desc"] = data.get("desc", "")
        meta["dependencies"] = [str(d) for d in (data.get("dependencies") or []) if d]
        versions = data.get("versions", {})
        meta["version"] = versions.get("stable", "")

        files = data.get("bottle", {}).get("stable", {}).get("files", {})
        if not files:
            logger.debug("No pre-built bottles available for %s", formula)
            return None

        chosen_tag: str | None = None
        if detected_tag and detected_tag in files:
            chosen_tag = detected_tag
        else:
            cand_tags = cls.get_fallback_bottle_tags()
            for tg in cand_tags:
                if tg in files:
                    chosen_tag = tg
                    break

        if not chosen_tag:
            chosen_tag = list(files.keys())[0]

        bottle_data = files[chosen_tag]
        blob_url = bottle_data.get("url", "")
        sha256 = bottle_data.get("sha256", "")

        meta["tag"] = chosen_tag
        meta["sha256"] = sha256
        meta["filename"] = cache_path.name if cache_path else f"{formula}-{meta['version']}.{chosen_tag}.bottle.tar.gz"

        if blob_url:
            direct_url, auth_hdrs, cl = cls.resolve_ghcr_direct_url(blob_url, formula)
            meta["url"] = direct_url
            meta["headers"] = auth_hdrs
            if cl:
                meta["filesize"] = cl

        return meta

    def execute(
        self,
        target_info: TargetInfo,
        out_dir: Path,
        *,
        connections: int = 16,
        **options: Any,
    ) -> TransferResult:
        """Download resolved Homebrew bottle with 16 parallel channels and verify SHA-256."""
        from pdl.engines.aria2 import Aria2Engine

        meta = target_info.brew_meta or {}
        formula = meta.get("formula", "brew-formula")
        ver = meta.get("version", "")
        tag = meta.get("tag", "")
        desc = meta.get("desc", "")
        safe_conn = min(max(connections, 1), 16)

        print_plain(
            f"  [bold white]Homebrew:[/bold white]        [bold cyan]{formula}[/bold cyan] [green]v{ver}[/green] [dim]({tag})[/dim]"
        )
        if desc:
            print_plain(f"  [dim]Description:[/dim]     [dim]{desc[:80]}[/dim]")
        if meta.get("filesize"):
            print_plain(f"  [dim]File Size:[/dim]       [white]{format_size(meta['filesize'])}[/white]")
        print_plain(f"  [dim]Destination:[/dim]     [dim]{out_dir}[/dim] [dim]({safe_conn} parallel streams)[/dim]")

        res = Aria2Engine.download(
            target_info=target_info,
            out_dir=out_dir,
            connections=safe_conn,
            limit_rate=options.get("limit_rate"),
            anti_throttle=options.get("anti_throttle", "4MBps"),
            grace_period=options.get("grace_period", 5),
            watchdog_mode=options.get("watchdog_mode", WatchdogMode.V2),
            caffeinate=bool(options.get("caffeinate", True)),
            net_watch=bool(options.get("net_watch", True)),
            stall_timeout=float(options.get("stall_timeout", 30.0)),
            require_ac=bool(options.get("require_ac", False)),
            pause_on_low_power=bool(options.get("pause_on_low_power", False)),
        )

        expected_sha = meta.get("sha256")
        if res.success and res.file_path and res.file_path.is_file() and expected_sha:
            print_plain("  [dim]•[/dim] [white]Verifying Homebrew bottle SHA-256 checksum...[/white]")
            matched, algo, digest = ChecksumEngine.verify(res.file_path, f"sha256:{expected_sha}")
            res.checksum_verified = matched
            res.checksum_algo = algo
            res.checksum_digest = digest
            UI.emit_event(
                "verification",
                file=str(res.file_path),
                algorithm=algo,
                expected=expected_sha,
                computed=digest,
                verified=matched,
                source="homebrew_formula",
            )
            if matched:
                print_plain("  [green]✓ Bottle integrity verified against Homebrew catalog (SHA-256)![/green]")
            else:
                print_plain(f"  [error]✗ Checksum mismatch! Expected {expected_sha}, got {digest}[/error]", force=True)
                res.success = False
                res.exit_code = int(ExitCode.VERIFICATION_FAILED)
                res.error = f"Homebrew bottle SHA-256 mismatch: expected {expected_sha}, got {digest}"

        return res

    @classmethod
    def install(
        cls,
        target: str,
        *,
        connections: int = 16,
        assume_yes: bool = False,
    ) -> TransferResult:
        """Download bottle via 16 parallel channels into Homebrew cache, verify SHA-256, and install."""
        from pdl.net import probe_target

        brew_bin = shutil.which("brew")
        if not brew_bin:
            msg = "Homebrew ('brew') binary not found on PATH. Install Homebrew to install packages locally."
            print_plain(f"  [error]{msg}[/error]", force=True)
            return TransferResult(success=False, error=msg, exit_code=int(ExitCode.MISSING_DEPENDENCY))

        t_info = probe_target(target)
        if t_info.target_type != TargetType.BREW or not t_info.clean_url:
            msg = f"Failed to resolve Homebrew formula: {target}"
            print_plain(f"  [error]{msg}[/error]", force=True)
            return TransferResult(success=False, error=msg, exit_code=int(ExitCode.FAILURE))

        meta = t_info.brew_meta or {}
        formula = meta.get("formula", target)
        ver = meta.get("version", "")
        tag = meta.get("tag", "")

        print_plain(
            f"  [dim]•[/dim] [white]Accelerating install for Homebrew formula:[/white] [cyan]{formula}[/cyan] [green]{ver}[/green] [dim]({tag})[/dim]"
        )

        # Determine target cache destination
        cache_path_str = meta.get("cache_path")
        cache_path = Path(cache_path_str) if cache_path_str else cls.get_brew_cache_path(formula)

        if not cache_path:
            # Fallback to standard cache directory
            default_cache = Path.home() / "Library" / "Caches" / "Homebrew" / "downloads"
            default_cache.mkdir(parents=True, exist_ok=True)
            cache_path = (
                default_cache / t_info.filename_hint
                if t_info.filename_hint
                else default_cache / f"{formula}.bottle.tar.gz"
            )

        cache_dir = cache_path.parent
        cache_dir.mkdir(parents=True, exist_ok=True)
        t_info.filename_hint = cache_path.name

        engine = cls()
        dl_res = engine.execute(t_info, cache_dir, connections=connections)

        if not dl_res.success or not dl_res.file_path or not dl_res.file_path.exists():
            msg = dl_res.error or "Failed to download Homebrew bottle."
            print_plain(f"  [error]{msg}[/error]", force=True)
            return dl_res

        # Pre-flight dependencies: check dependencies and pre-download any uninstalled dependency bottles into brew cache
        deps = meta.get("dependencies") or []
        if deps:
            installed_formulas: set[str] = set()
            try:
                list_proc = subprocess.run([brew_bin, "list", "--formula"], capture_output=True, text=True, timeout=5.0, check=False)
                if list_proc.returncode == 0:
                    installed_formulas = {line.strip() for line in list_proc.stdout.splitlines() if line.strip()}
            except Exception:
                pass

            missing_deps = [d for d in deps if d not in installed_formulas]
            if missing_deps:
                dep_label = ", ".join(missing_deps[:6])
                if len(missing_deps) > 6:
                    dep_label += f" (+{len(missing_deps) - 6} more)"
                print_plain(
                    f"  [dim]•[/dim] [white]Pre-fetching {len(missing_deps)} Homebrew dependency bottle(s) with 16 parallel channels:[/white] [cyan]{dep_label}[/cyan]"
                )
                for dep_name in missing_deps:
                    try:
                        dep_target = f"brew:{dep_name}"
                        dep_info = probe_target(dep_target)
                        if dep_info.target_type == TargetType.BREW and dep_info.clean_url:
                            dep_meta = dep_info.brew_meta or {}
                            dep_cache_str = dep_meta.get("cache_path")
                            dep_cache_path = Path(dep_cache_str) if dep_cache_str else cls.get_brew_cache_path(dep_name)
                            if not dep_cache_path:
                                dep_cache_path = default_cache / (dep_info.filename_hint or f"{dep_name}.bottle.tar.gz")
                            if dep_cache_path.exists():
                                continue
                            dep_cache_dir = dep_cache_path.parent
                            dep_cache_dir.mkdir(parents=True, exist_ok=True)
                            dep_info.filename_hint = dep_cache_path.name
                            engine.execute(dep_info, dep_cache_dir, connections=connections)
                    except Exception as dep_exc:
                        logger.debug("Failed to pre-fetch bottle for dependency %s: %s", dep_name, dep_exc)

        # Bottle is now verified in Homebrew's download cache.
        # Run non-interactive brew install
        print_plain("  [dim]•[/dim] [white]Pouring bottle via Homebrew (NONINTERACTIVE=1)...[/white]")
        env = dict(os.environ)
        env["NONINTERACTIVE"] = "1"
        env["HOMEBREW_NO_AUTO_UPDATE"] = "1"

        try:
            proc = subprocess.run(
                [brew_bin, "install", formula],
                env=env,
                capture_output=True,
                text=True,
                check=False,
            )
            if proc.returncode != 0:
                err_msg = proc.stderr.strip() or f"brew install failed with exit code {proc.returncode}"
                print_plain(f"  [error]✗ {err_msg}[/error]", force=True)
                return TransferResult(
                    success=False,
                    error=err_msg,
                    exit_code=int(ExitCode.FAILURE),
                )
            print_plain(f"  [green]✓ Successfully installed {formula} {ver} via Homebrew![/green]")
            return TransferResult(
                success=True,
                file_path=cache_path,
                bytes_downloaded=dl_res.bytes_downloaded,
                checksum_verified=dl_res.checksum_verified,
                checksum_algo=dl_res.checksum_algo,
                checksum_digest=dl_res.checksum_digest,
                exit_code=int(ExitCode.OK),
            )
        except Exception as exc:
            msg = f"Installation error: {exc}"
            print_plain(f"  [error]{msg}[/error]", force=True)
            return TransferResult(success=False, error=msg, exit_code=int(ExitCode.FAILURE))
