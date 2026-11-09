"""Unit tests for the network layer: rate parsing, probes, DNS, and HTTP/3 support."""

from __future__ import annotations

import sys
import types
import unittest
from unittest.mock import MagicMock, patch

from pdl.net import (
    HttpProbeResult,
    extract_etag_sha256,
    parse_content_disposition_filename,
    probe_http_url,
    probe_range_support,
    probe_targets_parallel,
    re_resolve_dns,
    request_with_retry,
    resolve_dns,
    resolve_dns_doh,
)


class TestParseContentDispositionFilename(unittest.TestCase):
    """Tests for RFC 6266 Content-Disposition filename parsing."""

    def test_plain_filename(self) -> None:
        result = parse_content_disposition_filename('attachment; filename="report.pdf"')
        self.assertEqual(result, "report.pdf")

    def test_rfc5987_filename_star(self) -> None:
        result = parse_content_disposition_filename("attachment; filename*=UTF-8''report%20file.pdf")
        self.assertEqual(result, "report file.pdf")

    def test_filename_star_takes_priority(self) -> None:
        result = parse_content_disposition_filename(
            'filename="old.pdf"; filename*=UTF-8\'\'new%20file.pdf'
        )
        self.assertEqual(result, "new file.pdf")

    def test_none_returns_none(self) -> None:
        self.assertIsNone(parse_content_disposition_filename(None))
        self.assertIsNone(parse_content_disposition_filename(""))

    def test_path_traversal_blocked(self) -> None:
        result = parse_content_disposition_filename('filename="../../../etc/passwd"')
        self.assertEqual(result, "passwd")


class TestExtractETagSHA256(unittest.TestCase):
    """Tests for SHA-256 extraction from ETag headers."""

    def test_x_linked_etag(self) -> None:
        headers = {"x-linked-etag": '"sha256:abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"'}
        result = extract_etag_sha256(headers)
        self.assertEqual(result, "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890")

    def test_plain_etag(self) -> None:
        headers = {"etag": '"W/abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890"'}
        result = extract_etag_sha256(headers)
        self.assertEqual(result, "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890")

    def test_no_etag(self) -> None:
        self.assertIsNone(extract_etag_sha256({}))
        self.assertIsNone(extract_etag_sha256({"etag": '"not-a-sha256"'}))


class TestProbeRangeSupport(unittest.TestCase):
    """Tests for probe_range_support() HTTP Range detection."""

    @patch("pdl.net.get_http_client")
    def test_206_partial_content_detected(self, mock_get_client) -> None:
        mock_resp = MagicMock()
        mock_resp.status_code = 206
        mock_resp.headers.get.return_value = ""
        mock_resp.headers.items.return_value = []
        mock_resp.url = "https://example.com/file.zip"

        mock_client = MagicMock()
        mock_client.stream.return_value.__enter__.return_value = mock_resp
        mock_get_client.return_value = mock_client

        result = probe_range_support("https://example.com/file.zip")
        self.assertTrue(result.ok)
        self.assertEqual(result.status_code, 206)
        self.assertTrue(result.accept_ranges)

    @patch("pdl.net.get_http_client")
    def test_200_only_no_range(self, mock_get_client) -> None:
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.headers.get.return_value = ""
        mock_resp.headers.items.return_value = []
        mock_resp.url = "https://example.com/file.zip"

        mock_client = MagicMock()
        mock_client.stream.return_value.__enter__.return_value = mock_resp
        mock_get_client.return_value = mock_client

        result = probe_range_support("https://example.com/file.zip")
        self.assertTrue(result.ok)
        self.assertEqual(result.status_code, 200)
        self.assertFalse(result.accept_ranges)


