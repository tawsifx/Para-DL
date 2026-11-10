"""Android ADB device synchronization post-processor."""

from __future__ import annotations

import logging
import subprocess
from pathlib import Path

from paradl.toolchain import TOOLCHAIN
from paradl.ui import print_plain

logger = logging.getLogger(__name__)


class ADBDeviceSync:
    """Transfers completed downloads to connected Android devices via ADB."""

    @staticmethod
    def get_connected_devices() -> list[dict[str, str]]:
        adb_bin = TOOLCHAIN.adb
        if not adb_bin:
            return []
        try:
            res = subprocess.run(
                [adb_bin, "devices", "-l"], capture_output=True, text=True, check=False, timeout=5.0
            )
            devices: list[dict[str, str]] = []
            for raw_line in res.stdout.splitlines()[1:]:
                line = raw_line.strip()
                if not line or "offline" in line:
                    continue
                parts = line.split()
                if len(parts) < 2:
                    continue
                serial = parts[0]
                status = parts[1]
                model = "Android Device"
                for p in parts[2:]:
                    if p.startswith("model:"):
                        model = p.split("model:", 1)[1].replace("_", " ")
                devices.append({"serial": serial, "status": status, "model": model})
            return devices
        except (OSError, subprocess.SubprocessError) as exc:
            logger.debug("ADB device enumeration failed: %s", exc)
            return []

    @staticmethod
    def push_file(file_path: Path, target_dir: str = "/storage/emulated/0/Music") -> bool:
        adb_bin = TOOLCHAIN.adb
        if not adb_bin:
            print_plain("  [warning]ADB is not installed or not in PATH.[/warning]")
            return False

        devices = ADBDeviceSync.get_connected_devices()
        if not devices:
            print_plain("  [warning]No active Android devices detected via ADB.[/warning]")
            return False

        target_dev = devices[0]
        if target_dev["status"] != "device":
            print_plain(
                f"  [warning]Device {target_dev['serial']} is {target_dev['status']}. Confirm USB debugging prompt on phone.[/warning]"
            )
            return False

        print_plain(f"  [dim]• Syncing to {target_dev['model']}...[/dim]")
        try:
            push_cmd = [
                adb_bin,
                "-s",
                target_dev["serial"],
                "push",
                str(file_path),
                f"{target_dir}/",
            ]
            p_res = subprocess.run(push_cmd, check=False)
            if p_res.returncode == 0:
                remote_path = f"{target_dir}/{file_path.name}"
                subprocess.run(
                    [
                        adb_bin,
                        "-s",
                        target_dev["serial"],
                        "shell",
                        "am",
                        "broadcast",
                        "-a",
                        "android.intent.action.MEDIA_SCANNER_SCAN_FILE",
                        "-d",
                        f"file://{remote_path}",
                    ],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    check=False,
                )
                print_plain(f"  [green]✓[/green] [white]Synced to[/white] {target_dev['model']}")
                return True
        except (OSError, subprocess.SubprocessError) as exc:
            logger.debug("ADB push error: %s", exc)

        print_plain("  [error]✗ ADB push failed. Check phone connection.[/error]")
        return False
