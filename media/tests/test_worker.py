"""Golden request→response tests with yt-dlp/mutagen/ffmpeg fully mocked.

No live network. All op handlers are exercised by driving ``worker.run()`` with
an in-memory stdin/stdout, plus one real ``python -m paradl_media`` subprocess
smoke test for the one-shot transport.
"""

from __future__ import annotations

import io
import json
import os
import signal
import subprocess
import sys
from pathlib import Path

import paradl_media.worker as worker
import pytest
from paradl_media import media as media_mod
from paradl_media.media import MediaEngine
from paradl_media.protocol import WorkerError
from paradl_media.toolchain import TOOLCHAIN

FIXTURES = Path(__file__).parent / "fixtures"
MEDIA_ROOT = Path(__file__).parents[1]
GOLDEN = json.loads((FIXTURES / "media_protocol_golden.json").read_text())
GOLDEN_CASES = {case["name"]: case for case in GOLDEN["golden_cases"]}


class FakeYoutubeDL:
    """Minimal context-manager stand-in for ``yt_dlp.YoutubeDL``."""

    def __init__(self, opts: dict, info: dict | None = None, data: bytes = b"FAKEAUDIO"):
        self.opts = opts
        self._info = info
        self._data = data

    def __enter__(self) -> FakeYoutubeDL:
        return self

    def __exit__(self, *_args: object) -> bool:
        return False

    def extract_info(self, url: str, download: bool = False) -> dict | None:
        return self._info

    def download(self, urls: list[str]) -> None:
        outtmpl = self.opts.get("outtmpl", "")
        directory = Path(outtmpl).parent
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "raw.m4a").write_bytes(self._data)
        for hook in self.opts.get("progress_hooks", []):
            hook(
                {
                    "status": "downloading",
                    "downloaded_bytes": 5,
                    "total_bytes": 10,
                    "speed": 100.0,
                }
            )


def run_request(
    request: dict | None = None,
    *,
    stdin_line: str | None = None,
    **kwargs: object,
) -> tuple[int, list[dict]]:
    """Run one protocol cycle in-process and return ``(exit_code, frames)``."""
    line = stdin_line if stdin_line is not None else json.dumps(request) + "\n"
    stdout = io.StringIO()
    code = worker.run(io.StringIO(line), stdout, **kwargs)
    frames = [json.loads(raw) for raw in stdout.getvalue().splitlines()]
    return code, frames


@pytest.fixture
def fixed_versions(monkeypatch) -> None:
    """Pin versions/caps so the entire terminal frame is byte-comparable."""
    monkeypatch.setattr(worker, "collect_versions", lambda: dict(GOLDEN["fixed_versions"]))
    monkeypatch.setattr(worker, "collect_caps", lambda: list(GOLDEN["fixed_caps"]))


@pytest.fixture
def mocked_media(monkeypatch) -> None:
    """Mock yt-dlp/mutagen/ffmpeg: no network, no real conversion, no tagging."""
    monkeypatch.setattr(MediaEngine, "_ensure_dependencies", staticmethod(lambda target: None))
    monkeypatch.setattr(
        MediaEngine, "embed_metadata_and_cover", staticmethod(lambda *a, **k: None)
    )
    monkeypatch.setitem(TOOLCHAIN._cache, "ffmpeg", None)
    monkeypatch.setitem(TOOLCHAIN._cache, "afconvert", None)
    monkeypatch.setattr(media_mod, "is_macos", lambda: False)


def patch_fake_ytdl(monkeypatch, info: dict | None) -> None:
    # Keep it a classmethod: the engine calls both cls._new_youtube_dl(...) and
    # self._new_youtube_dl(...), so binding must match the original.
    monkeypatch.setattr(
        MediaEngine,
        "_new_youtube_dl",
        classmethod(lambda cls, opts: FakeYoutubeDL(opts, info)),
    )


# --- golden request/response cases -------------------------------------------

def test_golden_claims(fixed_versions) -> None:
    case = GOLDEN_CASES["claims_youtube"]
    code, frames = run_request(case["request"])
    assert code == case["expected_exit"]
    assert len(frames) == 1
    frame = frames[0]
    assert frame["frame"] == "result"
    assert frame["ok"] is True
    assert frame["request_id"] == case["request"]["request_id"]
    assert frame["versions"] == GOLDEN["fixed_versions"]
    assert frame["caps"] == GOLDEN["fixed_caps"]
    assert frame["data"] == case["expected_data"]


