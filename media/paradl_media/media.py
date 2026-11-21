"""Media extraction engine — the Python half of the media subsystem.

Extracted from ``src/paradl/engines/media.py`` with all ``paradl.*`` imports
removed. yt-dlp runs in-process (quiet, with a stderr-only logger); mutagen
embeds cover/tags; ffmpeg/afconvert/7z remain external processes.

Three public operations are implemented here and dispatched by :mod:`worker`:

``resolve``
    Extract metadata and decide direct-vs-managed handoff (no download).
``download_managed``
    Full yt-dlp download, conversion, and tagging; emits progress frames.
``finalize``
    Convert and tag an already-downloaded local file (no network).

Byte-exact wire contracts are honored: ``http_headers`` values are coerced to
``str`` with keys unchanged, ``filename`` comes from :func:`sanitize_filename`,
and ``audio_format_produced`` comes from :func:`classify_audio_production`.
"""

from __future__ import annotations

import contextlib
import logging
import shutil
import subprocess
import tempfile
import urllib.request
from collections.abc import Callable
from pathlib import Path
from typing import Any

from .extractor import ytdlp_extractor_claims_url
from .models import (
    classify_audio_production,
    parse_rate,
    sanitize_filename,
)
from .protocol import WorkerError, log
from .toolchain import TOOLCHAIN, is_macos

logger = logging.getLogger(__name__)

# Exact codec argument map from the legacy MediaEngine (H3 regression test).
CODEC_ARGS_MAP: dict[str, list[str]] = {
    "mp3": ["-vn", "-c:a", "libmp3lame", "-b:a", "256k"],
    "flac": ["-vn", "-c:a", "flac"],
    "opus": ["-vn", "-c:a", "libopus", "-b:a", "192k"],
    "wav": ["-vn", "-c:a", "pcm_s16le"],
    "aac": ["-vn", "-c:a", "aac", "-b:a", "256k", "-movflags", "+faststart"],
    "lossless": ["-vn", "-c:a", "aac", "-b:a", "256k", "-movflags", "+faststart"],
}

AUDIO_FORMAT_SELECTION = (
    "ba[ext=m4a][abr<=256]/ba[ext=aac][abr<=256]/ba[abr<=256]/bestaudio/best"
)

ProgressCallback = Callable[[int, int | None, float, float | None], None]


class _YtdlpLogger:
    """Route every yt-dlp message to stderr so stdout stays protocol-only."""

    def debug(self, msg: str) -> None:
        log(str(msg))

    def info(self, msg: str) -> None:
        log(str(msg))

    def warning(self, msg: str) -> None:
        log(f"warning: {msg}")

    def error(self, msg: str) -> None:
        log(f"error: {msg}")


def _coerce_headers(raw: Any) -> dict[str, str]:
    """Byte-exact contract: values → ``str``, keys unchanged."""
    return {str(k): str(v) for k, v in (raw or {}).items()}


def _is_youtube(url: str) -> bool:
    from urllib.parse import urlparse

    try:
        host = (urlparse(url.strip()).hostname or "").lower().rstrip(".")
    except ValueError:
        return False
    return host in ("youtube.com", "www.youtube.com", "m.youtube.com", "youtu.be")


def _codec_from_options(options: dict[str, Any]) -> str:
    """Resolve a requested codec from ``format`` / ``audio_codec`` options."""
    explicit = options.get("audio_codec")
    if explicit:
        return str(explicit).lower()
    fmt = str(options.get("format") or "").lower()
    for candidate in ("flac", "wav", "mp3", "opus", "aac", "m4a", "alac"):
        if candidate in fmt:
            return "aac" if candidate == "m4a" else candidate
    return "aac"


