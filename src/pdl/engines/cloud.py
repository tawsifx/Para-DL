"""Cloud sharing URL resolvers (Google Drive, Dropbox, Pastebin)."""

from __future__ import annotations

import re
from urllib.parse import urlparse


class CloudResolver:
    """Transmutes cloud preview/sharing links into direct download streams."""

    @staticmethod
    def resolve_url(url: str) -> tuple[str, str | None]:
        u = url.strip()
        try:
            host = (urlparse(u).hostname or "").lower()
        except ValueError:
            host = ""

        # Google Drive: convert preview link to direct export stream
        if host == "drive.google.com" or host.endswith(".drive.google.com") or "drive.google.com" in u:
            file_id: str | None = None
            m = re.search(r"drive\.google\.com/file/d/([a-zA-Z0-9_-]+)", u)
            if m:
                file_id = m.group(1)
            else:
                m = re.search(r"[?&]id=([a-zA-Z0-9_-]+)", u)
                if m:
                    file_id = m.group(1)

            if file_id:
                direct = f"https://drive.usercontent.google.com/download?id={file_id}&export=download&confirm=t"
                return direct, None

        # Dropbox: change dl=0 to dl=1
        if host == "dropbox.com" or host.endswith(".dropbox.com") or "dropbox.com" in u:
            if "dl=0" in u:
                return u.replace("dl=0", "dl=1"), None
            if "dl=1" in u:
                return u, None
            if "?" in u:
                return f"{u}&dl=1", None
            return f"{u}?dl=1", None

        # Pastebin: direct raw URL
        if (host == "pastebin.com" or host.endswith(".pastebin.com") or "pastebin.com" in u) and "/raw/" not in u:
            m = re.search(r"pastebin\.com/([a-zA-Z0-9]+)$", u)
            if m:
                return f"https://pastebin.com/raw/{m.group(1)}", f"{m.group(1)}.txt"

        return u, None