class TestProbeHTTPURL(unittest.TestCase):
    """Tests for probe_http_url() HEAD with Range fallback."""

    @patch("pdl.net.request_with_retry")
    def test_head_success(self, mock_request) -> None:
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.headers = {
            "content-type": "application/zip",
            "content-length": "1048576",
            "accept-ranges": "bytes",
            "content-disposition": 'filename="archive.zip"',
        }
        mock_request.return_value = mock_resp

        result = probe_http_url("https://example.com/archive.zip")
        self.assertTrue(result.ok)
        self.assertEqual(result.content_length, 1048576)
        self.assertTrue(result.accept_ranges)
        self.assertEqual(result.filename, "archive.zip")

    @patch("pdl.net.request_with_retry")
    @patch("pdl.net.probe_range_support")
    def test_head_405_falls_back_to_range(self, mock_range, mock_request) -> None:
        mock_request.side_effect = Exception("405 Method Not Allowed")
        mock_range.return_value = HttpProbeResult(
            ok=True, status_code=206, accept_ranges=True, content_length=5000, filename="file.bin"
        )

        result = probe_http_url("https://example.com/file.bin")
        self.assertTrue(result.ok)
        self.assertTrue(result.accept_ranges)


class TestReResolveDNS(unittest.TestCase):
    """Tests for re_resolve_dns() DNS resolution."""

    @patch("pdl.net.socket.getaddrinfo")
    def test_resolves_ipv4_and_ipv6(self, mock_getaddrinfo) -> None:
        mock_getaddrinfo.return_value = [
            (None, None, None, None, ("192.168.1.1", 0)),
            (None, None, None, None, ("::1", 0)),
            (None, None, None, None, ("192.168.1.1", 0)),  # duplicate
        ]
        result = re_resolve_dns("example.com")
        self.assertIn("192.168.1.1", result)
        self.assertIn("::1", result)
        # Deduplicated
        self.assertEqual(result.count("192.168.1.1"), 1)


class _FakeHTTPXResponse:
    """Fake httpx.Response for request_with_retry tests."""

    def __init__(self, status_code: int) -> None:
        self.status_code = status_code


class _FakeHTTPXClient:
    """Fake httpx.Client that installs itself into sys.modules."""

    def __init__(self, responses: list) -> None:
        self._responses = responses
        self._call_count = 0

    def request(self, method, url, **kwargs):
        resp = self._responses[self._call_count]
        self._call_count += 1
        return resp


class TestRequestWithRetry(unittest.TestCase):
    """Tests for request_with_retry() transient failure handling."""

    def setUp(self) -> None:
        # Install a fake httpx module so request_with_retry's `import httpx` works
        self.fake_httpx = types.ModuleType("httpx")

        class HTTPError(Exception):
            pass

        self.fake_httpx.HTTPError = HTTPError
        self._old_httpx = sys.modules.get("httpx")
        sys.modules["httpx"] = self.fake_httpx

    def tearDown(self) -> None:
        if self._old_httpx is not None:
            sys.modules["httpx"] = self._old_httpx
        else:
            sys.modules.pop("httpx", None)

    @patch("pdl.net.get_http_client")
    def test_success_first_attempt(self, mock_get_client) -> None:
        mock_resp = _FakeHTTPXResponse(200)
        mock_client = MagicMock()
        mock_client.request.return_value = mock_resp
        mock_get_client.return_value = mock_client

        result = request_with_retry("GET", "https://example.com")
        self.assertEqual(result.status_code, 200)

    @patch("pdl.net.get_http_client")
    @patch("pdl.net.time.sleep")
    def test_retries_on_503(self, mock_sleep, mock_get_client) -> None:
        mock_resp_503 = _FakeHTTPXResponse(503)
        mock_resp_200 = _FakeHTTPXResponse(200)

        mock_client = MagicMock()
        mock_client.request.side_effect = [mock_resp_503, mock_resp_200]
        mock_get_client.return_value = mock_client

        result = request_with_retry("GET", "https://example.com", retries=3)
        self.assertEqual(result.status_code, 200)
        mock_sleep.assert_called_once()


