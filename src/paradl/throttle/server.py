"""Asyncio VPS ground-truth test server (`pdl throttle server`, M9).

Offers configurable payloads (`random`, `video`, `file`), Range support, upload sink,
and optional UDP echo on arbitrary ports for DPI, payload-type, and port-sweep testing.
"""

from __future__ import annotations

import asyncio
import json
import logging
import re
from typing import Any
from urllib.parse import parse_qs, urlparse

logger = logging.getLogger(__name__)

_RANGE_RE = re.compile(r"bytes=(\d+)-(\d*)")
_MP4_FTYP_HEADER = b"\x00\x00\x00\x18ftypmp42\x00\x00\x00\x00mp42isom\x00\x01\x00\x00mdat"
_ZIP_LOCAL_HEADER = b"PK\x03\x04\x14\x00\x00\x00\x00\x00"


def build_payload_block(payload_type: str = "random", size: int = 65536) -> tuple[bytes, str]:
    """Return `(deterministic_block_bytes, content_type)` for `random`, `video`, or `file`."""
    base = bytes((i * 73 + 29) & 0xFF for i in range(max(64, size)))
    kind = (payload_type or "random").strip().lower()
    if kind == "video":
        blk = _MP4_FTYP_HEADER + base[len(_MP4_FTYP_HEADER) :]
        return blk, "video/mp4"
    if kind == "file":
        blk = _ZIP_LOCAL_HEADER + base[len(_ZIP_LOCAL_HEADER) :]
        return blk, "application/zip"
    return base, "application/octet-stream"


class UDPThrottleEchoProtocol(asyncio.DatagramProtocol):
    """Simple UDP datagram echo responder for M9 UDP reachability/loss tests."""

    def __init__(self) -> None:
        self.transport: asyncio.DatagramTransport | None = None

    def connection_made(self, transport: asyncio.BaseTransport) -> None:
        self.transport = transport  # type: ignore[assignment]

    def datagram_received(self, data: bytes, addr: tuple[str, int]) -> None:
        if self.transport is not None:
            self.transport.sendto(data, addr)


async def handle_tcp_client(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    """Handle HTTP/1.1 GET `/down`, POST `/upload`, and GET `/echo` on the asyncio VPS server."""
    try:
        req_line_bytes = await asyncio.wait_for(reader.readline(), timeout=5.0)
        if not req_line_bytes:
            return
        req_line = req_line_bytes.decode("latin-1", errors="ignore").strip()
        parts = req_line.split()
        if len(parts) < 2:
            return
        method, raw_path = parts[0].upper(), parts[1]

        headers: dict[str, str] = {}
        while True:
            hdr_line = await asyncio.wait_for(reader.readline(), timeout=5.0)
            if not hdr_line or hdr_line in (b"\r\n", b"\n"):
                break
            line_str = hdr_line.decode("latin-1", errors="ignore").strip()
            if ":" in line_str:
                k, v = line_str.split(":", 1)
                headers[k.strip().lower()] = v.strip()

        parsed = urlparse(raw_path)
        qs = parse_qs(parsed.query)

        if method == "POST" and parsed.path in ("/up", "/upload"):
            content_len = int(headers.get("content-length", "0") or 0)
            remaining = content_len
            received = 0
            while remaining > 0:
                chunk = await asyncio.wait_for(reader.read(min(65536, remaining)), timeout=5.0)
                if not chunk:
                    break
                received += len(chunk)
                remaining -= len(chunk)
            body = json.dumps({"status": "ok", "received_bytes": received}).encode("utf-8")
            resp_hdr = (
                "HTTP/1.1 200 OK\r\n"
                "Content-Type: application/json\r\n"
                f"Content-Length: {len(body)}\r\n"
                "Connection: close\r\n\r\n"
            ).encode("ascii")
            writer.write(resp_hdr + body)
            await writer.drain()
            return

        if parsed.path == "/echo":
            body = json.dumps({"path": raw_path, "headers": headers}).encode("utf-8")
            resp_hdr = (
                "HTTP/1.1 200 OK\r\n"
                "Content-Type: application/json\r\n"
                f"Content-Length: {len(body)}\r\n"
                "Connection: close\r\n\r\n"
            ).encode("ascii")
            writer.write(resp_hdr + body)
            await writer.drain()
            return

        # Default: bulk download endpoint (/down or /file.bin)
        total_size = int(qs.get("bytes", ["4194304"])[0] or 4194304)
        total_size = min(max(1024, total_size), 512 * 1024 * 1024)
        p_type = qs.get("type", ["random"])[0]
        block, content_type = build_payload_block(p_type, size=65536)

        start_b = 0
        end_b = total_size - 1
        is_206 = False
        range_hdr = headers.get("range", "")
        if range_hdr:
            m = _RANGE_RE.search(range_hdr)
            if m:
                start_b = min(int(m.group(1)), total_size - 1)
                if m.group(2):
                    end_b = min(int(m.group(2)), total_size - 1)
                is_206 = True

        slice_len = max(0, end_b - start_b + 1)
        status_line = "HTTP/1.1 206 Partial Content\r\n" if is_206 else "HTTP/1.1 200 OK\r\n"
        extra_range = f"Content-Range: bytes {start_b}-{end_b}/{total_size}\r\n" if is_206 else ""
        resp_hdr = (
            f"{status_line}"
            f"Content-Type: {content_type}\r\n"
            f"Content-Length: {slice_len}\r\n"
            "Accept-Ranges: bytes\r\n"
            f"{extra_range}"
            "Connection: close\r\n\r\n"
        ).encode("ascii")
        writer.write(resp_hdr)
        await writer.drain()

        if method == "HEAD":
            return

        sent = 0
        while sent < slice_len:
            step = min(len(block), slice_len - sent)
            writer.write(block[:step])
            await writer.drain()
            sent += step
    except Exception as exc:
        logger.debug("VPS server client handler closed: %s", exc)
    finally:
        try:
            writer.close()
            await writer.wait_closed()
        except Exception:
            pass


async def run_vps_server_async(
    host: str = "127.0.0.1",
    port: int = 8765,
    *,
    enable_udp: bool = True,
    duration_sec: float | None = None,
) -> dict[str, Any]:
    """Start the asyncio TCP + optional UDP test server and run for `duration_sec` (or until cancelled)."""
    server = await asyncio.start_server(handle_tcp_client, host, port)
    sockets: tuple[Any, ...] = tuple(server.sockets or ())
    bound_port = int(sockets[0].getsockname()[1]) if sockets else port

    udp_transport = None
    if enable_udp:
        loop = asyncio.get_running_loop()
        try:
            udp_transport, _ = await loop.create_datagram_endpoint(
                UDPThrottleEchoProtocol,
                local_addr=(host, bound_port),
            )
        except OSError:
            udp_transport = None

    try:
        async with server:
            if duration_sec is not None:
                await asyncio.sleep(duration_sec)
            else:
                await server.serve_forever()
    finally:
        if udp_transport is not None:
            udp_transport.close()

    return {"host": host, "port": bound_port, "udp": udp_transport is not None}


def run_vps_server_blocking(
    *,
    host: str = "127.0.0.1",
    tcp_port: int = 8765,
    udp_port: int = 8766,
    duration_sec: float | None = None,
) -> dict[str, Any]:
    """Run the asyncio TCP/UDP VPS ground-truth test server until interrupted or `duration_sec` elapses."""
    _ = udp_port
    return asyncio.run(
        run_vps_server_async(
            host=host,
            port=tcp_port,
            enable_udp=True,
            duration_sec=duration_sec,
        )
    )
