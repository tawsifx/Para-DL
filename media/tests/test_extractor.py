"""Retargeted ``test_gen_extractor_classes_cached`` from ``test_aria2_rpc.py``.

The Rust client delegates ``claims`` to this worker; the live yt-dlp extractor
registry must be probed here and ``gen_extractor_classes()`` called only once.
"""

from __future__ import annotations

from paradl_media import extractor as media_extractor
from yt_dlp import extractor as yt_extractor


class _FakeIE:
    IE_NAME = "examplelive"

    @classmethod
    def suitable(cls, url: str) -> bool:
        return "example.org/watch" in url


class _GenericIE:
    IE_NAME = "generic"

    @classmethod
    def suitable(cls, url: str) -> bool:
        raise AssertionError("generic extractor must be skipped")


def test_gen_extractor_classes_cached_across_claims_url_calls(monkeypatch) -> None:
    """gen_extractor_classes() must be invoked once across many claims calls."""
    calls: list[int] = []

    def fake_gen():
        calls.append(1)
        return (_GenericIE, _FakeIE)

    monkeypatch.setattr(yt_extractor, "gen_extractor_classes", fake_gen)
    media_extractor._cached_extractor_classes.cache_clear()
    try:
        for i in range(10):
            assert media_extractor.ytdlp_extractor_claims_url(
                f"https://example.org/watch?v={i}"
            )
        assert len(calls) == 1, f"gen_extractor_classes called {len(calls)} times"
    finally:
        media_extractor._cached_extractor_classes.cache_clear()


def test_media_domain_short_circuits_extractor_registry(monkeypatch) -> None:
    """Known media domains claim without touching gen_extractor_classes()."""

    def fail_gen():
        raise AssertionError("registry must not be loaded for known media domains")

    monkeypatch.setattr(yt_extractor, "gen_extractor_classes", fail_gen)
    media_extractor._cached_extractor_classes.cache_clear()
    assert media_extractor.ytdlp_extractor_claims_url("https://www.youtube.com/watch?v=x")
    assert media_extractor.ytdlp_extractor_claims_url("https://soundcloud.com/a/b")


def test_non_claimed_url_returns_false(monkeypatch) -> None:
    monkeypatch.setattr(yt_extractor, "gen_extractor_classes", lambda: (_GenericIE, _FakeIE))
    media_extractor._cached_extractor_classes.cache_clear()
    assert not media_extractor.ytdlp_extractor_claims_url("https://example.com/plain.zip")
    media_extractor._cached_extractor_classes.cache_clear()
