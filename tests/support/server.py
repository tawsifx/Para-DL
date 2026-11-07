"""Unified Local Throttling Simulator for pdl Tests.

Provides ONE ThreadingHTTPServer with 11 named parametrizable profiles:
1. per_flow_cap: constant bit rate per connection
2. shared_cap: aggregate bandwidth divided among active flows
3. burst_then_throttle: high throughput burst followed by speed collapse
4. uniformly_slow: persistent low-throughput connection
5. loss_based_policing: drops connection after byte threshold to simulate packet loss / RST
6. queue_shaping: emulates buffer bloat and queue delays
7. time_of_day: shifts throughput based on simulated hour
8. volume_step_down: high speed for first N megabytes, then steps down
9. 200_only: rejects byte ranges with HTTP 200 OK
10. protocol_specific: alters speed/headers based on request path or headers
11. domestic_vs_international: fast for domestic routes, throttled for international
"""

from __future__ import annotations

import hashlib
import re
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any
from urllib.parse import parse_qs, urlparse

from pdl.clock import get_clock

_RANGE_RE = re.compile(r"bytes=(\d+)-(\d*)")


class SimulatorProfile:
    """Configurable parameters for the throttle simulator."""

    def __init__(
        self,
        name: str = "none",
        *,
        file_size: int = 4 * 1024 * 1024,
        per_flow_bps: float = 256 * 1024,
        shared_bps: float = 512 * 1024,
        burst_bps: float = 1200 * 1024,
        throttled_bps: float = 55 * 1024,
        burst_duration_sec: float = 0.65,
        uniform_slow_bps: float = 80 * 1024,
        loss_threshold_bytes: int = 512 * 1024,
        queue_delay_sec: float = 0.05,
        off_peak_bps: float = 2048 * 1024,
        peak_bps: float = 64 * 1024,
        step_down_bytes: int = 1024 * 1024,
        stepped_down_bps: float = 75 * 1024,
        support_range: bool = True,
        stall_after_bytes: int = 256 * 1024,
        fail_get_after_probe: bool = False,
        corrupt_once: bool = False,
    ) -> None:
        self.name = name
        self.file_size = file_size
        self.per_flow_bps = per_flow_bps
        self.shared_bps = shared_bps
        self.burst_bps = burst_bps
        self.throttled_bps = throttled_bps
        self.burst_duration_sec = burst_duration_sec
        self.uniform_slow_bps = uniform_slow_bps
        self.loss_threshold_bytes = loss_threshold_bytes
        self.queue_delay_sec = queue_delay_sec
        self.off_peak_bps = off_peak_bps
        self.peak_bps = peak_bps
        self.step_down_bytes = step_down_bytes
        self.stepped_down_bps = stepped_down_bps
        self.support_range = support_range
        self.stall_after_bytes = stall_after_bytes
        self.fail_get_after_probe = fail_get_after_probe
        self.corrupt_once = corrupt_once


