"""Media extraction, progressive Aria2 watchdog handoff, CoreAudio conversion, and Mutagen cover/tag embedding."""

from __future__ import annotations

import logging
import os
import shutil
import subprocess
import tempfile
import time
from pathlib import Path
from typing import Any

from pdl.engines.aria2 import (
    Aria2Engine,
    register_temp_path,
    unregister_temp_path,
)
from pdl.models import (
    ExitCode,
    TargetInfo,
    TargetType,
    TransferResult,
    WatchdogMode,
    parse_rate,
    sanitize_filename,
)
from pdl.net import request_with_retry
from pdl.toolchain import TOOLCHAIN, is_macos
from pdl.ui import LiveTransferDisplay, print_plain

logger = logging.getLogger(__name__)

LOSSLESS_SOURCE_CODECS = {"flac", "alac", "wav", "pcm", "pcm_s16le", "pcm_s24le", "pcm_f32le", "ape", "wv"}


def classify_audio_production(
    requested_codec: str,
    source_acodec: str | None,
    conversion_mode: str,
) -> str:
    """Return an honest human-readable description of the audio format actually produced.

    Phase 0 #8: Distinguish passthrough AAC, ALAC-wrapped lossy, and true lossless from a lossless source.
    """
    src = (source_acodec or "unknown").lower().split(".")[0]
    is_src_lossless = src in LOSSLESS_SOURCE_CODECS

    if conversion_mode == "passthrough_aac":
        return f"passthrough AAC (from {src} source, no re-encode)"
    if conversion_mode == "alac":
        if is_src_lossless:
            return f"true lossless ALAC (from lossless {src} source)"
        return f"ALAC-wrapped lossy (transcoded from lossy {src} source)"
    if requested_codec == "flac":
        if is_src_lossless:
            return f"true lossless FLAC (from {src} source)"
        return f"FLAC (transcoded from lossy {src} source)"
    if requested_codec == "wav":
        return f"uncompressed WAV PCM (from {src} source)"
    return f"{requested_codec.upper()} (from {src} source)"


def download_thumbnail_bytes(thumb_url: str | None) -> tuple[bytes, str] | None:
    """Download thumbnail bytes and detect MIME type ('image/jpeg' or 'image/png')."""
    if not thumb_url or not thumb_url.startswith(("http://", "https://")):
        return None
    try:
        resp = request_with_retry("GET", thumb_url, retries=2, timeout=5.0)
        if resp.status_code == 200 and resp.content:
            data = bytes(resp.content)
            ctype = resp.headers.get("content-type", "").lower()
            if data.startswith(b"\x89PNG") or "png" in ctype:
                return data, "image/png"
            if data.startswith(b"\xff\xd8\xff") or "jpeg" in ctype or "jpg" in ctype:
                return data, "image/jpeg"
            # If WebP and ffmpeg or sips is available on macOS, convert to JPEG so MP4 covr / ID3 APIC readers support it
            if data[:4] == b"RIFF" and data[8:12] == b"WEBP":
                converted = _convert_webp_to_jpeg(data)
                if converted:
                    return converted, "image/jpeg"
            return data, "image/jpeg"
    except Exception as exc:
        logger.debug("Thumbnail download failed for %s: %s", thumb_url, exc)
    return None


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


def cleanup_sidecar_thumbnails(directory: Path, stem: str) -> int:
    """Remove any leftover thumbnail sidecar files (.webp, .jpg, .jpeg, .png) matching stem."""
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


