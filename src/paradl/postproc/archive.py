"""Safe archive extractor (ZIP, Tarball with filter='data', 7z, and macOS DMG mounter)."""

from __future__ import annotations

import logging
import os
import shutil
import stat
import subprocess
import tarfile
import zipfile
from pathlib import Path
from typing import ClassVar

from paradl.toolchain import TOOLCHAIN, is_macos
from paradl.ui import print_plain

logger = logging.getLogger(__name__)


class ArchiveSecurityError(ValueError):
    """Raised when an archive contains path traversal, absolute paths, or escaping links."""


class ArchiveExtractor:
    """Unpacks archives with strict path and symlink traversal validation."""

    ARCHIVE_EXTS: ClassVar[set[str]] = {
        ".zip",
        ".tar",
        ".tgz",
        ".tar.gz",
        ".txz",
        ".tar.xz",
        ".tbz2",
        ".tar.bz2",
        ".7z",
        ".dmg",
    }

    @classmethod
    def is_archive(cls, path: Path) -> bool:
        ext = path.suffix.lower()
        double_ext = "".join(path.suffixes[-2:]).lower()
        return ext in cls.ARCHIVE_EXTS or double_ext in cls.ARCHIVE_EXTS

    @staticmethod
    def _validate_relative_member_path(member_name: str, dest_dir: Path) -> Path:
        """Validate that member_name is relative, contains no '..', and stays within dest_dir."""
        if not member_name:
            raise ArchiveSecurityError("Empty archive member name")
        if member_name.startswith(("/", "\\")) or (len(member_name) >= 2 and member_name[1] == ":"):
            raise ArchiveSecurityError(f"Absolute path in archive is forbidden: {member_name!r}")

        norm_parts = Path(member_name.replace("\\", "/")).parts
        if ".." in norm_parts or "/" in norm_parts:
            raise ArchiveSecurityError(
                f"Path traversal in archive member is forbidden: {member_name!r}"
            )

        resolved_dest = dest_dir.resolve()
        candidate = (resolved_dest / Path(*norm_parts)).resolve()
        if not candidate.is_relative_to(resolved_dest):
            raise ArchiveSecurityError(
                f"Archive member escapes destination directory: {member_name!r}"
            )
        return candidate

    @classmethod
    def _validate_link_target(
        cls, member_name: str, link_target: str, dest_dir: Path, is_hardlink: bool = False
    ) -> None:
        """Validate that a symlink or hardlink target does not escape dest_dir."""
        if not link_target:
            raise ArchiveSecurityError(f"Empty link target for {member_name!r}")
        if link_target.startswith(("/", "\\")) or (len(link_target) >= 2 and link_target[1] == ":"):
            raise ArchiveSecurityError(
                f"Absolute link target in archive is forbidden: {member_name!r} -> {link_target!r}"
            )

        resolved_dest = dest_dir.resolve()
        member_dest = cls._validate_relative_member_path(member_name, resolved_dest)

        if is_hardlink:
            target_resolved = (resolved_dest / link_target.replace("\\", "/")).resolve()
        else:
            target_resolved = (member_dest.parent / link_target.replace("\\", "/")).resolve()

        if not target_resolved.is_relative_to(resolved_dest):
            raise ArchiveSecurityError(
                f"Symlink/hardlink escapes destination directory: {member_name!r} -> {link_target!r}"
            )

    @classmethod
    def validate_tar(cls, tf: tarfile.TarFile, dest_dir: Path) -> None:
        """Validate all members of a TarFile before extraction."""
        for member in tf.getmembers():
            cls._validate_relative_member_path(member.name, dest_dir)
            if member.issym():
                cls._validate_link_target(member.name, member.linkname, dest_dir, is_hardlink=False)
            elif member.islnk():
                cls._validate_link_target(member.name, member.linkname, dest_dir, is_hardlink=True)
            elif member.ischr() or member.isblk() or member.isfifo():
                raise ArchiveSecurityError(
                    f"Special device/FIFO member forbidden in archive: {member.name!r}"
                )

    @classmethod
    def validate_zip(cls, zf: zipfile.ZipFile, dest_dir: Path) -> None:
        """Validate all members and symlinks of a ZipFile before extraction."""
        for member in zf.infolist():
            cls._validate_relative_member_path(member.filename, dest_dir)
            mode = member.external_attr >> 16
            if stat.S_ISLNK(mode):
                link_bytes = zf.read(member)
                link_target = link_bytes.decode("utf-8", errors="replace")
                cls._validate_link_target(member.filename, link_target, dest_dir, is_hardlink=False)

    @classmethod
    def extract(
        cls,
        archive_path: Path,
        dest_dir: Path,
        remove_archive: bool = False,
    ) -> tuple[bool, Path | None, list[Path]]:
        archive_path = Path(archive_path).expanduser().resolve()
        if not archive_path.exists():
            return False, None, []

        dest_dir = Path(dest_dir).expanduser().resolve()
        dest_dir.mkdir(parents=True, exist_ok=True)

        name = archive_path.name
        lower_name = name.lower()
        extracted_files: list[Path] = []

        print_plain(
            f"  [dim]•[/dim] [white]Extracting archive:[/white] [cyan]{name}[/cyan] -> [dim]{dest_dir}[/dim]"
        )

        try:
            if lower_name.endswith(".zip"):
                with zipfile.ZipFile(archive_path, "r") as zf:
                    cls.validate_zip(zf, dest_dir)
                    for member in zf.infolist():
                        target_path = cls._validate_relative_member_path(member.filename, dest_dir)
                        mode = member.external_attr >> 16
                        if stat.S_ISLNK(mode):
                            link_target = zf.read(member).decode("utf-8", errors="replace")
                            target_path.parent.mkdir(parents=True, exist_ok=True)
                            if target_path.exists() or target_path.is_symlink():
                                target_path.unlink()
                            os.symlink(link_target, target_path)
                        else:
                            zf.extract(member, dest_dir)
                            safe_perm = mode & 0o777
                            if safe_perm and target_path.exists() and not target_path.is_symlink():
                                try:
                                    os.chmod(target_path, safe_perm)
                                except OSError as chmod_err:
                                    logger.debug("chmod failed on %s: %s", target_path, chmod_err)
                            if target_path.is_file() and not target_path.is_symlink():
                                extracted_files.append(target_path)

            elif any(
                lower_name.endswith(x)
                for x in (".tar", ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2")
            ):
                with tarfile.open(archive_path, "r:*") as tf:
                    cls.validate_tar(tf, dest_dir)
                    try:
                        tf.extractall(dest_dir, filter="data")
                    except TypeError:
                        tf.extractall(dest_dir)
                    for tar_member in tf.getmembers():
                        out_f = dest_dir / tar_member.name
                        if out_f.exists() and out_f.is_file() and not out_f.is_symlink():
                            extracted_files.append(out_f)

            elif lower_name.endswith(".dmg"):
                if not is_macos() or not TOOLCHAIN.hdiutil:
                    print_plain("  [warning]DMG mounting requires macOS with hdiutil.[/warning]")
                    return False, None, []
                mount_proc = subprocess.run(
                    [TOOLCHAIN.hdiutil, "attach", "-nobrowse", "-readonly", str(archive_path)],
                    capture_output=True,
                    text=True,
                    check=False,
                )
                if mount_proc.returncode == 0:
                    mount_point: str | None = None
                    for line in mount_proc.stdout.splitlines():
                        if "/Volumes/" in line:
                            vol_part = line.split("/Volumes/", 1)[1].strip()
                            mount_point = f"/Volumes/{vol_part}"
                            break
                    if mount_point and os.path.exists(mount_point):
                        try:
                            for item in os.listdir(mount_point):
                                if item.startswith("."):
                                    continue
                                src_item = os.path.join(mount_point, item)
                                dst_item = dest_dir / sanitize_dmg_item(item)
                                if item.endswith(".app") and os.path.isdir(src_item):
                                    if dst_item.exists():
                                        shutil.rmtree(dst_item, ignore_errors=True)
                                    shutil.copytree(src_item, dst_item, symlinks=True)
                                    extracted_files.append(dst_item)
                                    print_plain(
                                        f"  [green]✓[/green] [white]Copied application bundle:[/white] [cyan]{item}[/cyan]"
                                    )
                                elif os.path.isfile(src_item) and not os.path.islink(src_item):
                                    shutil.copy2(src_item, dst_item)
                                    extracted_files.append(dst_item)
                        finally:
                            subprocess.run(
                                [TOOLCHAIN.hdiutil, "detach", mount_point],
                                stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL,
                                check=False,
                            )
                else:
                    print_plain("  [error]Failed to attach DMG disk image.[/error]")
                    return False, None, []

            elif lower_name.endswith(".7z") and TOOLCHAIN.p7zip:
                # Validate members before extraction to prevent path traversal
                list_cmd = [TOOLCHAIN.p7zip, "l", "-slt", str(archive_path)]
                list_res = subprocess.run(
                    list_cmd,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.DEVNULL,
                    text=True,
                    check=False,
                )
                if list_res.returncode != 0:
                    print_plain("  [error]7z archive inspection failed.[/error]")
                    return False, None, []
                for line in list_res.stdout.splitlines():
                    if line.startswith("Path = "):
                        m_name = line[7:].strip()
                        if m_name and m_name != str(archive_path) and m_name != archive_path.name:
                            cls._validate_relative_member_path(m_name, dest_dir)
                    elif line.startswith("SymLink = "):
                        m_target = line[10:].strip()
                        if m_target:
                            cls._validate_link_target("symlink", m_target, dest_dir)

                cmd = [TOOLCHAIN.p7zip, "x", f"-o{dest_dir}", "-y", str(archive_path)]
                res = subprocess.run(
                    cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=False
                )
                if res.returncode != 0:
                    print_plain("  [error]7z extraction failed.[/error]")
                    return False, None, []
                extracted_files.extend(
                    Path(root) / f for root, _, files in os.walk(dest_dir) for f in files
                )
            else:
                print_plain(
                    f"  [warning]Unsupported archive format for automatic extraction: {name}[/warning]"
                )
                return False, None, []

            print_plain(f"  [green]✓[/green] [white]Extracted {len(extracted_files)} files[/white]")

            if remove_archive:
                try:
                    archive_path.unlink()
                except OSError as unlink_err:
                    logger.debug("Failed to unlink archive %s: %s", archive_path, unlink_err)

            return True, dest_dir, extracted_files

        except ArchiveSecurityError as sec_err:
            logger.error("Security violation in archive %s: %s", archive_path, sec_err)
            print_plain(f"  [error]✗ Archive security violation: {sec_err}[/error]", force=True)
            raise
        except (OSError, zipfile.BadZipFile, tarfile.TarError, subprocess.SubprocessError) as exc:
            logger.debug("Archive extraction error for %s: %s", archive_path, exc)
            print_plain(f"  [error]Extraction error: {exc}[/error]")
            return False, None, []


def sanitize_dmg_item(name: str) -> str:
    return name.replace("/", "_").replace("\\", "_").lstrip(".")