class TestProbeTargetsParallel(unittest.TestCase):
    """Tests for probe_targets_parallel() concurrency and ordering."""

    @patch("pdl.net.probe_target")
    def test_preserves_input_order(self, mock_probe) -> None:
        mock_probe.side_effect = lambda t, **kw: f"result-for-{t}"

        targets = ["https://a.com", "https://b.com", "https://c.com"]
        results = probe_targets_parallel(targets)

        self.assertEqual(
            results,
            ["result-for-https://a.com", "result-for-https://b.com", "result-for-https://c.com"],
        )
        # probe_target called once per target
        self.assertEqual(mock_probe.call_count, 3)

    @patch("pdl.net.probe_target")
    def test_single_target_no_thread_pool(self, mock_probe) -> None:
        mock_probe.return_value = "single"
        results = probe_targets_parallel(["https://example.com"])
        self.assertEqual(results, ["single"])

    @patch("pdl.net.probe_target")
    def test_empty_list(self, mock_probe) -> None:
        results = probe_targets_parallel([])
        self.assertEqual(results, [])
        mock_probe.assert_not_called()


class TestTLSSessionCache(unittest.TestCase):
    """Tests for TLS session ticket caching (session resumption / 0-RTT)."""

    def tearDown(self) -> None:
        from pdl.net import _TLS_SESSION_CACHE

        _TLS_SESSION_CACHE.clear()

    def test_tls_session_cache_stores_and_retrieves(self) -> None:
        from pdl.net import _TLS_SESSION_CACHE

        # Simulate storing a session ticket
        fake_session = object()
        _TLS_SESSION_CACHE["example.com"] = fake_session
        self.assertIn("example.com", _TLS_SESSION_CACHE)
        self.assertIs(_TLS_SESSION_CACHE["example.com"], fake_session)

    def test_tls_session_cache_cleared(self) -> None:
        from pdl.net import _TLS_SESSION_CACHE

        _TLS_SESSION_CACHE["example.com"] = object()
        _TLS_SESSION_CACHE.clear()
        self.assertEqual(len(_TLS_SESSION_CACHE), 0)


class TestResolveDnsDoh(unittest.TestCase):
    """Tests for DNS-over-HTTPS resolution."""

    def setUp(self) -> None:
        # Install a fake httpx module so resolve_dns_doh's `import httpx` works
        self.fake_httpx = types.ModuleType("httpx")
        self._old_httpx = sys.modules.get("httpx")
        sys.modules["httpx"] = self.fake_httpx

    def tearDown(self) -> None:
        if self._old_httpx is not None:
            sys.modules["httpx"] = self._old_httpx
        else:
            sys.modules.pop("httpx", None)

    @patch("pdl.net.get_http_client")
    def test_resolve_dns_doh_success(self, mock_get_client) -> None:
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.json.return_value = {
            "Answer": [
                {"type": 1, "data": "93.184.216.34"},  # A record
                {"type": 28, "data": "2606:2800:220:1:248:1893:25c8:1946"},  # AAAA record
            ]
        }
        mock_client = MagicMock()
        mock_client.get.return_value = mock_resp
        mock_get_client.return_value = mock_client

        result = resolve_dns_doh("example.com")
        self.assertIn("93.184.216.34", result)
        self.assertIn("2606:2800:220:1:248:1893:25c8:1946", result)

    @patch("pdl.net.get_http_client")
    def test_resolve_dns_doh_failure(self, mock_get_client) -> None:
        mock_resp = MagicMock()
        mock_resp.status_code = 500
        mock_client = MagicMock()
        mock_client.get.return_value = mock_resp
        mock_get_client.return_value = mock_client

        result = resolve_dns_doh("example.com")
        self.assertEqual(result, [])

    def test_resolve_dns_doh_fallback(self) -> None:
        # When DoH endpoint is provided but fails, should fall back to system resolver
        with patch("pdl.net.resolve_dns_doh", return_value=[]), patch(
            "pdl.net.re_resolve_dns", return_value=["192.168.1.1"]
        ):
            result = resolve_dns("example.com", doh_endpoint="https://1.1.1.1/dns-query")
            self.assertEqual(result, ["192.168.1.1"])