class MediaEngine:
    """Extracts, downloads, converts, and tags streaming media."""

    # --- yt-dlp option construction ------------------------------------------

    @staticmethod
    def _build_base_ydl_opts(
        *,
        ejs_remote: bool = False,
        player_client: str | None = None,
        cookies_from_browser: str | None = None,
        limit_rate: str | None = None,
    ) -> dict[str, Any]:
        opts: dict[str, Any] = {
            "quiet": True,
            "no_warnings": True,
            "noprogress": True,
            "no_color": True,
        }
        opts.update(TOOLCHAIN.get_ytdlp_js_opts(ejs_remote=ejs_remote))
        if player_client:
            opts["extractor_args"] = {"youtube": {"player_client": [player_client]}}
        if cookies_from_browser:
            opts["cookiesfrombrowser"] = (cookies_from_browser,)
        if limit_rate:
            bps = parse_rate(limit_rate)
            if bps > 0:
                opts["ratelimit"] = int(bps)
        return opts

    @classmethod
    def _new_youtube_dl(cls, opts: dict[str, Any]) -> Any:
        """Construct a quiet ``yt_dlp.YoutubeDL`` whose logger writes to stderr."""
        import yt_dlp

        return yt_dlp.YoutubeDL(opts, logger=_YtdlpLogger())

    @staticmethod
    def _ensure_dependencies(target: str) -> None:
        """Fail closed when yt-dlp (or a required JS runtime) is missing."""
        try:
            import yt_dlp  # noqa: F401
        except Exception as exc:  # pragma: no cover - depends on environment
            raise WorkerError(
                "dependency", f"yt-dlp is required for media operations: {exc}"
            ) from exc

        if _is_youtube(target) and TOOLCHAIN.yt_dlp_ejs_version and not TOOLCHAIN.select_js_runtime():
            raise WorkerError(
                "dependency",
                "a JavaScript runtime (deno, node >= 22, or quickjs) is required for "
                "yt-dlp EJS extraction",
            )

    # --- metadata extraction -------------------------------------------------

    @classmethod
    def extract_metadata(
        cls,
        url: str,
        *,
        is_playlist: bool = False,
        ejs_remote: bool = False,
        player_client: str | None = None,
        cookies_from_browser: str | None = None,
    ) -> dict[str, Any] | None:
        """Extract media metadata in a single in-process yt_dlp call."""
        try:
            opts = cls._build_base_ydl_opts(
                ejs_remote=ejs_remote,
                player_client=player_client,
                cookies_from_browser=cookies_from_browser,
            )
            opts["noplaylist"] = not is_playlist
            opts["extract_flat"] = "in_playlist" if is_playlist else False
            with cls._new_youtube_dl(opts) as ydl:
                info = ydl.extract_info(url, download=False)
                if not info:
                    return None
                if "entries" in info and not is_playlist:
                    entries = [e for e in info["entries"] if e]
                    if entries:
                        info = entries[0]
                return dict(info)
        except WorkerError:
            raise
        except Exception as exc:
            log(f"yt_dlp extract_metadata failed for {url}: {exc}")
            return None

    @staticmethod
    def _parse_metadata_fields(
        info: dict[str, Any],
    ) -> tuple[str, str, str | None, str | None, str | None]:
        """Return ``(title, artist, album, year, thumbnail_url)`` from yt_dlp info."""
        raw_title = str(info.get("track") or info.get("title") or "Unknown_Track")
        artist = str(
            info.get("artist") or info.get("creator") or info.get("uploader") or "Unknown Artist"
        )
        album = info.get("album")
        album_str = str(album) if album else None

        year_str: str | None = None
        rel_year = info.get("release_year")
        upload_date = info.get("release_date") or info.get("upload_date")
        if rel_year:
            year_str = str(rel_year)
        elif isinstance(upload_date, str) and len(upload_date) >= 4 and upload_date[:4].isdigit():
            year_str = upload_date[:4]

        title = raw_title
        if artist == "Unknown Artist" and " - " in raw_title:
            parts = raw_title.split(" - ", 1)
            artist, title = parts[0].strip(), parts[1].strip()

        for junk in (
            "(Official Video)",
            "(Official Audio)",
            "[Official Video]",
            "(Lyrics)",
            "(Official Music Video)",
            "[4K]",
            "(HD)",
        ):
            title = title.replace(junk, "").strip()

        thumb_url = info.get("thumbnail")
        return title, artist, album_str, year_str, (str(thumb_url) if thumb_url else None)

    @staticmethod
    def _is_direct_handoff(info: dict[str, Any], is_playlist: bool) -> bool:
        """True when yt-dlp resolved a single direct HTTPS progressive stream.

        Rust performs the actual aria2/DIRECT download; the worker only reports
        the handoff decision and the direct URL/headers.
        """
        if is_playlist:
            return False
        if info.get("_type") in ("playlist", "multi_video"):
            return False
        if info.get("requested_formats") or info.get("fragments"):
            return False
        direct_url = info.get("url")
        if not isinstance(direct_url, str) or not direct_url.startswith(("http://", "https://")):
            return False
        protocol = str(info.get("protocol") or "")
        return not any(p in protocol for p in ("m3u8", "dash", "f4m", "ism", "http_dash_segments"))

    # --- thumbnails / tagging ------------------------------------------------

    @staticmethod
    def _http_get(url: str, timeout: float = 5.0) -> tuple[bytes, str] | None:
        """Best-effort thumbnail GET using only the stdlib (no httpx dependency)."""
        if not url.startswith(("http://", "https://")):
            return None
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "paradl-media/1"})
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                data = bytes(resp.read())
                ctype = resp.headers.get("content-type", "").lower()
                return data, ctype
        except Exception as exc:
            logger.debug("Thumbnail download failed for %s: %s", url, exc)
            return None

    @classmethod
    def download_thumbnail_bytes(cls, thumb_url: str | None) -> tuple[bytes, str] | None:
        """Download thumbnail bytes and detect MIME type ('image/jpeg' or 'image/png')."""
        result = cls._http_get(thumb_url)
        if not result:
            return None
        data, ctype = result
        if not data:
            return None
        if data.startswith(b"\x89PNG") or "png" in ctype:
            return data, "image/png"
        if data.startswith(b"\xff\xd8\xff") or "jpeg" in ctype or "jpg" in ctype:
            return data, "image/jpeg"
        if data[:4] == b"RIFF" and data[8:12] == b"WEBP":
            converted = cls._convert_webp_to_jpeg(data)
            if converted:
                return converted, "image/jpeg"
        return data, "image/jpeg"

    @staticmethod
    def _convert_webp_to_jpeg(webp_bytes: bytes) -> bytes | None:
        if not TOOLCHAIN.ffmpeg:
            return None
        try:
            proc = subprocess.run(
                [TOOLCHAIN.ffmpeg, "-y", "-i", "pipe:0", "-f", "image2", "-vcodec", "mjpeg", "pipe:1"],
                input=webp_bytes,
                capture_output=True,
                timeout=5.0,
                check=False,
            )
            if proc.returncode == 0 and proc.stdout.startswith(b"\xff\xd8\xff"):
                return proc.stdout
        except (OSError, subprocess.SubprocessError) as exc:
            logger.debug("WebP to JPEG conversion failed: %s", exc)
        return None

    @staticmethod
    def cleanup_sidecar_thumbnails(directory: Path, stem: str) -> int:
        """Remove leftover thumbnail sidecar files matching ``stem``."""
        removed = 0
        for ext in (".webp", ".jpg", ".jpeg", ".png"):
            cand = directory / f"{stem}{ext}"
            if cand.exists() and cand.is_file():
                try:
                    cand.unlink()
                    removed += 1
                except OSError as exc:
                    logger.debug("Failed to delete thumbnail %s: %s", cand, exc)
        return removed

    @classmethod
    def embed_metadata_and_cover(
        cls,
        audio_file: Path,
        *,
        title: str,
        artist: str,
        album: str | None = None,
        year: str | None = None,
        thumbnail_url: str | None = None,
        thumbnail_file: Path | None = None,
        embed_thumbnail: bool = True,
    ) -> None:
        """Embed title/artist/album/year and cover art into ``audio_file``."""
        thumb_payload: tuple[bytes, str] | None = None
        if embed_thumbnail:
            if thumbnail_file and thumbnail_file.exists():
                try:
                    raw_bytes = thumbnail_file.read_bytes()
                    mime = "image/png" if raw_bytes.startswith(b"\x89PNG") else "image/jpeg"
                    if raw_bytes[:4] == b"RIFF" and raw_bytes[8:12] == b"WEBP":
                        conv = cls._convert_webp_to_jpeg(raw_bytes)
                        if conv:
                            raw_bytes = conv
                            mime = "image/jpeg"
                    thumb_payload = (raw_bytes, mime)
                except OSError as exc:
                    logger.debug("Could not read thumbnail file %s: %s", thumbnail_file, exc)
                finally:
                    with contextlib.suppress(OSError):
                        thumbnail_file.unlink()
            elif thumbnail_url:
                thumb_payload = cls.download_thumbnail_bytes(thumbnail_url)

        cls.cleanup_sidecar_thumbnails(audio_file.parent, audio_file.stem)

        suffix = audio_file.suffix.lower()
        try:
            cls._write_tags(audio_file, suffix, title, artist, album, year, thumb_payload)
        except Exception as exc:
            logger.debug("Metadata/cover embedding warning for %s: %s", audio_file, exc)

    @staticmethod
    def _write_tags(
        audio_file: Path,
        suffix: str,
        title: str,
        artist: str,
        album: str | None,
        year: str | None,
        thumb_payload: tuple[bytes, str] | None,
    ) -> None:
        if suffix in (".m4a", ".mp4", ".aac"):
            from mutagen.mp4 import MP4, MP4Cover

            mp4_tags = MP4(str(audio_file))
            if mp4_tags.tags is None:
                mp4_tags.add_tags()
            assert mp4_tags.tags is not None
            mp4_tags.tags["\xa9nam"] = [title]
            mp4_tags.tags["\xa9ART"] = [artist]
            if album:
                mp4_tags.tags["\xa9alb"] = [album]
            if year:
                mp4_tags.tags["\xa9day"] = [str(year)]
            if thumb_payload:
                img_bytes, img_mime = thumb_payload
                img_fmt = MP4Cover.FORMAT_PNG if img_mime == "image/png" else MP4Cover.FORMAT_JPEG
                mp4_tags.tags["covr"] = [MP4Cover(img_bytes, imageformat=img_fmt)]
            mp4_tags.save()
        elif suffix == ".mp3":
            from mutagen.id3 import APIC, ID3, TALB, TDRC, TIT2, TPE1, ID3NoHeaderError

            try:
                id3 = ID3(str(audio_file))
            except ID3NoHeaderError:
                id3 = ID3()
            id3.add(TIT2(encoding=3, text=[title]))
            id3.add(TPE1(encoding=3, text=[artist]))
            if album:
                id3.add(TALB(encoding=3, text=[album]))
            if year:
                id3.add(TDRC(encoding=3, text=[str(year)]))
            if thumb_payload:
                img_bytes, img_mime = thumb_payload
                id3.add(
                    APIC(encoding=3, mime=img_mime, type=3, desc="Cover", data=img_bytes)
                )
            id3.save(str(audio_file), v2_version=3)
        elif suffix == ".flac":
            from mutagen.flac import FLAC, Picture

            flac = FLAC(str(audio_file))
            flac["title"] = title
            flac["artist"] = artist
            if album:
                flac["album"] = album
            if year:
                flac["date"] = str(year)
            if thumb_payload:
                img_bytes, img_mime = thumb_payload
                pic = Picture()
                pic.data = img_bytes
                pic.type = 3
                pic.mime = img_mime
                pic.desc = "Cover"
                flac.clear_pictures()
                flac.add_picture(pic)
            flac.save()

    # --- conversion ----------------------------------------------------------

    @classmethod
    def _convert_coreaudio_or_ffmpeg(
        cls,
        raw_file: Path,
        out_dir: Path,
        safe_stem: str,
        requested_codec: str,
        source_acodec: str | None,
    ) -> tuple[Path | None, str]:
        """Convert a raw file to the final format; return ``(path, honest_desc)``.

        Every returned description is an exact ``classify_audio_production``
        string (the wire contract). Missing ffmpeg/afconvert is weak: the
        ``shutil.move`` fallback still succeeds.
        """
        # Native mode: keep original stream container/codec without re-encoding
        if requested_codec == "native":
            ext = raw_file.suffix.lower() or ".m4a"
            final_path = out_dir / f"{safe_stem}{ext}"
            if raw_file != final_path:
                shutil.move(str(raw_file), str(final_path))
            src = (source_acodec or ext.lstrip(".")).lower().split(".")[0]
            return final_path, classify_audio_production("native", src, "passthrough_aac")

        # macOS CoreAudio packaging (256 kbps AAC max)
        if requested_codec in ("aac", "lossless") and is_macos() and TOOLCHAIN.afconvert:
            final_m4a = out_dir / f"{safe_stem}.m4a"
            log("Packaging AAC via Apple CoreAudio (256 kbps max)")
            res_aac = subprocess.run(
                [
                    TOOLCHAIN.afconvert,
                    "-f",
                    "m4af",
                    "-d",
                    "aac",
                    "-b",
                    "256000",
                    str(raw_file),
                    str(final_m4a),
                ],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )
            if res_aac.returncode == 0 and final_m4a.exists() and final_m4a.stat().st_size > 0:
                return final_m4a, classify_audio_production("aac", source_acodec, "passthrough_aac")

        # FFmpeg conversion fallback (or explicit mp3 / flac / opus / wav / aac)
        target_ext = "m4a" if requested_codec in ("lossless", "aac") else requested_codec
        final_file = out_dir / f"{safe_stem}.{target_ext}"
        if TOOLCHAIN.ffmpeg:
            c_args = CODEC_ARGS_MAP.get(requested_codec, ["-vn", "-c:a", "aac", "-b:a", "256k"])
            proc = subprocess.run(
                [TOOLCHAIN.ffmpeg, "-y", "-i", str(raw_file), *c_args, str(final_file)],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )
            if proc.returncode == 0 and final_file.exists() and final_file.stat().st_size > 0:
                mode_tag = "aac" if requested_codec in ("lossless", "aac") else requested_codec
                return final_file, classify_audio_production(
                    requested_codec, source_acodec, mode_tag
                )
            if final_file.exists() and final_file.stat().st_size == 0:
                with contextlib.suppress(OSError):
                    final_file.unlink()

        # Last-resort move if neither afconvert nor ffmpeg succeeded
        fallback_path = out_dir / f"{safe_stem}{raw_file.suffix or '.m4a'}"
        shutil.move(str(raw_file), str(fallback_path))
        return fallback_path, classify_audio_production("native", source_acodec, "passthrough_aac")

    # --- shared metadata result ---------------------------------------------

    @staticmethod
    def _expected_audio_format(requested_codec: str, source_acodec: str | None) -> str:
        rc = (requested_codec or "aac").lower()
        mode = "passthrough_aac" if rc in ("aac", "native", "lossless", "alac") else "native"
        return classify_audio_production(rc, source_acodec, mode)

    @staticmethod
    def _safe_stem(info: dict[str, Any], title: str, artist: str) -> str:
        return sanitize_filename(f"{artist} - {title}" if artist != "Unknown Artist" else title)

    @staticmethod
    def _source_acodec(info: dict[str, Any]) -> str:
        return str(info.get("acodec") or "aac")

    # --- ops -----------------------------------------------------------------

    def resolve(self, target: str, options: dict[str, Any]) -> dict[str, Any]:
        """Extract metadata and return the direct-vs-managed handoff decision."""
        self._ensure_dependencies(target)
        is_playlist = bool(options.get("playlist", False))
        cookies = options.get("cookies_from_browser")
        ejs_remote = bool(options.get("ejs_remote", False))
        player_client = options.get("player_client")

        info = self.extract_metadata(
            target,
            is_playlist=is_playlist,
            ejs_remote=ejs_remote,
            player_client=player_client,
            cookies_from_browser=cookies,
        )
        if info is None:
            raise WorkerError("metadata", f"metadata resolution failed for {target}")

        # A playlist entry list with no usable entries is a metadata failure.
        if "entries" in info and not is_playlist:
            entries = [e for e in info["entries"] if e]
            if not entries:
                raise WorkerError("metadata", "no search results found")
            info = dict(entries[0])

        title, artist, album, year, thumb_url = self._parse_metadata_fields(info)
        source_acodec = self._source_acodec(info)
        codec = _codec_from_options(options)
        direct = self._is_direct_handoff(info, is_playlist)
        headers = _coerce_headers(info.get("http_headers"))
        expected_len = info.get("filesize") or info.get("filesize_approx")

        return {
            "filename": self._safe_stem(info, title, artist),
            "http_headers": headers,
            "audio_format_produced": self._expected_audio_format(codec, source_acodec),
            "paths": [],
            "claims": ytdlp_extractor_claims_url(target),
            "managed": not direct,
            "direct_url": str(info["url"]) if direct else None,
            "ext": str(info.get("ext") or "m4a"),
            "title": title,
            "artist": artist,
            "album": album,
            "year": year,
            "thumbnail_url": thumb_url,
            "source_acodec": source_acodec,
            "duration": info.get("duration"),
            "filesize": int(expected_len) if isinstance(expected_len, int) else None,
            "is_playlist": is_playlist,
            "webpage_url": info.get("webpage_url") or target,
        }

    def download_managed(
        self,
        target: str,
        out_dir: Path,
        options: dict[str, Any],
        emit_progress: ProgressCallback,
    ) -> dict[str, Any]:
        """Download via yt-dlp, convert, tag, and return the produced paths."""
        self._ensure_dependencies(target)
        out_dir.mkdir(parents=True, exist_ok=True)
        is_playlist = bool(options.get("playlist", False))
        cookies = options.get("cookies_from_browser")
        codec = _codec_from_options(options)
        embed_thumbnail = bool(options.get("embed_thumbnail", True))
        ejs_remote = bool(options.get("ejs_remote", False))
        player_client = options.get("player_client")
        limit_rate = options.get("limit_rate")

        ydl_opts = self._build_base_ydl_opts(
            ejs_remote=ejs_remote,
            player_client=player_client,
            cookies_from_browser=cookies,
            limit_rate=limit_rate,
        )
        ydl_opts["format"] = options.get("format") or AUDIO_FORMAT_SELECTION
        ydl_opts["noplaylist"] = not is_playlist

        try:
            with self._new_youtube_dl(ydl_opts) as ydl:
                info = ydl.extract_info(target, download=False)
        except WorkerError:
            raise
        except Exception as exc:
            raise WorkerError("metadata", f"audio metadata resolution failed: {exc}") from exc

        if not info:
            raise WorkerError("metadata", "no media info returned")
        if "entries" in info and not is_playlist:
            entries = [e for e in info["entries"] if e]
            if not entries:
                raise WorkerError("metadata", "no search results found")
            info = dict(entries[0])

        info_dict = dict(info)
        title, artist, album, year, thumb_url = self._parse_metadata_fields(info_dict)
        source_acodec = self._source_acodec(info_dict)
        safe_stem = self._safe_stem(info_dict, title, artist)
        headers = _coerce_headers(info_dict.get("http_headers"))

        with tempfile.TemporaryDirectory(prefix="pdl_ytdlp_audio_", dir=str(out_dir)) as tmp:
            tmp_path = Path(tmp)
            dl_opts = dict(ydl_opts)
            dl_opts["outtmpl"] = str(tmp_path / f"{safe_stem}.%(ext)s")
            dl_opts["concurrent_fragment_downloads"] = 16

            def hook(d: dict[str, Any]) -> None:
                if d.get("status") == "downloading":
                    emit_progress(
                        int(d.get("downloaded_bytes") or 0),
                        int(d.get("total_bytes") or d.get("total_bytes_estimate") or 0) or None,
                        float(d.get("speed") or 0.0),
                        None,
                    )

            dl_opts["progress_hooks"] = [hook]
            try:
                with self._new_youtube_dl(dl_opts) as ydl:
                    ydl.download([info_dict.get("webpage_url") or target])
            except WorkerError:
                raise
            except Exception as exc:
                raise WorkerError("download", f"audio download failed: {exc}") from exc

            candidates = [p for p in tmp_path.glob("*") if p.is_file() and not p.name.endswith(".aria2")]
            if not candidates:
                raise WorkerError("download", "yt-dlp finished without producing an audio file")
            raw_audio = candidates[0]

            final_audio, honest_desc = self._convert_coreaudio_or_ffmpeg(
                raw_audio, out_dir, safe_stem, codec, source_acodec
            )
            if not final_audio or not final_audio.exists():
                raise WorkerError("conversion", "audio conversion failed")
            self.embed_metadata_and_cover(
                final_audio,
                title=title,
                artist=artist,
                album=album,
                year=year,
                thumbnail_url=thumb_url,
                embed_thumbnail=embed_thumbnail,
            )

        return {
            "filename": safe_stem,
            "http_headers": headers,
            "audio_format_produced": honest_desc,
            "paths": [str(final_audio)],
            "claims": ytdlp_extractor_claims_url(target),
            "bytes_downloaded": final_audio.stat().st_size,
            "title": title,
            "artist": artist,
            "album": album,
            "year": year,
            "source_acodec": source_acodec,
        }

    def finalize(
        self,
        target: str,
        out_dir: Path,
        options: dict[str, Any],
    ) -> dict[str, Any]:
        """Convert and tag an already-downloaded local file (checksums are Rust-owned)."""
        source = Path(target)
        if not source.exists() or not source.is_file():
            raise WorkerError("usage", f"finalize target is not a file: {target}")
        out_dir.mkdir(parents=True, exist_ok=True)

        codec = _codec_from_options(options)
        stem = sanitize_filename(options.get("filename") or source.stem)
        source_acodec = options.get("source_acodec")
        title = str(options.get("title") or stem)
        artist = str(options.get("artist") or "Unknown Artist")
        album = options.get("album")
        year = options.get("year")
        thumbnail_url = options.get("thumbnail_url")
        embed_thumbnail = bool(options.get("embed_thumbnail", True))

        work_dir = out_dir
        final_audio, honest_desc = self._convert_coreaudio_or_ffmpeg(
            source, work_dir, stem, codec, source_acodec
        )
        if not final_audio or not final_audio.exists():
            raise WorkerError("conversion", "audio conversion failed")
        self.embed_metadata_and_cover(
            final_audio,
            title=title,
            artist=artist,
            album=album,
            year=year,
            thumbnail_url=thumbnail_url,
            embed_thumbnail=embed_thumbnail,
        )
        return {
            "filename": stem,
            "http_headers": {},
            "audio_format_produced": honest_desc,
            "paths": [str(final_audio)],
            "bytes_downloaded": final_audio.stat().st_size,
        }
