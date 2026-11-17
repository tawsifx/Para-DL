"""TDD: GitHub archive URLs must be accelerated through a Range-capable mirror."""

from __future__ import annotations

from paradl.net.accelerate import accelerate_url, is_github_archive

URL = "https://github.com/python/cpython/archive/refs/heads/main.zip"


def test_is_github_archive() -> None:
    assert is_github_archive(URL) is True
    assert is_github_archive("https://github.com/a/b/archive/refs/tags/v1.zip") is True
    assert is_github_archive("https://example.com/archive/x.zip") is False


def test_accelerate_github_archive(monkeypatch) -> None:
    monkeypatch.setenv("PARADL_GITHUB_ARCHIVE_MIRROR", "https://gh-proxy.com/")
    assert accelerate_url(URL) == "https://gh-proxy.com/" + URL


def test_accelerate_disabled(monkeypatch) -> None:
    monkeypatch.setenv("PARADL_GITHUB_ARCHIVE_MIRROR", "")
    assert accelerate_url(URL) is None


def test_accelerate_ignores_non_archive(monkeypatch) -> None:
    monkeypatch.setenv("PARADL_GITHUB_ARCHIVE_MIRROR", "https://gh-proxy.com/")
    assert accelerate_url("https://example.com/file.zip") is None
