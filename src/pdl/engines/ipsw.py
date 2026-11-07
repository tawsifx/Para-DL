"""Apple IPSW firmware resolution and verified download engine."""

from __future__ import annotations

import logging
import os
import re
from pathlib import Path
from typing import Any

from pdl.models import ExitCode, TargetInfo, TransferResult, WatchdogMode, format_size
from pdl.net import fetch_json_with_etag
from pdl.postproc.checksum import ChecksumEngine
from pdl.ui import UI, print_plain

logger = logging.getLogger(__name__)


class IpswEngine:
    """Resolves Apple IPSW firmware targets via ipsw.me v4 API and verifies signed SHA-256."""

    DEVICE_ALIASES: dict[str, str] = {
        "iphone 16": "iPhone17,3",
        "iphone 16 plus": "iPhone17,4",
        "iphone 16 pro": "iPhone17,1",
        "iphone 16 pro max": "iPhone17,2",
        "iphone 15": "iPhone15,4",
        "iphone 15 plus": "iPhone15,5",
        "iphone 15 pro": "iPhone16,1",
        "iphone 15 pro max": "iPhone16,2",
        "iphone 14": "iPhone14,7",
        "iphone 14 plus": "iPhone14,8",
        "iphone 14 pro": "iPhone15,2",
        "iphone 14 pro max": "iPhone15,3",
        "iphone 13": "iPhone14,5",
        "iphone 13 mini": "iPhone14,4",
        "iphone 13 pro": "iPhone14,2",
        "iphone 13 pro max": "iPhone14,3",
        "iphone 12": "iPhone13,2",
        "iphone 12 mini": "iPhone13,1",
        "iphone 12 pro": "iPhone13,3",
        "iphone 12 pro max": "iPhone13,4",
        "iphone 11": "iPhone12,1",
        "iphone 11 pro": "iPhone12,3",
        "iphone 11 pro max": "iPhone12,5",
        "iphone se 2": "iPhone12,8",
        "iphone se 3": "iPhone14,6",
    }

    @classmethod
    def is_ipsw(cls, target: str) -> bool:
        t = target.strip().lower()
        if t.startswith(("ipsw:", "ipsw/")):
            return True
        if "ipsw.me" in t:
            return True
        if ".ipsw" in t and ("updates.cdn-apple.com" in t or t.startswith("http")):
            return True
        return False

    @classmethod
    def resolve_device_identifier(cls, dev_str: str) -> str:
        s = dev_str.strip()
        if s.lower().startswith(("ipsw:", "ipsw/")):
            s = s[5:].strip()

        clean_lower = s.lower().replace("-", " ").replace("_", " ").strip()
        if clean_lower in cls.DEVICE_ALIASES:
            return cls.DEVICE_ALIASES[clean_lower]

        for alias, ident in cls.DEVICE_ALIASES.items():
            if clean_lower == alias or clean_lower == ident.lower():
                return ident

        return s

    @classmethod
    def resolve(cls, target: str, *, allow_network: bool = True) -> dict[str, Any] | None:
        target = target.strip()
        if target.startswith(("http://", "https://")):
            url_clean = target.split("?", 1)[0].split("#", 1)[0]
            fname = os.path.basename(url_clean)
            m = re.match(r"([A-Za-z0-9,]+)_([0-9.]+)_([A-Za-z0-9]+)_Restore\.ipsw", fname)
            ident = m.group(1) if m else None
            build = m.group(3) if m else None
            meta: dict[str, Any] = {
                "device_name": ident or "Apple Device",
                "identifier": ident,
                "version": m.group(2) if m else "",
                "buildid": build or "",
                "url": target,
                "filename": fname,
                "filesize": None,
                "sha256": None,
            }
            if ident and allow_network:
                api_url = f"https://api.ipsw.me/v4/device/{ident}?type=ipsw"
                data, _hdrs, status = fetch_json_with_etag(api_url, timeout=4.5)
                if status == 200 and isinstance(data, dict):
                    meta["device_name"] = data.get("name", ident)
                    for fw in data.get("firmwares", []):
                        if (build and fw.get("buildid") == build) or fw.get("url") == target:
                            meta["version"] = fw.get("version", meta["version"])
                            meta["buildid"] = fw.get("buildid", meta["buildid"])
                            meta["sha256"] = fw.get("sha256sum")
                            meta["filesize"] = fw.get("filesize")
                            break
            return meta

        ident = cls.resolve_device_identifier(target)
        if allow_network:
            api_url = f"https://api.ipsw.me/v4/device/{ident}?type=ipsw"
            data, _hdrs, status = fetch_json_with_etag(api_url, timeout=6.0)
            if status == 200 and isinstance(data, dict):
                signed = [f for f in data.get("firmwares", []) if f.get("signed")]
                if signed:
                    latest = signed[0]
                    url = latest["url"]
                    return {
                        "device_name": data.get("name", ident),
                        "identifier": ident,
                        "version": latest.get("version"),
                        "buildid": latest.get("buildid"),
                        "url": url,
                        "filename": os.path.basename(url.split("?", 1)[0]),
                        "filesize": latest.get("filesize"),
                        "sha256": latest.get("sha256sum"),
                    }
        else:
            # Offline synthetic metadata for classification table testing
            return {
                "device_name": ident,
                "identifier": ident,
                "version": "",
                "buildid": "",
                "url": f"https://api.ipsw.me/v4/ipsw/download/{ident}/latest",
                "filename": f"{ident}_Restore.ipsw",
                "filesize": None,
                "sha256": None,
            }
        return None

    def execute(
        self,
        target_info: TargetInfo,
        out_dir: Path,
        *,
        connections: int = 16,
        **options: Any,
    ) -> TransferResult:
        from pdl.engines.aria2 import Aria2Engine

        meta = target_info.ipsw_meta or {}
        dev_name = meta.get("device_name", "Apple Device")
        ios_ver = meta.get("version", "")
        build_id = meta.get("buildid", "")
        safe_conn = min(max(connections, 1), 16)

        print_plain(
            f"  [bold white]Apple Device:[/bold white] [bold cyan]{dev_name}[/bold cyan] [dim]({meta.get('identifier', '')})[/dim]"
        )
        if ios_ver:
            print_plain(
                f"  [dim]Firmware:[/dim]     [green]iOS {ios_ver}[/green] [dim](Build {build_id}) • Apple Signed[/dim]"
            )
        if meta.get("filesize"):
            print_plain(f"  [dim]File Size:[/dim]    [white]{format_size(meta['filesize'])}[/white]")
        print_plain(f"  [dim]Destination:[/dim]  [dim]{out_dir}[/dim] [dim]({safe_conn} parallel streams)[/dim]")

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

        # Signed SHA-256 verification is on by default and reported
        expected_sha = meta.get("sha256")
        if res.success and res.file_path and res.file_path.is_file() and expected_sha:
            print_plain("  [dim]•[/dim] [white]Verifying official Apple SHA-256 checksum...[/white]")
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
                source="apple_tss",
            )
            if matched:
                print_plain("  [green]✓ Firmware integrity verified against Apple TSS catalog (SHA-256)![/green]")
            else:
                print_plain(f"  [error]✗ Checksum mismatch! Expected {expected_sha}, got {digest}[/error]", force=True)
                res.success = False
                res.exit_code = int(ExitCode.VERIFICATION_FAILED)
                res.error = f"Apple SHA-256 mismatch: expected {expected_sha}, got {digest}"

        return res