class _UnifiedHTTPServer(ThreadingHTTPServer):
    allow_reuse_address = True
    daemon_threads = True
    request_queue_size = 64

    def __init__(
        self,
        server_address: tuple[str, int],
        handler_cls: type[BaseHTTPRequestHandler],
        profile: SimulatorProfile,
    ) -> None:
        super().__init__(server_address, handler_cls)
        self.profile = profile
        self.lock = threading.Lock()
        self._init_payload(profile.file_size)
        self.reset_state()

    def _init_payload(self, file_size: int) -> None:
        block = bytes((i * 31 + 17) & 0xFF for i in range(65536))
        repeats = (file_size // len(block)) + 1
        self.payload = (block * repeats)[:file_size]
        self.sha256_hex = hashlib.sha256(self.payload).hexdigest()

    def reset_state(self) -> None:
        with self.lock:
            self.active_flows: int = 0
            self.client_port_first_seen: dict[int, float] = {}
            self.distinct_client_ports: set[int] = set()
            self.request_count: int = 0
            self.range_request_count: int = 0
            self.bytes_served_total: int = 0
            self.stalled_ports: set[int] = set()
            self.stalled_generation_cleared: bool = False
            self.corrupted_once_done: bool = False
            self.loss_tripped: bool = False

    def handle_error(self, request: Any, client_address: Any) -> None:
        # Ignore client socket recycling disconnects
        return


class _UnifiedRequestHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, format: str, *args: Any) -> None:  # noqa: A002
        return

    @property
    def server_instance(self) -> _UnifiedHTTPServer:
        return self.server  # type: ignore[return-value]

    def _record_request(self, is_range: bool = False) -> float:
        client_port = int(self.client_address[1])
        now = time.monotonic()
        with self.server_instance.lock:
            self.server_instance.request_count += 1
            if is_range:
                self.server_instance.range_request_count += 1
            self.server_instance.distinct_client_ports.add(client_port)
            self.server_instance.client_port_first_seen[client_port] = now
            return now

    def do_HEAD(self) -> None:
        parsed = urlparse(self.path)
        prof = self.server_instance.profile

        if prof.name == "always_503":
            self.send_response(503)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if parsed.path == "/redirect":
            self.send_response(302)
            self.send_header("Location", "/file.bin")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if parsed.path == "/redirect-huge":
            self.send_response(302)
            self.send_header("Location", "/huge.bin")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if parsed.path in ("/endless.bin", "/huge.bin"):
            self._record_request(is_range=False)
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            if parsed.path == "/huge.bin":
                self.send_header("Content-Length", str(10 * 1024 * 1024 * 1024))
                self.send_header("Accept-Ranges", "bytes")
            self.end_headers()
            blk = self.server_instance.payload[:16384]
            try:
                for _ in range(4096):
                    self.wfile.write(blk)
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError, OSError):
                return
            return
        if parsed.path == "/redirect-huge":
            self.send_response(302)
            self.send_header("Location", "/huge.bin")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if parsed.path == "/endless.bin":
            self._record_request(is_range=False)
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.end_headers()
            return
        if parsed.path == "/huge.bin":
            self._record_request(is_range=False)
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(10 * 1024 * 1024 * 1024))
            self.send_header("Accept-Ranges", "bytes")
            self.end_headers()
            return

        self._record_request(is_range=False)
        no_range = (parsed.path == "/no-range.bin") or (not prof.support_range) or (prof.name == "200_only")

        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(len(self.server_instance.payload)))
        self.send_header("ETag", f'"{self.server_instance.sha256_hex}"')
        self.send_header("X-Linked-ETag", f'"{self.server_instance.sha256_hex}"')
        if not no_range:
            self.send_header("Accept-Ranges", "bytes")
        if parsed.path == "/custom-disp":
            self.send_header(
                "Content-Disposition",
                "attachment; filename=\"fallback.bin\"; filename*=UTF-8''hello%20world%21.bin",
            )
        self.end_headers()

    def do_GET(self) -> None:
        parsed = urlparse(self.path)
        qs = parse_qs(parsed.query)
        prof = self.server_instance.profile
        mode_override = qs.get("mode", [None])[0]
        mode = mode_override or prof.name

        if mode == "always_503":
            self._record_request(is_range=False)
            self.send_response(503)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return

        if parsed.path == "/redirect":
            self.send_response(302)
            self.send_header("Location", "/file.bin")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if parsed.path == "/redirect-huge":
            self.send_response(302)
            self.send_header("Location", "/huge.bin")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        if parsed.path in ("/endless.bin", "/huge.bin"):
            self._record_request(is_range=False)
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            if parsed.path == "/huge.bin":
                self.send_header("Content-Length", str(10 * 1024 * 1024 * 1024))
                self.send_header("Accept-Ranges", "bytes")
            self.end_headers()
            blk = self.server_instance.payload[:16384]
            try:
                for _ in range(4096):
                    self.wfile.write(blk)
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError, OSError):
                return
            return

        no_range = (
            (parsed.path == "/no-range.bin") or (not prof.support_range) or (mode in ("200_only", "no_range_200"))
        )
        range_hdr = self.headers.get("Range")
        total_len = len(self.server_instance.payload)
        start_byte = 0
        end_byte = total_len - 1
        is_206 = False

        if range_hdr and not no_range:
            m = _RANGE_RE.search(range_hdr)
            if m:
                start_byte = min(int(m.group(1)), total_len - 1)
                if m.group(2):
                    end_byte = min(int(m.group(2)), total_len - 1)
                is_206 = True

        flow_start_ts = self._record_request(is_range=is_206)
        slice_data = self.server_instance.payload[start_byte : end_byte + 1]
        content_len = len(slice_data)

        # Fail full GET after 1-byte Range probe if configured
        if prof.fail_get_after_probe and content_len > 1:
            self.send_response(503)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return

        # Corrupt single chunk once for metalink repair verification
        if prof.corrupt_once and content_len > 1:
            with self.server_instance.lock:
                if not self.server_instance.corrupted_once_done:
                    self.server_instance.corrupted_once_done = True
                    slice_data = bytes(b ^ 0xFF for b in slice_data[:64]) + slice_data[64:]

        if is_206:
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {start_byte}-{end_byte}/{total_len}")
        else:
            self.send_response(200)

        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(content_len))
        self.send_header("ETag", f'"{self.server_instance.sha256_hex}"')
        self.send_header("X-Linked-ETag", f'"{self.server_instance.sha256_hex}"')
        if not no_range:
            self.send_header("Accept-Ranges", "bytes")
        if parsed.path == "/custom-disp":
            self.send_header(
                "Content-Disposition",
                "attachment; filename=\"fallback.bin\"; filename*=UTF-8''hello%20world%21.bin",
            )
        self.end_headers()

        chunk_size = 2048
        offset = 0
        client_port = int(self.client_address[1])
        with self.server_instance.lock:
            self.server_instance.active_flows += 1
            if (
                mode == "stall_until_recycle"
                and content_len > 1
                and len(self.server_instance.stalled_ports) > 0
                and client_port not in self.server_instance.stalled_ports
            ):
                self.server_instance.stalled_generation_cleared = True

        try:
            while offset < content_len:
                if mode == "stall_until_recycle" and content_len > 1:
                    with self.server_instance.lock:
                        should_stall = (
                            not self.server_instance.stalled_generation_cleared
                            and self.server_instance.bytes_served_total >= prof.stall_after_bytes
                        )
                        if should_stall:
                            self.server_instance.stalled_ports.add(client_port)
                    if should_stall:
                        stall_deadline = get_clock().monotonic() + 15.0
                        while get_clock().monotonic() < stall_deadline:
                            with self.server_instance.lock:
                                if self.server_instance.stalled_generation_cleared:
                                    break
                            time.sleep(0.01)
                        return

                # Profile 5: Loss-based policing (abruptly reset/close connection)
                if mode == "loss_based_policing" and content_len > 1:
                    with self.server_instance.lock:
                        if (
                            not self.server_instance.loss_tripped
                            and self.server_instance.bytes_served_total >= prof.loss_threshold_bytes
                        ):
                            self.server_instance.loss_tripped = True
                            # Drop connection abruptly
                            self.close_connection = True
                            return

                step = min(chunk_size, content_len - offset)
                chunk = slice_data[offset : offset + step]
                t_chunk_start = time.monotonic()

                # Profile 6: Queue shaping (add packet delay buffer)
                if mode == "queue_shaping" and prof.queue_delay_sec > 0:
                    time.sleep(min(prof.queue_delay_sec, 0.05))

                self.wfile.write(chunk)
                self.wfile.flush()
                offset += step
                if content_len > 1:
                    with self.server_instance.lock:
                        self.server_instance.bytes_served_total += step

                target_bps = self._target_bps_for_flow(mode, flow_start_ts, parsed.path)
                if target_bps > 0:
                    expected_dt = step / target_bps
                    actual_dt = time.monotonic() - t_chunk_start
                    sleep_sec = expected_dt - actual_dt
                    if sleep_sec > 0.001:
                        time.sleep(min(sleep_sec, 0.5))
        except (BrokenPipeError, ConnectionResetError, OSError):
            return
        finally:
            with self.server_instance.lock:
                self.server_instance.active_flows = max(0, self.server_instance.active_flows - 1)

    def do_POST(self) -> None:
        self._record_request(is_range=False)
        cl = int(self.headers.get("Content-Length", "0") or 0)
        if cl > 0:
            _ = self.rfile.read(cl)
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        body = f'{{"received_bytes": {cl}}}'.encode()
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)
        self.wfile.flush()

    def _target_bps_for_flow(self, mode: str, flow_start_ts: float, req_path: str) -> float:
        prof = self.server_instance.profile
        if mode in ("none", "200_only", "no_range_200"):
            return 0.0
        if mode == "per_flow_cap":
            return prof.per_flow_bps
        if mode in ("shared_cap", "per_host_cap"):
            with self.server_instance.lock:
                flows = max(1, self.server_instance.active_flows)
            return prof.shared_bps / flows
        if mode == "burst_then_throttle":
            flow_age = time.monotonic() - flow_start_ts
            if flow_age <= prof.burst_duration_sec:
                return prof.burst_bps
            return prof.throttled_bps
        if mode == "uniformly_slow":
            with self.server_instance.lock:
                flows = max(1, self.server_instance.active_flows)
            return prof.uniform_slow_bps / flows
        if mode == "time_of_day":
            # Check simulated hour
            hour = int(get_clock().now() // 3600) % 24
            return prof.peak_bps if (8 <= hour <= 22) else prof.off_peak_bps
        if mode == "volume_step_down":
            with self.server_instance.lock:
                total_b = self.server_instance.bytes_served_total
            return prof.stepped_down_bps if total_b >= prof.step_down_bytes else prof.burst_bps
        if mode == "protocol_specific":
            if "/fast/" in req_path:
                return prof.burst_bps
            return prof.throttled_bps
        if mode == "domestic_vs_international":
            if "international" in req_path or self.headers.get("X-Region") == "intl":
                return prof.throttled_bps
            return prof.burst_bps
        if mode == "cached_vs_uncached":
            if "cached" in req_path and "uncached" not in req_path:
                return prof.burst_bps
            return prof.throttled_bps
        return 0.0


class ThrottleTestServer:
    """Ephemeral loopback HTTP server wrapper implementing the unified test simulator."""

    def __init__(self, profile: SimulatorProfile | None = None) -> None:
        self.profile = profile or SimulatorProfile()
        self.httpd = _UnifiedHTTPServer(("127.0.0.1", 0), _UnifiedRequestHandler, self.profile)
        self.port = int(self.httpd.server_address[1])
        self._thread = threading.Thread(
            target=self.httpd.serve_forever,
            kwargs={"poll_interval": 0.02},
            daemon=True,
        )

    @property
    def base_url(self) -> str:
        return f"http://127.0.0.1:{self.port}"

    @property
    def file_url(self) -> str:
        return f"{self.base_url}/file.bin"

    @property
    def no_range_url(self) -> str:
        return f"{self.base_url}/no-range.bin"

    @property
    def sha256_hex(self) -> str:
        return self.httpd.sha256_hex

    @property
    def payload(self) -> bytes:
        return self.httpd.payload

    @property
    def request_count(self) -> int:
        return self.httpd.request_count

    def set_profile(self, profile: SimulatorProfile) -> None:
        self.httpd.profile = profile
        if profile.file_size != len(self.httpd.payload):
            self.httpd._init_payload(profile.file_size)
        self.httpd.reset_state()

    def reset(self) -> None:
        self.httpd.reset_state()

    def start(self) -> ThrottleTestServer:
        self._thread.start()
        return self

    def stop(self) -> None:
        self.httpd.shutdown()
        self.httpd.server_close()
        self._thread.join(timeout=1.0)

    def __enter__(self) -> ThrottleTestServer:
        return self.start()

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        self.stop()
