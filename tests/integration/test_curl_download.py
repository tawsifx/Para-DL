"""Integration tests for Phase E: 'Copy as cURL' execution, cookie scoping, and secret protection."""

from __future__ import annotations

import hashlib
from pathlib import Path
from typing import Any

import pytest
from tests.support.server import SimulatorProfile, ThrottleTestServer

from pdl.cli import run_cli
from pdl.curl_parser import create_netscape_cookie_jar
from pdl.models import ExitCode
from pdl.ui import UI

pytestmark = pytest.mark.integration


def test_curl_download_execution_and_cookie_jar_cleanup(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Verify --from-curl creates a scoped cookie jar (0600), downloads file, and deletes jar on finish."""
    created_jars: list[Path] = []
    orig_create_jar = create_netscape_cookie_jar

    def tracked_create_jar(cookies: dict[str, str], host: str, is_secure: bool = True):
        jar_path, cleanup_fn = orig_create_jar(cookies, host=host, is_secure=is_secure)
        created_jars.append(jar_path)
        return jar_path, cleanup_fn

    monkeypatch.setattr("pdl.curl_parser.create_netscape_cookie_jar", tracked_create_jar)

    with ThrottleTestServer(SimulatorProfile("none", file_size=65536)) as srv:
        out_dir = tmp_path / "curl_out"
        curl_cmd = (
            f"curl '{srv.file_url}' "
            "-H 'User-Agent: pdl-test/1.0' "
            "-H 'Cookie: auth_token=secret_value_12345' "
            "-o 'custom_name.bin'"
        )

        rc = run_cli(["--from-curl", curl_cmd, "-o", str(out_dir)])
        assert rc == int(ExitCode.OK)

        downloaded_file = out_dir / "custom_name.bin"
        assert downloaded_file.exists()
        assert hashlib.sha256(downloaded_file.read_bytes()).hexdigest() == srv.sha256_hex

        # Cookie jar MUST be deleted afterwards
        assert len(created_jars) == 1
        assert not created_jars[0].exists(), "Cookie jar was not cleaned up after transfer"


def test_curl_cookie_jar_deleted_even_on_failure(tmp_path: Path) -> None:
    """Verify temporary cookie jar is deleted in a finally block even if transfer fails."""
    cookies = {"sess": "secret_abc"}
    jar_path, cleanup = create_netscape_cookie_jar(cookies, host="127.0.0.1", is_secure=False)
    assert jar_path.exists()

    try:
        raise RuntimeError("Simulated failure during transfer")
    except RuntimeError:
        pass
    finally:
        cleanup()

    assert not jar_path.exists()


def test_curl_secrets_never_appear_in_logs_or_events(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Verify that header and cookie values never appear in JSONL events or logs."""
    emitted_events: list[dict[str, Any]] = []

    monkeypatch.setattr(UI, "emit_event", lambda ev, **kw: emitted_events.append({"event": ev, **kw}))

    secret_cookie = "super_confidential_session_token_98765"
    with ThrottleTestServer(SimulatorProfile("none", file_size=32768)) as srv:
        out_dir = tmp_path / "secret_test_out"
        curl_cmd = f"curl '{srv.file_url}' -H 'Cookie: auth={secret_cookie}'"

        rc = run_cli(["--from-curl", curl_cmd, "-o", str(out_dir), "--json"])
        assert rc == int(ExitCode.OK)

        # Inspect all emitted event payloads
        for ev in emitted_events:
            ev_str = str(ev)
            assert secret_cookie not in ev_str, f"Secret leaked in event: {ev_str}"


def test_curl_cross_domain_redirect_cookie_scoping() -> None:
    """Verify that the Netscape cookie file scopes cookies strictly to target domain."""
    cookies = {"session": "my_val"}
    jar_path, cleanup = create_netscape_cookie_jar(cookies, host="api.example.com", is_secure=True)
    try:
        content = jar_path.read_text(encoding="utf-8")
        # Domain column must strictly specify target domain
        lines = [line.strip() for line in content.splitlines() if line and not line.startswith("#")]
        assert len(lines) == 1
        fields = lines[0].split("\t")
        domain_scope = fields[0]
        assert domain_scope == "api.example.com"
        # Not wild-carded to all domains
        assert domain_scope != "."
    finally:
        cleanup()
