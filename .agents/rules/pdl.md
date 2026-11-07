# NON-NEGOTIABLES

- The anti-throttle watchdog stays and must work at least as well as today. The flags
  `--anti-throttle`, `--no-anti-throttle`, `--anti-throttle-grace` and the config keys
  `anti_throttle`, `anti_throttle_grace` keep working with the same meaning.
- Every existing subcommand, flag, config key, and target grammar (`ipsw:`, `hf:`, `gh:`,
  Drive/Dropbox/Pastebin links, media URLs, magnets, free-text search) keeps working.
  Any unavoidable break must be listed in the final report with a migration note.
- Python 3.11+ (3.12 preferred). macOS arm64 is the primary platform. Guard
  macOS-only tools (`afconvert`, `hdiutil`) behind platform checks so tests run anywhere.
- Max 16 connections per server stays enforced everywhere, including media, GitHub
  batch, and HF batch paths.
- Never block on a prompt in non-interactive use. Prompts happen only with `--select` on
  a TTY. Add `--non-interactive` that forbids prompts entirely.
- Never put secrets (`HF_TOKEN`, GitHub token, RPC secret) on a command line. Pass them
  through 0600 aria2 input files or RPC options, and never print them in logs.
- Keep dependencies lean. Use argparse for the CLI (NOT Typer/Click: the form
  `pdl <target>` with free text plus subcommands like `install`/`info`/`hash` must keep its
  exact current disambiguation). Allowed runtime deps: `rich`, `httpx`, `mutagen`, `yt-dlp[default]`
  (which includes `yt-dlp-ejs`). Everything else is stdlib.