def embed_metadata_and_cover(
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
    """Embed title, artist, album, year, and cover art (MP4 covr / ID3 APIC) into audio_file.
    Deletes any downloaded thumbnail file afterwards (Phase 0 #9).
    """
    thumb_payload: tuple[bytes, str] | None = None
    if embed_thumbnail:
        if thumbnail_file and thumbnail_file.exists():
            try:
                raw_bytes = thumbnail_file.read_bytes()
                mime = "image/png" if raw_bytes.startswith(b"\x89PNG") else "image/jpeg"
                if raw_bytes[:4] == b"RIFF" and raw_bytes[8:12] == b"WEBP":
                    conv = _convert_webp_to_jpeg(raw_bytes)
                    if conv:
                        raw_bytes = conv
                        mime = "image/jpeg"
                thumb_payload = (raw_bytes, mime)
            except OSError as exc:
                logger.debug("Could not read thumbnail file %s: %s", thumbnail_file, exc)
            finally:
                try:
                    thumbnail_file.unlink()
                except OSError:
                    pass
        elif thumbnail_url:
            thumb_payload = download_thumbnail_bytes(thumbnail_url)

    # Always clean up any sidecar thumbnails left by yt-dlp
    cleanup_sidecar_thumbnails(audio_file.parent, audio_file.stem)

    suffix = audio_file.suffix.lower()
    try:
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
                    APIC(
                        encoding=3,
                        mime=img_mime,
                        type=3,
                        desc="Cover",
                        data=img_bytes,
                    )
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
    except Exception as exc:
        logger.debug("Metadata/cover embedding warning for %s: %s", audio_file, exc)


class MediaEngine:
    """Downloads and processes streaming audio/video with single-pass yt_dlp API and Aria2 Watchdog handoff."""

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
            import yt_dlp

            opts = cls._build_base_ydl_opts(
                ejs_remote=ejs_remote,
                player_client=player_client,
                cookies_from_browser=cookies_from_browser,
            )
            opts["noplaylist"] = not is_playlist
            opts["extract_flat"] = "in_playlist" if is_playlist else False
            with yt_dlp.YoutubeDL(opts) as ydl:
                info = ydl.extract_info(url, download=False)
                if not info:
                    return None
                if "entries" in info and not is_playlist:
                    entries = [e for e in info["entries"] if e]
                    if entries:
                        info = entries[0]
                return dict(info)
        except Exception as exc:
            logger.debug("yt_dlp extract_metadata failed for %s: %s", url, exc)
            return None

    @staticmethod
    def _parse_metadata_fields(info: dict[str, Any]) -> tuple[str, str, str | None, str | None, str | None]:
        """Return (title, artist, album, year, thumbnail_url) from yt_dlp info dict."""
        raw_title = str(info.get("track") or info.get("title") or "Unknown_Track")
        artist = str(info.get("artist") or info.get("creator") or info.get("uploader") or "Unknown Artist")
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
    def _can_handoff_to_aria2(info: dict[str, Any], is_playlist: bool, sponsorblock: bool) -> bool:
        """Phase 3 #6: Return True when yt_dlp resolved a single direct HTTPS progressive stream
        without DASH/HLS fragments so Aria2Engine can supervise it with Watchdog v2.
        """
        if is_playlist or sponsorblock or not TOOLCHAIN.aria2c:
            return False
        if info.get("_type") in ("playlist", "multi_video"):
            return False
        if info.get("requested_formats") or info.get("fragments"):
            return False
        direct_url = info.get("url")
        if not isinstance(direct_url, str) or not direct_url.startswith(("http://", "https://")):
            return False
        protocol = str(info.get("protocol") or "")
        if any(p in protocol for p in ("m3u8", "dash", "f4m", "ism", "http_dash_segments")):
            return False
        return True

    @classmethod
    def _convert_coreaudio_or_ffmpeg(
        cls,
        raw_file: Path,
        out_dir: Path,
        safe_stem: str,
        requested_codec: str,
        source_acodec: str | None,
    ) -> tuple[Path | None, str]:
        """Convert raw downloaded audio file into final format and return (final_path, honest_format_desc)."""
        # Native mode: keep original stream container/codec without re-encoding
        if requested_codec == "native":
            ext = raw_file.suffix.lower() or ".m4a"
            final_path = out_dir / f"{safe_stem}{ext}"
            if raw_file != final_path:
                shutil.move(str(raw_file), str(final_path))
            src = (source_acodec or ext.lstrip(".")).lower().split(".")[0]
            return final_path, f"native stream ({src}, no re-encode)"

        # macOS CoreAudio packaging (256 kbps AAC max)
        if requested_codec in ("aac", "lossless") and is_macos() and TOOLCHAIN.afconvert:
            final_m4a = out_dir / f"{safe_stem}.m4a"
            print_plain("  [dim]Packaging AAC via Apple CoreAudio (256 kbps max)...[/dim]")
            res_aac = subprocess.run(
                [TOOLCHAIN.afconvert, "-f", "m4af", "-d", "aac", "-b", "256000", str(raw_file), str(final_m4a)],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )
            if res_aac.returncode == 0 and final_m4a.exists() and final_m4a.stat().st_size > 0:
                return final_m4a, "AAC 256 kbps (Apple CoreAudio, max 256k)"

        # FFmpeg conversion fallback (or explicit mp3 / flac / opus / wav / aac)
        target_ext = "m4a" if requested_codec in ("lossless", "aac") else requested_codec
        final_file = out_dir / f"{safe_stem}.{target_ext}"
        if TOOLCHAIN.ffmpeg:
            codec_args_map: dict[str, list[str]] = {
                "mp3": ["-vn", "-c:a", "libmp3lame", "-b:a", "256k"],
                "flac": ["-vn", "-c:a", "flac"],
                "opus": ["-vn", "-c:a", "libopus", "-b:a", "192k"],
                "wav": ["-vn", "-c:a", "pcm_s16le"],
                "aac": ["-vn", "-c:a", "aac", "-b:a", "256k", "-movflags", "+faststart"],
                "lossless": ["-vn", "-c:a", "aac", "-b:a", "256k", "-movflags", "+faststart"],
            }
            c_args = codec_args_map.get(requested_codec, ["-vn", "-c:a", "aac", "-b:a", "256k"])
            proc = subprocess.run(
                [TOOLCHAIN.ffmpeg, "-y", "-i", str(raw_file), *c_args, str(final_file)],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                check=False,
            )
            if proc.returncode == 0 and final_file.exists() and final_file.stat().st_size > 0:
                mode_tag = "aac" if requested_codec in ("lossless", "aac") else requested_codec
                return final_file, classify_audio_production(requested_codec, source_acodec, mode_tag)

            if final_file.exists() and final_file.stat().st_size == 0:
                try:
                    final_file.unlink()
                except OSError:
                    pass

        # Last-resort move if neither afconvert nor ffmpeg succeeded
        fallback_path = out_dir / f"{safe_stem}{raw_file.suffix or '.m4a'}"
        shutil.move(str(raw_file), str(fallback_path))
        return fallback_path, classify_audio_production("native", source_acodec, "passthrough_aac")

    @classmethod
    def download_audio(
        cls,
        url: str,
        out_dir: Path,
        connections: int = 16,
        codec: str = "aac",
        is_playlist: bool = False,
        cookies: str | None = None,
        sponsorblock: bool = False,
        embed_thumbnail: bool = True,
        limit_rate: str | None = None,
        anti_throttle: str | None = "4MBps",
        grace_period: int = 5,
        watchdog_mode: WatchdogMode | str = WatchdogMode.V2,
        ejs_remote: bool = False,
        player_client: str | None = None,
    ) -> TransferResult:
        out_dir.mkdir(parents=True, exist_ok=True)
        safe_conn = min(max(int(connections), 1), 16)
        requested_codec = codec.lower()
        t_start = time.time()

        print_plain("  [dim]Fetching media information...[/dim]")

        import yt_dlp

        ydl_opts = cls._build_base_ydl_opts(
            ejs_remote=ejs_remote,
            player_client=player_client,
            cookies_from_browser=cookies,
            limit_rate=limit_rate,
        )
        ydl_opts["format"] = "ba[ext=m4a][abr<=256]/ba[ext=aac][abr<=256]/ba[abr<=256]/bestaudio/best"
        ydl_opts["noplaylist"] = not is_playlist

        try:
            with yt_dlp.YoutubeDL(ydl_opts) as ydl:
                info = ydl.extract_info(url, download=False)
        except Exception as exc:
            msg = f"Audio metadata resolution failed: {exc}"
            print_plain(f"  [error]✗ {msg}[/error]", force=True)
            return TransferResult(success=False, error=msg, exit_code=int(ExitCode.FAILURE))

        if not info:
            return TransferResult(success=False, error="No media info returned", exit_code=int(ExitCode.FAILURE))

        if "entries" in info and not is_playlist:
            entries = [e for e in info["entries"] if e]
            if not entries:
                return TransferResult(success=False, error="No search results found", exit_code=int(ExitCode.FAILURE))
            info = entries[0]

        info_dict = dict(info)
        title, artist, album, year, thumb_url = cls._parse_metadata_fields(info_dict)
        safe_stem = sanitize_filename(f"{artist} - {title}" if artist != "Unknown Artist" else title)
        source_acodec = str(info_dict.get("acodec") or "aac")

        track_desc = f"{title}" if artist == "Unknown Artist" else f"{title} • {artist}"
        print_plain(f"  [bold white]Track:[/bold white]        [bold white]{track_desc}[/bold white]")
        if album:
            print_plain(f"  [dim]Album:[/dim]        [dim]{album}[/dim]")

        # Phase 3 #6: Handoff single-stream progressive URL to Aria2Engine for Watchdog v2 supervision!
        if cls._can_handoff_to_aria2(info_dict, is_playlist=is_playlist, sponsorblock=sponsorblock):
            direct_url = str(info_dict["url"])
            ext = str(info_dict.get("ext") or "m4a")
            stream_headers = {str(k): str(v) for k, v in (info_dict.get("http_headers") or {}).items()}
            expected_len = info_dict.get("filesize") or info_dict.get("filesize_approx")

            with tempfile.TemporaryDirectory(prefix="pdl_audio_raw_", dir=str(out_dir)) as tmp_raw_dir:
                raw_dir_path = Path(tmp_raw_dir)
                register_temp_path(raw_dir_path)
                try:
                    raw_filename = f"raw_stream.{ext}"
                    t_info = TargetInfo(
                        raw=direct_url,
                        target_type=TargetType.DIRECT,
                        clean_url=direct_url,
                        filename_hint=raw_filename,
                        content_length=int(expected_len) if isinstance(expected_len, int) else None,
                        headers=stream_headers,
                    )
                    aria_res = Aria2Engine.download(
                        t_info,
                        raw_dir_path,
                        connections=safe_conn,
                        limit_rate=limit_rate,
                        anti_throttle=anti_throttle,
                        grace_period=grace_period,
                        watchdog_mode=watchdog_mode,
                    )
                    if aria_res.success and aria_res.file_path and aria_res.file_path.exists():
                        final_audio, honest_desc = cls._convert_coreaudio_or_ffmpeg(
                            aria_res.file_path,
                            out_dir,
                            safe_stem,
                            requested_codec,
                            source_acodec,
                        )
                        if final_audio and final_audio.exists():
                            embed_metadata_and_cover(
                                final_audio,
                                title=title,
                                artist=artist,
                                album=album,
                                year=year,
                                thumbnail_url=thumb_url,
                                embed_thumbnail=embed_thumbnail,
                            )
                            dur = max(time.time() - t_start, 0.001)
                            fsize = final_audio.stat().st_size
                            print_plain(f"  [dim]Audio:[/dim]       [green]{honest_desc}[/green]")
                            return TransferResult(
                                success=True,
                                file_path=final_audio,
                                files=[final_audio],
                                bytes_downloaded=fsize,
                                duration=dur,
                                avg_speed_bps=aria_res.avg_speed_bps or (fsize / dur),
                                p10_speed_bps=aria_res.p10_speed_bps,
                                recycle_count=aria_res.recycle_count,
                                audio_format_produced=honest_desc,
                            )
                finally:
                    unregister_temp_path(raw_dir_path)

        # Fallback: yt_dlp native/fragmented downloader with LiveTransferDisplay hook
        with tempfile.TemporaryDirectory(prefix="pdl_ytdlp_audio_", dir=str(out_dir)) as tmp_dl_dir:
            tmp_path = Path(tmp_dl_dir)
            register_temp_path(tmp_path)
            try:
                dl_opts = dict(ydl_opts)
                dl_opts["outtmpl"] = str(tmp_path / f"{safe_stem}.%(ext)s")
                if TOOLCHAIN.aria2c:
                    dl_opts["external_downloader"] = {
                        "default": TOOLCHAIN.aria2c,
                        "dash": "native",
                        "m3u8": "native",
                    }
                    dl_opts["external_downloader_args"] = {
                        "aria2c": [
                            f"-x{safe_conn}",
                            f"-s{safe_conn}",
                            "-k1M",
                            "--file-allocation=none",
                        ]
                    }
                dl_opts["concurrent_fragment_downloads"] = safe_conn
                if sponsorblock:
                    dl_opts.setdefault("postprocessors", []).append(
                        {"key": "SponsorBlock", "categories": ["sponsor", "intro", "outro", "selfpromo"]}
                    )
                    dl_opts["postprocessors"].append(
                        {"key": "ModifyChapters", "remove_sponsor_segments": {"sponsor", "intro", "outro", "selfpromo"}}
                    )

                with LiveTransferDisplay(title=safe_stem) as live:

                    def _hook(d: dict[str, Any]) -> None:
                        if d.get("status") == "downloading":
                            live.update(
                                downloaded_bytes=int(d.get("downloaded_bytes") or 0),
                                total_bytes=int(d.get("total_bytes") or d.get("total_bytes_estimate") or 0),
                                speed_bps=float(d.get("speed") or 0.0),
                                connections=safe_conn,
                                watchdog_state="yt-dlp",
                            )

                    dl_opts["progress_hooks"] = [_hook]
                    with yt_dlp.YoutubeDL(dl_opts) as ydl:
                        ydl.download([info_dict.get("webpage_url") or url])

                candidates = [p for p in tmp_path.glob("*") if p.is_file() and not p.name.endswith(".aria2")]
                if not candidates:
                    return TransferResult(
                        success=False,
                        error="yt-dlp finished without producing an audio file",
                        exit_code=int(ExitCode.FAILURE),
                    )
                raw_audio = candidates[0]
                final_audio, honest_desc = cls._convert_coreaudio_or_ffmpeg(
                    raw_audio,
                    out_dir,
                    safe_stem,
                    requested_codec,
                    source_acodec,
                )
                if not final_audio or not final_audio.exists():
                    return TransferResult(
                        success=False,
                        error="Audio conversion failed",
                        exit_code=int(ExitCode.FAILURE),
                    )
                embed_metadata_and_cover(
                    final_audio,
                    title=title,
                    artist=artist,
                    album=album,
                    year=year,
                    thumbnail_url=thumb_url,
                    embed_thumbnail=embed_thumbnail,
                )
                dur = max(time.time() - t_start, 0.001)
                fsize = final_audio.stat().st_size
                print_plain(f"  [dim]Audio:[/dim]       [green]{honest_desc}[/green]")
                return TransferResult(
                    success=True,
                    file_path=final_audio,
                    files=[final_audio],
                    bytes_downloaded=fsize,
                    duration=dur,
                    avg_speed_bps=fsize / dur,
                    audio_format_produced=honest_desc,
                )
            except Exception as exc:
                msg = f"Audio download failed: {exc}"
                print_plain(f"  [error]✗ {msg}[/error]", force=True)
                return TransferResult(success=False, error=msg, exit_code=int(ExitCode.FAILURE))
            finally:
                unregister_temp_path(tmp_path)

    @classmethod
    def download_video(
        cls,
        url: str,
        out_dir: Path,
        connections: int = 16,
        quality: str = "best",
        is_playlist: bool = False,
        subs: bool = False,
        cookies: str | None = None,
        sponsorblock: bool = False,
        embed_thumbnail: bool = True,
        limit_rate: str | None = None,
        anti_throttle: str | None = "4MBps",
        grace_period: int = 5,
        watchdog_mode: WatchdogMode | str = WatchdogMode.V2,
        ejs_remote: bool = False,
        player_client: str | None = None,
    ) -> TransferResult:
        out_dir.mkdir(parents=True, exist_ok=True)
        safe_conn = min(max(int(connections), 1), 16)
        t_start = time.time()

        quality_map = {
            "best": "bestvideo+bestaudio/best",
            "4k": "bestvideo[height<=2160]+bestaudio/best[height<=2160]",
            "1440p": "bestvideo[height<=1440]+bestaudio/best[height<=1440]",
            "1080p": "bestvideo[height<=1080]+bestaudio/best[height<=1080]",
            "720p": "bestvideo[height<=720]+bestaudio/best[height<=720]",
            "480p": "bestvideo[height<=480]+bestaudio/best[height<=480]",
        }
        fmt_str = quality_map.get(quality.lower(), "bestvideo+bestaudio/best")

        import yt_dlp

        ydl_opts = cls._build_base_ydl_opts(
            ejs_remote=ejs_remote,
            player_client=player_client,
            cookies_from_browser=cookies,
            limit_rate=limit_rate,
        )
        ydl_opts["format"] = fmt_str
        ydl_opts["merge_output_format"] = "mp4"
        ydl_opts["noplaylist"] = not is_playlist

        print_plain(f"  [bold white]Stream Target:[/bold white] [cyan]{url}[/cyan]")
        print_plain(f"  [dim]Quality:[/dim]       [bold cyan]{quality.upper()}[/bold cyan] (MP4)")

        try:
            with yt_dlp.YoutubeDL(ydl_opts) as ydl:
                info = ydl.extract_info(url, download=False)
        except Exception as exc:
            msg = f"Video metadata resolution failed: {exc}"
            print_plain(f"  [error]✗ {msg}[/error]", force=True)
            return TransferResult(success=False, error=msg, exit_code=int(ExitCode.FAILURE))

        if not info:
            return TransferResult(success=False, error="No video metadata returned", exit_code=int(ExitCode.FAILURE))

        if "entries" in info and not is_playlist:
            entries = [e for e in info["entries"] if e]
            if not entries:
                return TransferResult(success=False, error="No video results found", exit_code=int(ExitCode.FAILURE))
            info = entries[0]

        info_dict = dict(info)
        title = sanitize_filename(str(info_dict.get("title") or "video"))

        # Phase 3 #6: Handoff single-stream progressive video URL to Aria2Engine for Watchdog v2 supervision
        if not subs and cls._can_handoff_to_aria2(info_dict, is_playlist=is_playlist, sponsorblock=sponsorblock):
            direct_url = str(info_dict["url"])
            ext = str(info_dict.get("ext") or "mp4")
            stream_headers = {str(k): str(v) for k, v in (info_dict.get("http_headers") or {}).items()}
            expected_len = info_dict.get("filesize") or info_dict.get("filesize_approx")
            out_fname = f"{title}.{ext}"
            t_info = TargetInfo(
                raw=direct_url,
                target_type=TargetType.DIRECT,
                clean_url=direct_url,
                filename_hint=out_fname,
                content_length=int(expected_len) if isinstance(expected_len, int) else None,
                headers=stream_headers,
            )
            aria_res = Aria2Engine.download(
                t_info,
                out_dir,
                connections=safe_conn,
                limit_rate=limit_rate,
                anti_throttle=anti_throttle,
                grace_period=grace_period,
                watchdog_mode=watchdog_mode,
            )
            if aria_res.success:
                return aria_res

        # Multi-stream / DASH / HLS download via yt_dlp
        dl_opts = dict(ydl_opts)
        if is_playlist:
            dl_opts["outtmpl"] = str(out_dir / "%(playlist_title)s/%(playlist_index)02d - %(title)s.%(ext)s")
        else:
            dl_opts["outtmpl"] = str(out_dir / "%(title)s.%(ext)s")

        if TOOLCHAIN.aria2c:
            dl_opts["external_downloader"] = {
                "default": TOOLCHAIN.aria2c,
                "dash": "native",
                "m3u8": "native",
            }
            dl_opts["external_downloader_args"] = {
                "aria2c": [
                    f"-x{safe_conn}",
                    f"-s{safe_conn}",
                    "-k1M",
                    "--file-allocation=none",
                ]
            }
        dl_opts["concurrent_fragment_downloads"] = safe_conn

        postprocessors: list[dict[str, Any]] = [{"key": "FFmpegMetadata", "add_metadata": True}]
        if subs:
            dl_opts["writesubtitles"] = True
            dl_opts["subtitleslangs"] = ["en.*", "all"]
            postprocessors.append({"key": "FFmpegEmbedSubtitle"})
        if embed_thumbnail:
            dl_opts["writethumbnail"] = True
            postprocessors.append({"key": "EmbedThumbnail"})
        if sponsorblock:
            postprocessors.append({"key": "SponsorBlock", "categories": ["sponsor", "intro", "outro", "selfpromo"]})
            postprocessors.append(
                {"key": "ModifyChapters", "remove_sponsor_segments": {"sponsor", "intro", "outro", "selfpromo"}}
            )
        dl_opts["postprocessors"] = postprocessors

        try:
            with LiveTransferDisplay(title=title) as live:

                def _hook(d: dict[str, Any]) -> None:
                    if d.get("status") == "downloading":
                        live.update(
                            downloaded_bytes=int(d.get("downloaded_bytes") or 0),
                            total_bytes=int(d.get("total_bytes") or d.get("total_bytes_estimate") or 0),
                            speed_bps=float(d.get("speed") or 0.0),
                            connections=safe_conn,
                            watchdog_state="yt-dlp",
                        )

                dl_opts["progress_hooks"] = [_hook]
                with yt_dlp.YoutubeDL(dl_opts) as ydl:
                    ydl.download([info_dict.get("webpage_url") or url])

            cleanup_sidecar_thumbnails(out_dir, title)
            mp4_files = sorted(out_dir.glob("*.mp4"), key=os.path.getmtime, reverse=True)
            final_vid = mp4_files[0] if mp4_files else out_dir
            fsize = final_vid.stat().st_size if final_vid.is_file() else 0
            dur = max(time.time() - t_start, 0.001)
            return TransferResult(
                success=True,
                file_path=final_vid,
                files=[final_vid] if final_vid.is_file() else [],
                bytes_downloaded=fsize,
                duration=dur,
                avg_speed_bps=fsize / dur if fsize else 0.0,
            )
        except Exception as exc:
            msg = f"Video download failed: {exc}"
            print_plain(f"  [error]✗ {msg}[/error]", force=True)
            return TransferResult(success=False, error=msg, exit_code=int(ExitCode.FAILURE))

    def execute(
        self,
        target_info: TargetInfo,
        out_dir: Path,
        *,
        connections: int = 16,
        **options: Any,
    ) -> TransferResult:
        is_audio = bool(options.get("is_audio", False))
        if is_audio:
            return self.download_audio(
                url=target_info.clean_url,
                out_dir=out_dir,
                connections=connections,
                codec=str(options.get("audio_codec", "lossless")),
                is_playlist=bool(options.get("playlist", False)),
                cookies=options.get("cookies_from_browser"),
                sponsorblock=bool(options.get("sponsorblock", False)),
                embed_thumbnail=bool(options.get("embed_thumbnail", True)),
                limit_rate=options.get("limit_rate"),
                anti_throttle=options.get("anti_throttle", "4MBps"),
                grace_period=int(options.get("grace_period", 5)),
                watchdog_mode=options.get("watchdog_mode", WatchdogMode.V2),
                ejs_remote=bool(options.get("ejs_remote", False)),
                player_client=options.get("player_client"),
            )
        return self.download_video(
            url=target_info.clean_url,
            out_dir=out_dir,
            connections=connections,
            quality=str(options.get("video_quality", "best")),
            is_playlist=bool(options.get("playlist", False)),
            subs=bool(options.get("subs", False)),
            cookies=options.get("cookies_from_browser"),
            sponsorblock=bool(options.get("sponsorblock", False)),
            embed_thumbnail=bool(options.get("embed_thumbnail", True)),
            limit_rate=options.get("limit_rate"),
            anti_throttle=options.get("anti_throttle", "4MBps"),
            grace_period=int(options.get("grace_period", 5)),
            watchdog_mode=options.get("watchdog_mode", WatchdogMode.V2),
            ejs_remote=bool(options.get("ejs_remote", False)),
            player_client=options.get("player_client"),
        )
