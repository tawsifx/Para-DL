"""Protocol handshake, frame builders, and error-kind contract tests.

Golden source: ``rust/crates/paradl-oracle/fixtures/media_protocol.json``
(copied into ``media/tests/fixtures/media_protocol_golden.json``).
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest
from paradl_media import protocol
from paradl_media.protocol import (
    DATA_FIELDS,
    ERROR_FIELDS,
    ERROR_KIND_TO_EXIT_CODE,
    PROGRESS_FIELDS,
    RESULT_FIELDS,
    VERSIONS_FIELDS,
    WorkerError,
    check_protocol_version,
    error_frame,
    progress_frame,
    result_frame,
)

FIXTURES = Path(__file__).parent / "fixtures"
GOLDEN = json.loads((FIXTURES / "media_protocol_golden.json").read_text())


@pytest.mark.parametrize("case", GOLDEN["version_conformance"], ids=lambda c: c["label"])
def test_version_conformance_matrix(case: dict) -> None:
    accepted, diagnostic, exit_code = check_protocol_version(case["protocol"])
    assert accepted is case["accepted"]
    assert diagnostic == case["diagnostic"]
    assert exit_code == case["exit_code"]


def test_golden_field_lists() -> None:
    assert list(PROGRESS_FIELDS) == [
        "protocol", "request_id", "frame", "downloaded_bytes", "total_bytes", "speed_bps", "eta_seconds"
    ]
    assert list(RESULT_FIELDS) == [
        "protocol", "request_id", "frame", "ok", "versions", "caps", "data"
    ]
    assert list(ERROR_FIELDS) == ["protocol", "request_id", "frame", "ok", "error", "versions"]
    assert list(VERSIONS_FIELDS) == [
        "worker_version", "yt_dlp_version", "mutagen_version", "ffmpeg_version"
    ]
    assert list(DATA_FIELDS) == ["filename", "http_headers", "audio_format_produced", "paths"]


def test_frame_key_order_byte_exact() -> None:
    pf = progress_frame("r1", downloaded_bytes=0, total_bytes=None, speed_bps=0, eta_seconds=None)
    assert list(pf.keys()) == [
        "protocol", "request_id", "frame", "downloaded_bytes", "total_bytes", "speed_bps", "eta_seconds"
    ]
    versions = dict.fromkeys(VERSIONS_FIELDS, "x")
    rf = result_frame("r1", versions=versions, caps=["ejs"], data={})
    assert list(rf.keys()) == list(RESULT_FIELDS)
    ef = error_frame("r1", kind="usage", message="bad", versions=versions)
    assert list(ef.keys()) == list(ERROR_FIELDS)
    assert list(ef["error"].keys()) == ["kind", "message"]


def test_every_frame_carries_protocol_and_request_id() -> None:
    versions = dict.fromkeys(VERSIONS_FIELDS, "x")
    for frame in (
        progress_frame("abc", downloaded_bytes=1, total_bytes=2, speed_bps=3.0, eta_seconds=0.5),
        result_frame("abc", versions=versions, caps=[], data={}),
        error_frame("abc", kind="download", message="oops", versions=versions),
    ):
        assert frame["protocol"] == "paradl-media/1"
        assert frame["request_id"] == "abc"


def test_error_kind_to_exit_code_exact() -> None:
    assert GOLDEN["error_kind_to_exit_code"] == ERROR_KIND_TO_EXIT_CODE
    assert 3 not in ERROR_KIND_TO_EXIT_CODE.values()
    assert protocol.WORKER_NEVER_EMITS == GOLDEN["worker_never_emits"] == 3


@pytest.mark.parametrize("kind", list(ERROR_KIND_TO_EXIT_CODE))
def test_worker_error_exit_code(kind: str) -> None:
    err = WorkerError(kind, "msg")
    assert err.kind == kind
    assert err.message == "msg"
    assert err.exit_code == ERROR_KIND_TO_EXIT_CODE[kind]


def test_invalid_error_kind_rejected() -> None:
    with pytest.raises(ValueError):
        WorkerError("bogus", "msg")


def test_progress_field_types() -> None:
    pf = progress_frame("r", downloaded_bytes=10, total_bytes=20, speed_bps=1.5, eta_seconds=None)
    assert isinstance(pf["downloaded_bytes"], int)
    assert isinstance(pf["total_bytes"], int)
    assert isinstance(pf["speed_bps"], float)
    assert pf["eta_seconds"] is None
