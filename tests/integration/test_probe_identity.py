"""R1 regression guard: HTTP probes must request ``Accept-Encoding: identity``.

Migrated verbatim from ``tests/test_bug_fixes.py::test_r1_probe_requests_identity_encoding``.
"""

from __future__ import annotations

from unittest import mock

from paradl.net import probe_http_url, probe_range_support


def test_r1_probe_requests_identity_encoding():
    with mock.patch("paradl.net.get_http_client") as mock_client_fn:
        mock_client = mock_client_fn.return_value
        stream_ctx = mock_client.stream.return_value
        mock_resp = stream_ctx.__enter__.return_value
        mock_resp.status_code = 200
        mock_resp.url = "https://example.com/test.zip"
        mock_resp.headers = {"content-length": "100"}

        probe_range_support("https://example.com/test.zip")
        _args, kwargs = mock_client.stream.call_args
        assert kwargs["headers"].get("Accept-Encoding") == "identity"

    with mock.patch("paradl.net.request_with_retry") as mock_req:
        mock_resp = mock.MagicMock()
        mock_resp.status_code = 200
        mock_resp.url = "https://example.com/test.zip"
        mock_resp.headers = {"content-length": "100"}
        mock_req.return_value = mock_resp

        probe_http_url("https://example.com/test.zip")
        _args, kwargs = mock_req.call_args
        assert kwargs["headers"].get("Accept-Encoding") == "identity"