def test_golden_resolve_byte_exact(fixed_versions, mocked_media, monkeypatch) -> None:
    case = GOLDEN_CASES["resolve_audio_mp4a"]
    patch_fake_ytdl(monkeypatch, case["mock_info"])
    code, frames = run_request(case["request"])
    assert code == case["expected_exit"]
    assert len(frames) == 1
    frame = frames[0]
    assert frame["frame"] == "result"
    assert frame["data"] == case["expected_data"]
    # Byte-exact assertions called out explicitly by the charter:
    assert frame["data"]["http_headers"] == GOLDEN["byte_exact"]["coerced_http_headers"]
    assert frame["data"]["filename"] == "Artist - Song"
    assert frame["data"]["audio_format_produced"] == (
        "passthrough AAC (from mp4a source, no re-encode)"
    )


def test_golden_dependency_missing_ytdlp(fixed_versions, monkeypatch) -> None:
    case = GOLDEN_CASES["dependency_missing_ytdlp"]

    def missing(_target: str) -> None:
        raise WorkerError("dependency", "yt-dlp is required for media operations")

    monkeypatch.setattr(MediaEngine, "_ensure_dependencies", staticmethod(missing))
    code, frames = run_request(case["request"])
    assert code == case["expected_exit"] == 4
    assert frames[-1]["frame"] == "error"
    assert frames[-1]["ok"] is False
    assert frames[-1]["error"]["kind"] == case["expected_error"]["kind"] == "dependency"
    assert frames[-1]["versions"] == GOLDEN["fixed_versions"]


def test_real_dependency_probe_without_ytdlp(monkeypatch) -> None:
    """The real _ensure_dependencies fails closed when yt-dlp is absent."""
    monkeypatch.setitem(sys.modules, "yt_dlp", None)  # type: ignore[dict-item]
    with pytest.raises(WorkerError) as exc:
        MediaEngine._ensure_dependencies("https://example.com")
    assert exc.value.kind == "dependency"
    assert exc.value.exit_code == 4


def test_golden_usage_bad_op(fixed_versions) -> None:
    case = GOLDEN_CASES["usage_bad_op"]
    code, frames = run_request(case["request"])
    assert code == case["expected_exit"] == 2
    assert frames[-1]["error"]["kind"] == case["expected_error"]["kind"] == "usage"
    assert "bogus" in frames[-1]["error"]["message"]


# --- download_managed / finalize ---------------------------------------------

def test_download_managed_emits_progress_and_golden_data(
    fixed_versions, mocked_media, monkeypatch, tmp_path
) -> None:
    info = {
        "track": "Song (Official Audio)",
        "artist": "Artist",
        "acodec": "mp4a.40.2",
        "ext": "m4a",
        "http_headers": {"Referer": "https://www.youtube.com/", "X-Num": 7},
    }
    patch_fake_ytdl(monkeypatch, info)
    request = {
        "protocol": "paradl-media/1",
        "request_id": "dl-1",
        "op": "download_managed",
        "target": "https://www.youtube.com/watch?v=audio",
        "out_dir": str(tmp_path),
        "options": {"audio_only": True, "format": "m4a", "embed_thumbnail": False},
    }
    code, frames = run_request(request)

    assert code == 0
    assert [f["frame"] for f in frames] == ["progress", "result"]
    progress = frames[0]
    assert progress["downloaded_bytes"] == 5
    assert progress["total_bytes"] == 10
    assert progress["speed_bps"] == 100.0
    assert progress["eta_seconds"] is None
    assert progress["protocol"] == "paradl-media/1"
    assert progress["request_id"] == "dl-1"

    result = frames[-1]
    data = result["data"]
    assert data["filename"] == "Artist - Song"
    assert data["http_headers"] == {"Referer": "https://www.youtube.com/", "X-Num": "7"}
    assert data["audio_format_produced"] == "passthrough AAC (from mp4a source, no re-encode)"
    assert data["paths"] == [str(tmp_path / "Artist - Song.m4a")]
    assert data["bytes_downloaded"] == len(b"FAKEAUDIO")
    assert data["claims"] is True


def test_finalize_golden(fixed_versions, mocked_media, tmp_path) -> None:
    source = tmp_path / "raw.m4a"
    source.write_bytes(b"FAKEAUDIO")
    request = {
        "protocol": "paradl-media/1",
        "request_id": "fin-1",
        "op": "finalize",
        "target": str(source),
        "out_dir": str(tmp_path),
        "options": {
            "filename": "FINAL",
            "source_acodec": "mp4a.40.2",
            "audio_codec": "native",
            "embed_thumbnail": False,
            "title": "Final Track",
            "artist": "Artist",
        },
    }
    code, frames = run_request(request)
    assert code == 0
    assert len(frames) == 1
    data = frames[0]["data"]
    assert data["filename"] == "FINAL"
    assert data["http_headers"] == {}
    assert data["audio_format_produced"] == "passthrough AAC (from mp4a source, no re-encode)"
    assert data["paths"] == [str(tmp_path / "FINAL.m4a")]
    assert data["bytes_downloaded"] == len(b"FAKEAUDIO")


