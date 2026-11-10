"""Lazy binary discovery, NVM version sorting, and yt-dlp EJS runtime configuration."""

from __future__ import annotations

import importlib.metadata
import importlib.util
import logging
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path
from typing import Any

from paradl.platform.macos import is_macos

logger = logging.getLogger(__name__)


def parse_version_tuple(version_str: str) -> tuple[int, ...]:
    """Parse a version string like 'v22.11.0' or '9.11.2' into a numeric tuple (22, 11, 0)."""
    cleaned = version_str.strip().lstrip("vV")
    parts: list[int] = []
    for part in re.split(r"[^0-9]+", cleaned):
        if part:
            parts.append(int(part))
        else:
            break
    return tuple(parts) if parts else (0,)


class Toolchain:
    """Lazy, cached system binary and JS runtime resolver."""

    def __init__(self, check_fallbacks: bool = True) -> None:
        self._check_fallbacks = check_fallbacks
        self._cache: dict[str, str | None] = {}

    def reset_cache(self) -> None:
        """Clear cached binary paths (useful in tests when PATH is modified)."""
        self._cache.clear()

    def _find_bin(self, name: str, fallback_paths: list[str] | None = None) -> str | None:
        # First check PATH via shutil.which
        found = shutil.which(name)
        if found:
            return found

        if not self._check_fallbacks:
            return None

        is_win = sys.platform == "win32"

        # Also check current Python interpreter's bin/Scripts directory
        parent = Path(sys.executable).resolve().parent
        for cand_dir in [parent, parent / "Scripts"]:
            venv_cand = cand_dir / name
            if is_win and not name.lower().endswith(".exe"):
                venv_cand_exe = cand_dir / f"{name}.exe"
                if venv_cand_exe.exists() and os.access(venv_cand_exe, os.X_OK):
                    return str(venv_cand_exe)
            if venv_cand.exists() and os.access(venv_cand, os.X_OK):
                return str(venv_cand)

        for p in fallback_paths or []:
            if not p:
                continue
            candidates = [p]
            if is_win and not p.lower().endswith(".exe"):
                candidates.append(f"{p}.exe")
            for c in candidates:
                if os.path.exists(c) and os.access(c, os.X_OK):
                    return c
        return None

    @property
    def aria2c(self) -> str | None:
        if "aria2c" not in self._cache:
            self._cache["aria2c"] = self._find_bin(
                "aria2c",
                [
                    "/opt/homebrew/bin/aria2c",
                    "/usr/local/bin/aria2c",
                    "/usr/bin/aria2c",
                    str(Path.home() / "scoop/shims/aria2c.exe"),
                    "C:/ProgramData/chocolatey/bin/aria2c.exe",
                    "C:/Program Files/aria2/aria2c.exe",
                    str(Path.home() / "AppData/Local/Microsoft/WinGet/Links/aria2c.exe"),
                ],
            )
        return self._cache["aria2c"]

    @property
    def ytdlp(self) -> str | None:
        if "ytdlp" not in self._cache:
            self._cache["ytdlp"] = self._find_bin(
                "yt-dlp",
                [
                    str(Path.home() / ".local/bin/yt-dlp"),
                    "/opt/homebrew/bin/yt-dlp",
                    "/usr/local/bin/yt-dlp",
                    str(Path.home() / "scoop/shims/yt-dlp.exe"),
                    "C:/ProgramData/chocolatey/bin/yt-dlp.exe",
                    str(Path.home() / "AppData/Local/Programs/yt-dlp/yt-dlp.exe"),
                ],
            )
        return self._cache["ytdlp"]

    @property
    def afconvert(self) -> str | None:
        if "afconvert" not in self._cache:
            if not is_macos():
                self._cache["afconvert"] = None
            else:
                self._cache["afconvert"] = self._find_bin("afconvert", ["/usr/bin/afconvert"])
        return self._cache["afconvert"]

    @property
    def hdiutil(self) -> str | None:
        if "hdiutil" not in self._cache:
            if not is_macos():
                self._cache["hdiutil"] = None
            else:
                self._cache["hdiutil"] = self._find_bin("hdiutil", ["/usr/bin/hdiutil"])
        return self._cache["hdiutil"]

    @property
    def ffmpeg(self) -> str | None:
        if "ffmpeg" not in self._cache:
            self._cache["ffmpeg"] = self._find_bin(
                "ffmpeg",
                [
                    "/opt/homebrew/bin/ffmpeg",
                    "/usr/local/bin/ffmpeg",
                    "/usr/bin/ffmpeg",
                    str(Path.home() / "scoop/shims/ffmpeg.exe"),
                    "C:/ProgramData/chocolatey/bin/ffmpeg.exe",
                    "C:/Program Files/ffmpeg/bin/ffmpeg.exe",
                    str(Path.home() / "AppData/Local/Microsoft/WinGet/Links/ffmpeg.exe"),
                ],
            )
        return self._cache["ffmpeg"]

    @property
    def adb(self) -> str | None:
        if "adb" not in self._cache:
            self._cache["adb"] = self._find_bin(
                "adb",
                [
                    "/opt/homebrew/bin/adb",
                    "/usr/local/bin/adb",
                    str(Path.home() / "Library/Android/sdk/platform-tools/adb"),
                    str(Path.home() / "AppData/Local/Android/Sdk/platform-tools/adb.exe"),
                ],
            )
        return self._cache["adb"]

    @property
    def p7zip(self) -> str | None:
        if "p7zip" not in self._cache:
            self._cache["p7zip"] = self._find_bin(
                "7z",
                [
                    "/opt/homebrew/bin/7z",
                    "/usr/local/bin/7z",
                    "C:/Program Files/7-Zip/7z.exe",
                    str(Path.home() / "scoop/shims/7z.exe"),
                    "C:/ProgramData/chocolatey/bin/7z.exe",
                ],
            ) or self._find_bin("7za", ["/opt/homebrew/bin/7za", "/usr/local/bin/7za"])
        return self._cache["p7zip"]

    @property
    def curl(self) -> str | None:
        if "curl" not in self._cache:
            self._cache["curl"] = self._find_bin(
                "curl",
                ["/opt/homebrew/opt/curl/bin/curl", "/opt/homebrew/bin/curl", "/usr/bin/curl"],
            )
        return self._cache["curl"]

    @property
    def gh(self) -> str | None:
        if "gh" not in self._cache:
            self._cache["gh"] = self._find_bin("gh", ["/opt/homebrew/bin/gh", "/usr/local/bin/gh"])
        return self._cache["gh"]

    @property
    def deno(self) -> str | None:
        if "deno" not in self._cache:
            self._cache["deno"] = self._find_bin(
                "deno",
                [
                    "/opt/homebrew/bin/deno",
                    "/usr/local/bin/deno",
                    str(Path.home() / ".deno/bin/deno"),
                ],
            )
        return self._cache["deno"]

    @staticmethod
    def find_nvm_nodes(nvm_versions_dir: Path | None = None) -> list[tuple[tuple[int, ...], Path]]:
        """Discover and sort NVM node binaries by numeric version tuple descending (e.g., v22 beats v9)."""
        base = nvm_versions_dir or (Path.home() / ".nvm" / "versions" / "node")
        if not base.exists() or not base.is_dir():
            return []
        candidates: list[tuple[tuple[int, ...], Path]] = []
        for vdir in base.iterdir():
            if not vdir.is_dir() or not vdir.name.lower().startswith("v"):
                continue
            node_bin = vdir / "bin" / "node"
            if node_bin.exists() and os.access(node_bin, os.X_OK):
                ver = parse_version_tuple(vdir.name)
                candidates.append((ver, node_bin))
        candidates.sort(key=lambda item: item[0], reverse=True)
        return candidates

    @staticmethod
    def get_node_version(node_path: str) -> tuple[int, ...]:
        """Get numeric version tuple for a node binary path."""
        m = re.search(r"/versions/node/(v[0-9]+\.[0-9]+\.[0-9]+)/", node_path)
        if m:
            return parse_version_tuple(m.group(1))
        try:
            proc = subprocess.run(
                [node_path, "--version"],
                capture_output=True,
                text=True,
                timeout=2.0,
                check=False,
            )
            if proc.returncode == 0 and proc.stdout.strip():
                return parse_version_tuple(proc.stdout.strip())
        except (OSError, subprocess.SubprocessError) as exc:
            logger.debug("Failed to probe node version for %s: %s", node_path, exc)
        return (0,)

    @property
    def node(self) -> str | None:
        """Return path to node binary (>= 22 preferred and required for EJS)."""
        if "node" not in self._cache:
            candidates: list[str] = []
            which_node = shutil.which("node")
            if which_node:
                candidates.append(which_node)
            if self._check_fallbacks:
                for p in ["/opt/homebrew/bin/node", "/usr/local/bin/node", "/usr/bin/node"]:
                    if os.path.exists(p) and os.access(p, os.X_OK) and p not in candidates:
                        candidates.append(p)
                for _ver, nvm_bin in self.find_nvm_nodes():
                    s_bin = str(nvm_bin)
                    if s_bin not in candidates:
                        candidates.append(s_bin)

            chosen: str | None = None
            for cand in candidates:
                ver = self.get_node_version(cand)
                if ver and ver[0] >= 22:
                    chosen = cand
                    break
            self._cache["node"] = chosen
        return self._cache["node"]

    @staticmethod
    def get_npm_version(npm_path: str) -> str | None:
        """Get version string for an npm binary path (e.g., '12.2.2')."""
        try:
            proc = subprocess.run(
                [npm_path, "--version"],
                capture_output=True,
                text=True,
                timeout=2.5,
                check=False,
            )
            if proc.returncode == 0 and proc.stdout.strip():
                return proc.stdout.strip().splitlines()[0].lstrip("vV")
        except (OSError, subprocess.SubprocessError) as exc:
            logger.debug("Failed to probe npm version for %s: %s", npm_path, exc)
        return None

    @property
    def npm(self) -> str | None:
        """Return path to npm binary."""
        if "npm" not in self._cache:
            candidates: list[str] = []
            which_npm = shutil.which("npm")
            if which_npm:
                candidates.append(which_npm)
            if self._check_fallbacks:
                for p in [
                    "/opt/homebrew/bin/npm",
                    "/usr/local/bin/npm",
                    "/usr/bin/npm",
                    str(Path.home() / ".nvm/current/bin/npm"),
                    str(Path.home() / "AppData/Roaming/npm/npm.cmd"),
                    "C:/Program Files/nodejs/npm.cmd",
                ]:
                    if os.path.exists(p) and os.access(p, os.X_OK) and p not in candidates:
                        candidates.append(p)
                for _ver, nvm_bin in self.find_nvm_nodes():
                    cand_npm = nvm_bin.parent / ("npm.cmd" if sys.platform == "win32" else "npm")
                    s_bin = str(cand_npm)
                    if (
                        cand_npm.exists()
                        and os.access(cand_npm, os.X_OK)
                        and s_bin not in candidates
                    ):
                        candidates.append(s_bin)

            chosen: str | None = None
            for cand in candidates:
                if os.path.exists(cand) and os.access(cand, os.X_OK):
                    chosen = cand
                    break
            self._cache["npm"] = chosen
        return self._cache["npm"]

    @property
    def npm_version(self) -> str | None:
        """Return installed npm version if available."""
        if "npm_version" not in self._cache:
            ver: str | None = None
            if self.npm:
                ver = self.get_npm_version(self.npm)
            self._cache["npm_version"] = ver
        return self._cache["npm_version"]

    @property
    def quickjs(self) -> str | None:
        if "quickjs" not in self._cache:
            found = (
                self._find_bin("qjs", ["/opt/homebrew/bin/qjs", "/usr/local/bin/qjs"])
                or self._find_bin("qjs-ng", ["/opt/homebrew/bin/qjs-ng", "/usr/local/bin/qjs-ng"])
                or self._find_bin(
                    "quickjs", ["/opt/homebrew/bin/quickjs", "/usr/local/bin/quickjs"]
                )
            )
            self._cache["quickjs"] = found
        return self._cache["quickjs"]

    @property
    def bun(self) -> str | None:
        """Deprecated fallback JS runtime."""
        if "bun" not in self._cache:
            self._cache["bun"] = self._find_bin(
                "bun",
                [
                    "/opt/homebrew/bin/bun",
                    "/usr/local/bin/bun",
                    str(Path.home() / ".bun/bin/bun"),
                ],
            )
        return self._cache["bun"]

    @property
    def yt_dlp_ejs_version(self) -> str | None:
        """Return installed yt-dlp-ejs package version if available."""
        if "yt_dlp_ejs_version" not in self._cache:
            ver: str | None = None
            for pkg_name in ("yt-dlp-ejs", "yt_dlp_ejs"):
                try:
                    ver = importlib.metadata.version(pkg_name)
                    break
                except importlib.metadata.PackageNotFoundError:
                    continue
            if ver is None and importlib.util.find_spec("yt_dlp_ejs") is not None:
                ver = "installed"
            if ver is None and self.npm:
                try:
                    proc = subprocess.run(
                        [self.npm, "list", "-g", "yt-dlp-ejs", "--depth=0", "--json"],
                        capture_output=True,
                        text=True,
                        timeout=2.0,
                        check=False,
                    )
                    if proc.returncode == 0 and proc.stdout.strip():
                        import json as _json

                        npm_pkg_data = _json.loads(proc.stdout)
                        pkg_info = npm_pkg_data.get("dependencies", {}).get("yt-dlp-ejs")
                        if pkg_info and "version" in pkg_info:
                            ver = pkg_info["version"]
                except Exception:
                    pass
            self._cache["yt_dlp_ejs_version"] = ver
        return self._cache["yt_dlp_ejs_version"]

    def select_js_runtime(self) -> tuple[str, str] | None:
        """Return (runtime_name, binary_path) according to preference:
        1. deno
        2. node (>= 22)
        3. quickjs / quickjs-ng
        4. bun (deprecated)
        """
        if self.deno:
            return ("deno", self.deno)
        if self.node:
            return ("node", self.node)
        if self.quickjs:
            return ("quickjs", self.quickjs)
        if self.bun:
            logger.warning(
                "Using bun as yt-dlp JS runtime; bun support is deprecated, prefer deno or node >= 22."
            )
            return ("bun", self.bun)
        return None

    @property
    def js_runtime(self) -> str | None:
        sel = self.select_js_runtime()
        if not sel:
            return None
        name, path = sel
        if name == "bun":
            return f"bun [deprecated] ({path})"
        return f"{name} ({path})"

    def get_ytdlp_js_args(self, ejs_remote: bool = False) -> list[str]:
        """Return CLI arguments for yt-dlp JS runtime and optional remote EJS components.

        Only deno is enabled by yt-dlp by default, so node or quickjs are passed explicitly.
        Remote components (ejs:github) are only enabled when ejs_remote=True.
        """
        args: list[str] = []
        sel = self.select_js_runtime()
        if sel:
            rt_name, rt_path = sel
            args.extend(["--js-runtimes", f"{rt_name}:{rt_path}"])

        if ejs_remote:
            args.extend(["--remote-components", "ejs:github"])
        return args

    def get_ytdlp_js_opts(self, ejs_remote: bool = False) -> dict[str, Any]:
        """Return yt_dlp Python API options dict for JS runtimes and EJS."""
        opts: dict[str, Any] = {}
        sel = self.select_js_runtime()
        if sel:
            rt_name, rt_path = sel
            opts["js_runtimes"] = {rt_name: {"path": rt_path}}
        if ejs_remote:
            opts["remote_components"] = ["ejs:github"]
        return opts


TOOLCHAIN = Toolchain()
