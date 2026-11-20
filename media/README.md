# paradl-media

Standalone Python media worker for the paraDL Rust framework.

This package owns **all** media extraction logic (yt-dlp / mutagen / ffmpeg /
CoreAudio) and is invoked by the native `paradl` binary over the versioned
`paradl-media/1` JSON-lines protocol. It is a **one-shot** process:

```sh
python -m paradl_media
```

One request line is read from stdin; zero or more `progress` frames plus
exactly one terminal `result` or `error` line are written to stdout. All human
logs and diagnostics go to stderr only (`stdout` is protocol-only).

It intentionally has **no** `paradl.*` imports, **no** console script, and no
PyO3 / sidecar runtime. External tools (`ffmpeg`, `afconvert`, JS runtimes)
remain separately invoked processes.
