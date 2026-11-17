# Security

Please report security problems privately, not in a public issue: use GitHub's
**[Report a vulnerability](https://github.com/anefzaoui/misterdownloadmanager/security/advisories/new)**
form on this repository. Say what you found, how to reproduce it, and which version you used
(**Help → About** shows it).

You'll get an answer as soon as possible, and a fix is released before the details are made
public.

What matters most here:

- **The browser host and the command channel.** Downloads handed over by the browser extension
  go through the native messaging host (`crates/nmhost`), which only answers MDM's own extension,
  and the running app listens only on `127.0.0.1` with a random token (`crates/ipc`).
- **Helpers downloaded at run time.** yt-dlp is checked against its developers' signing key and
  ffmpeg against its published checksums before either runs (`crates/tools`).
- **Files written to disk.** Downloaded names are sanitised before they become paths
  (`crates/engine/src/naming.rs`).

Only the latest release gets security fixes during the preview (0.x).
