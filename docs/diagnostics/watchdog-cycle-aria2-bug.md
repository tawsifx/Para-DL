# Diagnosis: Watchdog v2 socket-cycle corrupts redirected aria2 downloads

## Symptom
`paradl https://github.com/anefzaoui/misterdownloadmanager/archive/refs/heads/main.zip`
reports `100% / 31.8 MB` within seconds but leaves only ~50–400 KB on disk and
exits 1. The CLI error line is empty (`✗ Download failed: <url>` with no reason).

## Reproduction (deterministic)
```
paradl <github.com/.../archive/.../main.zip> -o /tmp/mdm-ref --non-interactive -c 1
```
Instrumenting `Aria2RpcDaemon.tell_status` shows:

```
RPC- status=active done=16384 total=33304529 err=None
⚡ [Watchdog Cycle #1] [NET_CHANGE] Network change detected via scutil (4 notification(s))
RPC- status=error done=16384 total=33304529 err=8
ERROR status: code=8 msg='No URI available.'
```

With `--watchdog off`, no `NET_CHANGE` cycle occurs and aria2 streams normally
(direct `aria2c` pulled the full 33,304,529-byte archive).

## Root cause chain
1. `paradl.engines.aria2` daemon is configured with `continue=true`,
   `allow-overwrite=true`, `conditional-get=true` and an isolated RPC port.
2. The GitHub URL **302-redirects** to `codeload.github.com/...`. codeload sends
   **no `Content-Length` and no `Accept-Ranges`** on the final 200, so aria2
   falls back to a single stream and only learns the size (~33.3 MB) mid-transfer.
3. Watchdog v2's `ResilienceEventSource` polls macOS `scutil` and fires a
   `NET_CHANGE` event. `download_batch` handles `RESILIENCE_RECYCLE` by calling
   `daemon.force_pause(gid)`, `sleep(0.15)`, then `daemon.unpause(gid)`
   (`src/paradl/engines/aria2.py:1165-1185`).
4. For a download whose only URI came from a redirect, aria2 does not persist the
   redirected URI in its URI pool. After the forced pause/unpause cycle the
   effective URI is gone, so aria2 emits `errorCode=8, "No URI available."`.
5. The engine returns `TransferResult(success=False, ...)`. Two secondary bugs
   make this misleading:
   - the CLI prints `res['target']` but not `res['error']` (`app/commands.py:1162`),
     so the failure reason is invisible;
   - the live progress display jumps to `100% / total` on the error tick because
     `total_done`/`total_len` are refreshed from the GID whose `totalLength` is
     already known, giving a false “complete” impression.

## Why this matters for the Rust migration
This is a concrete example of the JSON-RPC/aria2 control-plane fragility the
`mdm-engine` migration is meant to remove. In the Rust engine, socket recycling
must be implemented as an in-process worker cancellation/reconnect on the same
`Range` offsets — never as a pause/unpause of a daemon-held URI list. The parity
tests in the migration must assert that a mid-transfer recycle preserves the
byte offset and the URI pool.

## Suggested follow-up fixes (not applied here; separate scope)
- Re-add URIs after an unpause via `aria2.changeUri(gid, add=[...])`, or recycle
  by toggling connection options instead of pause/unpause.
- Surface `res['error']` in the CLI failure message.
- Do not emit a 100% progress update from an errored GID.
