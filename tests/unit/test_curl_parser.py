"""Unit tests for Phase E: 'Copy as cURL' parser, cookie extraction, and security validation."""

from __future__ import annotations

from pathlib import Path

import pytest

from pdl.curl_parser import create_netscape_cookie_jar, parse_curl_command

pytestmark = pytest.mark.unit


CHROME_CURL_SAMPLE = """curl 'https://downloads.example.org/packages/release-v1.0.tar.gz' \\
  -H 'accept: text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8' \\
  -H 'accept-language: en-US,en;q=0.9' \\
  -H 'cookie: session_id=s%3Aabc123xyz; secure_token=tok_secret_999' \\
  -H 'referer: https://downloads.example.org/dashboard' \\
  -H 'user-agent: Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36' \\
  --compressed"""

FIREFOX_CURL_SAMPLE = """curl "https://data.example.com/datasets/archive.zip" \\
  -H "User-Agent: Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:132.0) Gecko/20100101 Firefox/132.0" \\
  -H "Accept: */*" \\
  -H "Referer: https://data.example.com/explore" \\
  --compressed \\
  -b "auth_user=developer; auth_exp=1799999999" \\
  -o "downloaded_archive.zip" """


def test_parse_chrome_curl_sample() -> None:
    """Parse real Chrome 'Copy as cURL' command with headers, cookies, and referer."""
    parsed = parse_curl_command(CHROME_CURL_SAMPLE)
    assert parsed.url == "https://downloads.example.org/packages/release-v1.0.tar.gz"
    assert parsed.compressed is True
    assert parsed.referer == "https://downloads.example.org/dashboard"
    assert parsed.user_agent is not None
    assert "Mozilla/5.0" in parsed.user_agent
    assert parsed.cookies.get("session_id") == "s%3Aabc123xyz"
    assert parsed.cookies.get("secure_token") == "tok_secret_999"
    # General headers
    assert "accept" in parsed.headers


def test_parse_firefox_curl_sample() -> None:
    """Parse real Firefox 'Copy as cURL' command with -b cookie and -o filename."""
    parsed = parse_curl_command(FIREFOX_CURL_SAMPLE)
    assert parsed.url == "https://data.example.com/datasets/archive.zip"
    assert parsed.compressed is True
    assert parsed.referer == "https://data.example.com/explore"
    assert parsed.cookies.get("auth_user") == "developer"
    assert parsed.cookies.get("auth_exp") == "1799999999"
    assert parsed.output_filename == "downloaded_archive.zip"


@pytest.mark.parametrize(
    "rejected_cmd",
    [
        "curl -X POST https://example.com/upload --data 'param=value'",
        "curl https://example.com/api -d '{\"key\": 1}'",
        "curl https://example.com/file --data-raw 'raw_body'",
        "curl https://example.com/file --data-binary @image.png",
        "curl https://example.com/file -F 'file=@dump.sql'",
        "curl -X DELETE https://example.com/resource",
        "curl ftp://ftp.example.com/file.iso",
        "curl file:///etc/passwd",
        "curl",  # missing URL
    ],
)
def test_reject_unsupported_curl_forms(rejected_cmd: str) -> None:
    """Ensure POST/data payloads, unsupported HTTP methods, and non-HTTP protocols are rejected."""
    with pytest.raises(ValueError):
        parse_curl_command(rejected_cmd)


def test_sanitize_curl_output_filename() -> None:
    """Directory traversals in -o/--output are sanitized to prevent arbitrary file writes."""
    cmd = "curl 'https://example.com/file.bin' -o '../../../../escaped.bin'"
    parsed = parse_curl_command(cmd)
    assert parsed.output_filename == "escaped.bin"
    assert "/" not in (parsed.output_filename or "")


def test_create_netscape_cookie_jar_format_and_0600_permissions() -> None:
    """Verify Netscape cookie jar format, domain scoping, and 0600 file permissions."""
    cookies = {
        "auth_token": "sec_12345",
        "theme": "dark",
    }
    jar_path, cleanup = create_netscape_cookie_jar(cookies, host="downloads.example.org", is_secure=True)
    try:
        assert jar_path.exists()
        # Verify file permissions: 0600 (owner read/write only)
        mode = jar_path.stat().st_mode & 0o777
        assert mode == 0o600, f"Expected 0600 file mode, got octal {oct(mode)}"

        content = jar_path.read_text(encoding="utf-8")
        assert "# Netscape HTTP Cookie File" in content
        # Domain scoped
        assert "downloads.example.org\tFALSE\t/\tTRUE\t2147483647\tauth_token\tsec_12345" in content
        assert "downloads.example.org\tFALSE\t/\tTRUE\t2147483647\ttheme\tdark" in content
    finally:
        cleanup()

    # Guaranteed deletion test
    assert not jar_path.exists()


def test_curl_parser_user_agent_referer_compressed(tmp_path: Path) -> None:
    """Verify -A, -e, and --compressed flag parsing and cookie file loading."""
    # 1. Cookie file loading
    cookie_file = tmp_path / "cookies.txt"
    cookie_file.write_text("example.com\tFALSE\t/\tTRUE\t2147483647\tfile_sess\txyz789\n", encoding="utf-8")

    cmd = (
        f"curl 'https://example.com/file.bin' -A 'CustomBot/2.0' -e 'https://ref.example.com' "
        f"--compressed -b '{cookie_file}'"
    )
    req = parse_curl_command(cmd)
    assert req.user_agent == "CustomBot/2.0"
    assert req.referer == "https://ref.example.com"
    assert req.compressed is True
    assert req.cookies.get("file_sess") == "xyz789"

    # 2. Flag validation errors
    with pytest.raises(ValueError, match="missing user-agent"):
        parse_curl_command("curl 'https://example.com' -A")

    with pytest.raises(ValueError, match="missing referer"):
        parse_curl_command("curl 'https://example.com' -e")


def test_sanitize_filename_and_multiline_curl() -> None:
    """Verify filename sanitization and parsing multi-line backslash-escaped commands."""
    from pdl.curl_parser import sanitize_filename

    assert sanitize_filename("safe_file.tar.gz") == "safe_file.tar.gz"
    assert sanitize_filename("../../../etc/passwd") == "passwd"
    assert sanitize_filename("file:with*bad?chars|.bin") == "file_with_bad_chars_.bin"

    multiline = """curl 'https://example.com/data.bin' \\
        -H 'Host: example.com' \\
        -H 'Accept: */*'"""
    parsed = parse_curl_command(multiline)
    assert parsed.url == "https://example.com/data.bin"


def test_curl_parser_edge_cases_and_missing_flags() -> None:
    """Verify curl parser error branches: missing arguments, invalid scheme, unsupported methods."""
    with pytest.raises(ValueError, match="missing HTTP method"):
        parse_curl_command("curl 'https://example.com' -X")

    with pytest.raises(ValueError, match="is not supported"):
        parse_curl_command("curl 'https://example.com' -X DELETE")

    with pytest.raises(ValueError, match="missing header argument"):
        parse_curl_command("curl 'https://example.com' -H")

    with pytest.raises(ValueError, match="missing cookie argument"):
        parse_curl_command("curl 'https://example.com' -b")

    with pytest.raises(ValueError, match="missing output argument"):
        parse_curl_command("curl 'https://example.com' -o")

    with pytest.raises(ValueError, match="Unsupported URL scheme"):
        parse_curl_command("curl 'ftp://example.com/file.bin'")

    with pytest.raises(ValueError, match="No URL found"):
        parse_curl_command("curl")

    with pytest.raises(ValueError, match="missing hostname"):
        parse_curl_command("curl 'https://'")