# --- handshake / fail-closed end to end --------------------------------------

@pytest.mark.parametrize("case", GOLDEN["version_conformance"], ids=lambda c: c["label"])
def test_handshake_end_to_end(case: dict, fixed_versions) -> None:
    request = {
        "protocol": case["protocol"],
        "request_id": "hs-1",
        "op": "claims",
        "target": "https://www.youtube.com/watch?v=x",
        "out_dir": "/tmp",
        "options": {},
    }
    code, frames = run_request(request)
    assert code == case["exit_code"]
    assert len(frames) == 1
    frame = frames[0]
    assert frame["protocol"] == "paradl-media/1"
    assert frame["request_id"] == "hs-1"
    if case["accepted"]:
        assert frame["frame"] == "result"
    else:
        assert frame["frame"] == "error"
        assert frame["error"]["kind"] == "dependency"
        assert frame["error"]["message"] == case["diagnostic"]
        assert frame["versions"] == GOLDEN["fixed_versions"]


@pytest.mark.parametrize(
    ("line", "diagnostic"),
    [
        ("not json\n", "unparseable protocol version"),
        ("", "protocol version missing"),
        ("[]\n", "unparseable protocol version"),
    ],
)
def test_malformed_stdin_fails_closed(line: str, diagnostic: str, fixed_versions) -> None:
    code, frames = run_request(stdin_line=line)
    assert code == 4
    assert frames[-1]["frame"] == "error"
    assert frames[-1]["error"]["kind"] == "dependency"
    assert frames[-1]["error"]["message"] == diagnostic


# --- SIGINT / process policy -------------------------------------------------

def test_sigint_returns_130_and_preserves_partials(monkeypatch, tmp_path) -> None:
    part = tmp_path / "song.m4a.pdl.part"
    partial = tmp_path / "song.m4a.pdl.json"
    part.write_bytes(b"partial")
    partial.write_text('{"event":"progress"}')

    def interrupt(*_args: object, **_kwargs: object) -> None:
        raise KeyboardInterrupt

    monkeypatch.setattr(worker, "handle_request", interrupt)
    request = {
        "protocol": "paradl-media/1",
        "request_id": "sig-1",
        "op": "claims",
        "target": "https://www.youtube.com/watch?v=x",
        "out_dir": str(tmp_path),
        "options": {},
    }
    code, frames = run_request(request)
    assert code == 130
    assert frames[-1]["error"]["kind"] == "interrupted"
    assert frames[-1]["request_id"] == "sig-1"
    assert part.read_bytes() == b"partial"
    assert partial.exists()


def test_signal_handler_installed_and_restored() -> None:
    original_int = signal.getsignal(signal.SIGINT)
    original_term = signal.getsignal(signal.SIGTERM)
    try:
        worker.install_signal_handlers()
        with pytest.raises(KeyboardInterrupt):
            signal.raise_signal(signal.SIGINT)
        with pytest.raises(KeyboardInterrupt):
            signal.raise_signal(signal.SIGTERM)
    finally:
        signal.signal(signal.SIGINT, original_int)
        signal.signal(signal.SIGTERM, original_term)


def test_stdout_protocol_only_stderr_has_logs(capsys) -> None:
    request = GOLDEN_CASES["claims_youtube"]["request"]
    _code, frames = run_request(request)
    captured = capsys.readouterr()
    assert len(frames) == 1
    assert "[paradl_media] op=claims" in captured.err
    assert captured.out == ""


def test_subprocess_one_shot_smoke() -> None:
    """Real `python -m paradl_media`: one protocol line on stdout, logs on stderr."""
    request = GOLDEN_CASES["claims_youtube"]["request"]
    env = dict(os.environ)
    env["PYTHONPATH"] = str(MEDIA_ROOT)
    proc = subprocess.run(
        [sys.executable, "-m", "paradl_media"],
        input=json.dumps(request) + "\n",
        capture_output=True,
        text=True,
        env=env,
        timeout=30,
        check=False,
    )
    assert proc.returncode == 0, proc.stderr
    lines = proc.stdout.splitlines()
    assert len(lines) == 1, f"stdout must be protocol-only, got: {proc.stdout!r}"
    frame = json.loads(lines[0])
    assert frame["protocol"] == "paradl-media/1"
    assert frame["request_id"] == "claims-1"
    assert frame["frame"] == "result"
    assert frame["data"]["claims"] is True
    assert set(frame["versions"]) == set(GOLDEN["fixed_versions"])
    assert "[paradl_media] op=claims" in proc.stderr
