"""PyPI / pip Python package resolution, multi-stream download, and installation engine."""

from __future__ import annotations

import logging
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

from pdl.models import ExitCode, TargetInfo, TargetType, TransferResult, WatchdogMode, format_size
from pdl.net import fetch_json_with_etag
from pdl.postproc.checksum import ChecksumEngine
from pdl.ui import UI, print_plain

logger = logging.getLogger(__name__)


class PyPIEngine:
    """Resolves Python packages via PyPI JSON API and downloads/installs with 16 parallel channels."""

    @classmethod
    def is_pypi(cls, target: str) -> bool:
        """Return True if target represents a pip/pypi package or URL."""
        t = target.strip().lower()
        if t.startswith(("pip:", "pip/", "pypi:", "pypi/")):
            return True
        if "pypi.org/project/" in t or "pypi.python.org/pypi/" in t:
            return True
        return False

    @classmethod
    def parse_pypi_target(cls, target: str) -> tuple[str, str | None]:
        """Extract package name and optional version requirement from target string.

        Examples:
            'pip:requests' -> ('requests', None)
            'pip:requests==2.32.3' -> ('requests', '2.32.3')
            'pypi:numpy@1.26.4' -> ('numpy', '1.26.4')
            'https://pypi.org/project/urllib3/2.2.0/' -> ('urllib3', '2.2.0')
        """
        t = target.strip()
        if t.lower().startswith(("pip:", "pip/", "pypi:", "pypi/")):
            t = t.split(":", 1)[-1].split("/", 1)[-1].strip()
        elif "pypi.org/project/" in t or "pypi.python.org/pypi/" in t:
            parsed = urlparse(t)
            parts = [p for p in parsed.path.split("/") if p and p not in ("project", "pypi")]
            pkg = parts[0] if parts else ""
            ver = parts[1] if len(parts) > 1 else None
            return pkg, ver

        m = re.match(r"^([A-Za-z0-9_\-\.]+)(?:([=><~@]+)([A-Za-z0-9_\-\.]+))?$", t)
        if m:
            pkg = m.group(1).strip()
            ver = m.group(3).strip() if (m.group(2) in ("==", "@") and m.group(3)) else None
            return pkg, ver

        try:
            from packaging.requirements import Requirement

            req = Requirement(t)
            pkg = req.name
            ver = None
            for s in req.specifier:
                if s.operator == "==":
                    ver = s.version
                    break
            return pkg, ver
        except Exception:
            pass

        return t, None

    @classmethod
    def score_wheel_filename(
        cls,
        filename: str,
        packagetype: str = "bdist_wheel",
        *,
        py_version: tuple[int, int] | None = None,
        target_platform: str | None = None,
        target_machine: str | None = None,
        target_py: str | None = None,
        target_plat: str | None = None,
        is_arm: bool | None = None,
    ) -> int:
        """Alias helper for score_wheel with flexible parameter names."""
        py_ver = py_version
        if target_py and not py_ver:
            if len(target_py) >= 2 and target_py.isdigit():
                py_ver = (int(target_py[0]), int(target_py[1:]))
        mach = target_machine
        if is_arm is not None and not mach:
            mach = "arm64" if is_arm else "amd64"
        return cls.score_wheel(
            filename,
            packagetype,
            py_version=py_ver,
            target_platform=target_plat or target_platform,
            target_machine=mach,
        )

    @classmethod
    def score_wheel(
        cls,
        filename: str,
        packagetype: str,
        *,
        py_version: tuple[int, int] | None = None,
        target_platform: str | None = None,
        target_machine: str | None = None,
    ) -> int:
        """Score candidate release artifact based on Python runtime and platform compatibility.

        Returns:
            Positive integer score (higher = better compatibility), or -1 if incompatible.
        """
        fn = filename.lower()
        if packagetype == "sdist" or fn.endswith((".tar.gz", ".zip")):
            return 10  # fallback source distribution

        if not fn.endswith(".whl"):
            return -1

        py_info = py_version or (sys.version_info.major, sys.version_info.minor)
        py_ver = f"cp{py_info[0]}{py_info[1]}"
        py_maj = f"py{py_info[0]}"

        plat = (target_platform or sys.platform).lower()
        mach = (target_machine or platform.machine()).lower()
        if mach in ("arm64", "aarch64"):
            arch_tags = ["arm64", "aarch64", "universal2", "win_arm64"]
        else:
            arch_tags = ["x86_64", "amd64", "universal2", "x86", "win_amd64", "win32"]

        is_plat_compat = False
        is_universal = fn.endswith("-any.whl") or "none-any" in fn
        if is_universal:
            is_plat_compat = True
        elif plat == "darwin" and "macosx" in fn:
            if any(a in fn for a in arch_tags):
                is_plat_compat = True
        elif plat.startswith("linux") and any(tag in fn for tag in ("manylinux", "musllinux", "linux")):
            if any(a in fn for a in arch_tags):
                is_plat_compat = True
        elif plat == "win32" and any(w in fn for w in ("win_amd64", "win_arm64", "win32", "win")):
            if any(a in fn for a in arch_tags):
                is_plat_compat = True

        if not is_plat_compat:
            return -1

        score = 20
        # Python ABI / version scoring
        if py_ver in fn:
            score += 60  # exact CPython version match (e.g. cp312)
        elif "abi3" in fn:
            score += 50  # stable CPython ABI
        elif "py3-none-any" in fn or "py2.py3-none-any" in fn or is_universal:
            score += 40  # universal pure-python wheel
        elif py_maj in fn:
            score += 30

        # Architecture scoring
        if plat == "win32":
            if mach in ("arm64", "aarch64"):
                if "win_arm64" in fn:
                    score += 20  # native ARM64
                elif "win_amd64" in fn:
                    score += 5   # x64 emulation on ARM64 Windows 11
                elif "win32" in fn:
                    score += 2   # 32-bit x86 emulation
                elif is_universal:
                    score += 10
            elif mach in ("x86", "i386", "i686"):
                if "win32" in fn and "win_amd64" not in fn:
                    score += 20  # native 32-bit
                elif is_universal:
                    score += 10
            else:
                if "win_amd64" in fn:
                    score += 20  # native 64-bit x86_64
                elif "win32" in fn:
                    score += 5   # 32-bit x86 compatibility
                elif is_universal:
                    score += 10
        else:
            if any(a in fn for a in arch_tags if a != "universal2"):
                score += 20  # native architecture
            elif "universal2" in fn:
                score += 15  # multi-arch binary
            elif is_universal:
                score += 10  # architecture-independent

        return score

    @classmethod
    def resolve(
        cls,
        target: str,
        *,
        allow_network: bool = True,
        py_version: tuple[int, int] | None = None,
    ) -> dict[str, Any] | None:
        """Resolve PyPI package target into metadata, direct CDN URL, and SHA-256 digest."""
        pkg_name, version = cls.parse_pypi_target(target)
        if not pkg_name:
            return None

        # Normalize package name per PEP 503
        norm_name = re.sub(r"[-_.]+", "-", pkg_name).lower()
        api_url = (
            f"https://pypi.org/pypi/{norm_name}/{version}/json"
            if version
            else f"https://pypi.org/pypi/{norm_name}/json"
        )

        meta: dict[str, Any] = {
            "package": pkg_name,
            "version": version or "",
            "filename": f"{norm_name}.whl",
            "url": "",
            "filesize": None,
            "sha256": None,
            "packagetype": "bdist_wheel",
            "summary": "",
            "requires_dist": [],
        }

        if not allow_network:
            return meta

        data, _hdrs, status = fetch_json_with_etag(api_url, timeout=4.5)
        if status != 200 or not isinstance(data, dict):
            # If specific version 404s, try fallback to latest
            if version and status == 404:
                api_url = f"https://pypi.org/pypi/{norm_name}/json"
                data, _hdrs, status = fetch_json_with_etag(api_url, timeout=4.5)
            if status != 200 or not isinstance(data, dict):
                logger.debug("PyPI API lookup failed for %s (status %s)", pkg_name, status)
                return None

        info = data.get("info", {})
        meta["package"] = info.get("name", pkg_name)
        meta["version"] = info.get("version", version or "")
        meta["summary"] = info.get("summary", "")
        meta["requires_dist"] = [str(r) for r in (info.get("requires_dist") or []) if r]

        urls = data.get("urls", [])

        # Check if the target has version specifiers (e.g. <1.4 or >=2.0)
        try:
            from packaging.requirements import Requirement
            from packaging.version import Version

            t_clean = target.strip()
            if t_clean.lower().startswith(("pip:", "pip/", "pypi:", "pypi/")):
                t_clean = t_clean.split(":", 1)[-1].split("/", 1)[-1].strip()
            req_parsed = Requirement(t_clean)
            if req_parsed.specifier:
                curr_v = info.get("version", "")
                if curr_v and Version(curr_v) not in req_parsed.specifier:
                    releases = data.get("releases", {})
                    matched_releases: list[tuple[Any, str]] = []
                    for v_str in releases:
                        try:
                            v_obj = Version(v_str)
                            if v_obj in req_parsed.specifier and not v_obj.is_prerelease:
                                matched_releases.append((v_obj, v_str))
                        except Exception:
                            continue
                    if matched_releases:
                        matched_releases.sort(key=lambda x: x[0])
                        chosen_ver = matched_releases[-1][1]
                        urls = releases.get(chosen_ver, [])
                        meta["version"] = chosen_ver
        except Exception as exc:
            logger.debug("Specifier resolution check skipped for %s: %s", pkg_name, exc)

        if not urls:
            return None

        best_cand: dict[str, Any] | None = None
        best_score = -1

        for cand in urls:
            fname = cand.get("filename", "")
            ptype = cand.get("packagetype", "")
            sc = cls.score_wheel(fname, ptype, py_version=py_version)
            if sc > best_score:
                best_score = sc
                best_cand = cand

        if not best_cand:
            return None

        meta["filename"] = best_cand.get("filename", f"{norm_name}.whl")
        meta["url"] = best_cand.get("url", "")
        meta["filesize"] = best_cand.get("size")
        meta["packagetype"] = best_cand.get("packagetype", "bdist_wheel")
        digests = best_cand.get("digests", {})
        meta["sha256"] = digests.get("sha256")

        return meta

    def execute(
        self,
        target_info: TargetInfo,
        out_dir: Path,
        *,
        connections: int = 16,
        **options: Any,
    ) -> TransferResult:
        """Download resolved Python package with 16 parallel channels and verify SHA-256."""
        from pdl.engines.aria2 import Aria2Engine

        meta = target_info.pypi_meta or {}
        pkg = meta.get("package", "python-package")
        ver = meta.get("version", "")
        summary = meta.get("summary", "")
        safe_conn = min(max(connections, 1), 16)

        print_plain(f"  [bold white]Python Package:[/bold white] [bold cyan]{pkg}[/bold cyan] [green]v{ver}[/green]")
        if summary:
            print_plain(f"  [dim]Summary:[/dim]        [dim]{summary[:80]}[/dim]")
        if meta.get("filesize"):
            print_plain(f"  [dim]File Size:[/dim]      [white]{format_size(meta['filesize'])}[/white]")
        print_plain(f"  [dim]Destination:[/dim]    [dim]{out_dir}[/dim] [dim]({safe_conn} parallel streams)[/dim]")

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
            print_plain("  [dim]•[/dim] [white]Verifying PyPI SHA-256 checksum...[/white]")
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
                source="pypi_index",
            )
            if matched:
                print_plain("  [green]✓ Package integrity verified against PyPI index (SHA-256)![/green]")
            else:
                print_plain(f"  [error]✗ Checksum mismatch! Expected {expected_sha}, got {digest}[/error]", force=True)
                res.success = False
                res.exit_code = int(ExitCode.VERIFICATION_FAILED)
                res.error = f"PyPI SHA-256 mismatch: expected {expected_sha}, got {digest}"

        return res

    @classmethod
    def extract_wheel_dependencies(
        cls,
        wheel_path: Path,
        *,
        target_env: dict[str, str] | None = None,
    ) -> list[str]:
        """Extract required dependencies from a local .whl file, evaluating PEP 508 environment markers."""
        if not str(wheel_path).endswith(".whl") or not wheel_path.is_file():
            return []

        import zipfile
        from packaging.requirements import Requirement

        reqs: list[str] = []
        try:
            with zipfile.ZipFile(wheel_path, "r") as zf:
                for name in zf.namelist():
                    if name.endswith(".dist-info/METADATA"):
                        content = zf.read(name).decode("utf-8", errors="replace")
                        for line in content.splitlines():
                            if line.startswith("Requires-Dist:"):
                                raw_req = line.split(":", 1)[1].strip()
                                try:
                                    r = Requirement(raw_req)
                                    if r.marker:
                                        m_str = str(r.marker)
                                        if "extra ==" in m_str or "extra==" in m_str:
                                            continue
                                        if not r.marker.evaluate(target_env):
                                            continue
                                    reqs.append(str(r))
                                except Exception:
                                    pass
                        break
        except Exception as exc:
            logger.debug("Failed to read metadata from wheel %s: %s", wheel_path, exc)
        return reqs

    @classmethod
    def install(
        cls,
        target: str,
        *,
        connections: int = 16,
        assume_yes: bool = False,
        python_bin: str | None = None,
    ) -> TransferResult:
        """Download package wheel and all transitive dependencies via 16 parallel channels, then install offline."""
        from pdl.net import probe_target

        # Detect installer: uv pip install > python3 -m pip install
        uv_bin = shutil.which("uv")
        py_exec = python_bin or (
            os.path.join(os.environ["VIRTUAL_ENV"], "bin", "python")
            if os.environ.get("VIRTUAL_ENV")
            else sys.executable
        )
        target_py_ver: tuple[int, int] | None = None
        if py_exec and os.path.exists(py_exec):
            try:
                proc = subprocess.run(
                    [py_exec, "-c", "import sys; print(f'{sys.version_info.major}.{sys.version_info.minor}')"],
                    capture_output=True,
                    text=True,
                    timeout=2.0,
                    check=False,
                )
                if proc.returncode == 0:
                    parts = proc.stdout.strip().split(".")
                    target_py_ver = (int(parts[0]), int(parts[1]))
            except Exception:
                pass

        t_info = probe_target(target, py_version=target_py_ver)
        if t_info.target_type != TargetType.PYPI or not t_info.clean_url:
            msg = f"Failed to resolve PyPI package: {target}"
            print_plain(f"  [error]{msg}[/error]", force=True)
            return TransferResult(success=False, error=msg, exit_code=int(ExitCode.FAILURE))

        meta = t_info.pypi_meta or {}
        pkg = meta.get("package", target)
        ver = meta.get("version", "")

        print_plain(
            f"  [dim]•[/dim] [white]Accelerating install for Python package:[/white] [cyan]{pkg}[/cyan] [green]{ver}[/green]"
        )

        with tempfile.TemporaryDirectory(prefix="pdl_pip_") as tmp_dir:
            tmp_path = Path(tmp_dir)
            engine = cls()
            dl_res = engine.execute(t_info, tmp_path, connections=connections)

            if not dl_res.success or not dl_res.file_path or not dl_res.file_path.exists():
                msg = dl_res.error or "Failed to download package wheel."
                print_plain(f"  [error]{msg}[/error]", force=True)
                return dl_res

            wheel_file = dl_res.file_path

            # Detect installed packages in target environment to avoid redundant downloads
            installed_pkgs: set[str] = set()
            if uv_bin:
                try:
                    cmd_l = [uv_bin, "pip", "list"]
                    if py_exec and os.path.exists(py_exec):
                        cmd_l.extend(["--python", py_exec])
                    list_proc = subprocess.run(
                        cmd_l, capture_output=True, text=True, timeout=4.0, check=False
                    )
                    if list_proc.returncode == 0:
                        for row in list_proc.stdout.splitlines()[2:]:
                            parts = row.split()
                            if parts:
                                installed_pkgs.add(parts[0].lower().replace("_", "-"))
                except Exception:
                    pass

            downloaded_pkgs: set[str] = {pkg.lower().replace("_", "-")}
            for f in tmp_path.glob("*.whl"):
                m_f = re.match(r"^([A-Za-z0-9_\-\.]+?)-", f.name)
                if m_f:
                    downloaded_pkgs.add(m_f.group(1).lower().replace("_", "-"))

            # Seed dependency queue
            queue: list[str] = []

            # 1. Inspect root package
            if wheel_file.suffix.lower() == ".whl":
                queue.extend(cls.extract_wheel_dependencies(wheel_file))
            else:
                for r in meta.get("requires_dist") or []:
                    queue.append(r)
                if "setuptools" not in installed_pkgs and "setuptools" not in downloaded_pkgs:
                    queue.append("setuptools")
                if "wheel" not in installed_pkgs and "wheel" not in downloaded_pkgs:
                    queue.append("wheel")

            # 2. Fast compile check
            if uv_bin:
                try:
                    req_spec = f"{pkg}=={ver}" if ver else pkg
                    cmd_comp = [uv_bin, "pip", "compile", "-"]
                    if py_exec and os.path.exists(py_exec):
                        cmd_comp.extend(["--python", py_exec])
                    compile_proc = subprocess.run(
                        cmd_comp,
                        input=f"{req_spec}\n".encode(),
                        capture_output=True,
                        timeout=5.0,
                        check=False,
                    )
                    if compile_proc.returncode == 0:
                        for line in compile_proc.stdout.decode().splitlines():
                            line = line.strip()
                            if line and not line.startswith("#") and "==" in line:
                                d_pkg, d_ver = line.split("==", 1)
                                if d_pkg.strip().lower() != pkg.lower():
                                    queue.append(f"{d_pkg.strip()}=={d_ver.strip()}")
                except Exception as exc:
                    logger.debug("Fast uv pip compile skipped: %s", exc)

            # 3. Breadth-First-Search: pre-fetch ALL transitive dependencies recursively
            visited_reqs: set[str] = set()
            while queue:
                current_batch: list[str] = []
                while queue:
                    r_str = queue.pop(0).strip()
                    if not r_str or r_str in visited_reqs:
                        continue
                    visited_reqs.add(r_str)
                    try:
                        from packaging.requirements import Requirement
                        r_obj = Requirement(r_str)
                        if r_obj.marker:
                            m_str = str(r_obj.marker)
                            if "extra ==" in m_str or "extra==" in m_str:
                                continue
                            if not r_obj.marker.evaluate():
                                continue
                        r_norm = r_obj.name.lower().replace("_", "-")
                        if r_norm == pkg.lower().replace("_", "-") or r_norm in installed_pkgs or r_norm in downloaded_pkgs:
                            continue
                        current_batch.append(str(r_obj))
                    except Exception:
                        m_plain = re.match(r"^([A-Za-z0-9_\-\.]+)", r_str)
                        if m_plain:
                            d_name = m_plain.group(1).lower().replace("_", "-")
                            if d_name != pkg.lower().replace("_", "-") and d_name not in installed_pkgs and d_name not in downloaded_pkgs:
                                current_batch.append(r_str)

                if not current_batch:
                    break

                batch_label = ", ".join(current_batch[:6])
                if len(current_batch) > 6:
                    batch_label += f" (+{len(current_batch) - 6} more)"
                print_plain(
                    f"  [dim]•[/dim] [white]Pre-fetching {len(current_batch)} Python dependency wheel(s) with 16 parallel channels:[/white] [cyan]{batch_label}[/cyan]"
                )

                for dep_req in current_batch:
                    try:
                        dep_target = f"pip:{dep_req}"
                        dep_info = probe_target(dep_target, py_version=target_py_ver)
                        if dep_info.target_type == TargetType.PYPI and dep_info.clean_url:
                            res_dep = engine.execute(dep_info, tmp_path, connections=connections)
                            if res_dep.success and res_dep.file_path and res_dep.file_path.is_file():
                                d_meta = dep_info.pypi_meta or {}
                                d_pkg_name = d_meta.get("package", "").lower().replace("_", "-")
                                if d_pkg_name:
                                    downloaded_pkgs.add(d_pkg_name)
                                if res_dep.file_path.suffix.lower() == ".whl":
                                    sub_deps = cls.extract_wheel_dependencies(res_dep.file_path)
                                    for sd in sub_deps:
                                        if sd not in visited_reqs:
                                            queue.append(sd)
                    except Exception as dep_exc:
                        logger.debug("Failed to pre-fetch wheel for dependency %s: %s", dep_req, dep_exc)

            # Strict offline installation: enforce --no-index to prevent single-stream offloading
            installer_base = (
                [uv_bin, "pip", "install", "--no-index", "--find-links", str(tmp_path)]
                if uv_bin
                else [py_exec, "-m", "pip", "install", "--no-index", "--find-links", str(tmp_path)]
            )
            if uv_bin and py_exec and os.path.exists(py_exec):
                installer_base.extend(["--python", py_exec])

            installer_desc = (
                "uv pip install (--no-index --find-links)"
                if uv_bin
                else f"{os.path.basename(py_exec)} -m pip install (--no-index --find-links)"
            )

            print_plain(f"  [dim]•[/dim] [white]Installing via {installer_desc}...[/white]")

            max_attempts = 3
            for attempt in range(1, max_attempts + 1):
                cmd = [*installer_base, str(wheel_file)]
                proc = subprocess.run(cmd, capture_output=True, text=True, check=False)
                if proc.returncode == 0:
                    print_plain(f"  [green]✓ Successfully installed {pkg} {ver} into Python environment![/green]")
                    return TransferResult(
                        success=True,
                        file_path=wheel_file,
                        bytes_downloaded=dl_res.bytes_downloaded,
                        checksum_verified=dl_res.checksum_verified,
                        checksum_algo=dl_res.checksum_algo,
                        checksum_digest=dl_res.checksum_digest,
                        exit_code=int(ExitCode.OK),
                    )

                # Check if a missing dependency was reported in error output
                err_text = proc.stderr.strip()
                missing_candidates = re.findall(
                    r"(?:require|depends on|found for)\s+([A-Za-z0-9_\-\.]+)",
                    err_text,
                    re.IGNORECASE,
                )
                missing_candidates = [
                    c for c in missing_candidates
                    if c.lower().replace("_", "-") not in downloaded_pkgs
                    and not c.startswith(("-", "http", "202", "19", "pip", "uv"))
                ]
                if missing_candidates and attempt < max_attempts:
                    rec_pkg = missing_candidates[0]
                    print_plain(
                        f"  [dim]•[/dim] [yellow]Recovering missing dependency via 16 parallel channels:[/yellow] [cyan]{rec_pkg}[/cyan]"
                    )
                    try:
                        rec_info = probe_target(f"pip:{rec_pkg}", py_version=target_py_ver)
                        if rec_info.target_type == TargetType.PYPI and rec_info.clean_url:
                            engine.execute(rec_info, tmp_path, connections=connections)
                            downloaded_pkgs.add(rec_pkg.lower().replace("_", "-"))
                            continue
                    except Exception as rec_exc:
                        logger.debug("Auto-recovery download failed for %s: %s", rec_pkg, rec_exc)

                err_msg = err_text or f"{installer_desc} failed with exit code {proc.returncode}"
                print_plain(f"  [error]✗ {err_msg}[/error]", force=True)
                return TransferResult(
                    success=False,
                    error=err_msg,
                    exit_code=int(ExitCode.FAILURE),
                )

        return TransferResult(success=False, error="Installation failed", exit_code=int(ExitCode.FAILURE))
