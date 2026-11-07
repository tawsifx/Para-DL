# Design Document: Windows 10 & 11 Compatibility for `parallel-dl` (`paradl`)

**Date**: 2026-11-08  
**Topic**: Windows 10 and Windows 11 Native Architecture & Compatibility Support  
**Status**: Approved by Architect Review — Ready for Implementation  

---

## 1. Intent and The Why

`parallel-dl` (`paradl` / `pdl`) is an ultra-fast multi-channel download accelerator designed to eliminate single-stream ISP throttling through 16 parallel range channels, Watchdog v2 socket recycling, and cryptographic validation.

Currently, `para-DL` is heavily coupled to macOS system internals (`caffeinate`, `scutil`, `pmset`, Apple Keychain, AppleScript `osascript`, `afconvert`, `xattr`). However, users on Windows 10 and Windows 11 face the exact same ISP bandwidth throttling and need a reliable, native multi-channel download accelerator for PyPI wheels, GitHub release binaries, Hugging Face AI models, large media streams, and direct HTTP/HTTPS transfers.

The goal of this project is to make `para-DL` a first-class, dual-platform utility fully compatible with Windows 10 and Windows 11 (x64 and ARM64) alongside macOS, maintaining full feature parity, zero new external runtime dependencies, and rock-solid backward compatibility.

---

## 2. Goals, Non-Goals, and Anti-Goals

### Goals
- **Full Windows 10 & 11 Compatibility**: Native support for Windows 10 (build 1809+) and Windows 11 on both x86_64 (`win_amd64`) and ARM64 (`win_arm64`).
- **Zero New External Runtime Dependencies**: All Windows platform integrations must use the Python standard library (`ctypes`, `subprocess`, `os`, `shutil`, `re`, `winreg` / `pathlib`).
- **Sleep Prevention & Power Policy**: Use Windows `SetThreadExecutionState` via `kernel32` to prevent system sleep during transfers, and `GetSystemPowerStatus` for AC power and battery saver checks.
- **Windows Package & Binary Discovery**:
  - `paradl pip`: Accurately score and download Windows wheels (`win_amd64`, `win32`, `win_arm64`, `any`).
  - `paradl install` & `paradl gh:`: Accurately score Windows release binaries (`.zip`, `.exe`, `.msi`, `windows-msvc`, `win64`). For `pdl install`, prioritize portable CLI archives and standalone executables over `.msi` installers.
  - Toolchain discovery: Automatically discover `aria2c.exe`, `ffmpeg.exe`, `yt-dlp.exe`, `curl.exe`, `7z.exe`, `deno.exe`, `node.exe` across Windows PATH, Scoop (`~/scoop/shims`), Chocolatey (`ProgramData\chocolatey\bin`), Winget, and Program Files.
- **Origin Metadata & Mark of the Web**: Use NTFS Alternate Data Streams (`:Zone.Identifier`) to record source URLs (`HostUrl`) and support unblocking/quarantine clearing for verified binaries without destroying metadata.
- **Credential Storage**: Support secure token storage on Windows via Windows Credential Manager (`wincred` C-API via `advapi32.dll`) with DPAPI encrypted user file fallback in `%LOCALAPPDATA%\pdl\credentials.dat`.
- **Windows Toast Notifications**: Provide native Windows 10/11 toast notifications via PowerShell.
- **Environment Diagnostics**: Update `paradl doctor` to audit Windows-native utilities (PowerShell, curl, tar, winget, power management) rather than failing on macOS tools.
- **Installation & Documentation**: Add `scripts/install-local.ps1` and comprehensively update `README.md` and `pyproject.toml`.

### Non-Goals
- Supporting legacy Windows 7, 8, or Windows XP.
- Porting macOS-only Apple proprietary tools like `afconvert` (lossless AAC/ALAC conversion on Windows uses `ffmpeg`, which is already supported).
- Rewriting `aria2c` in Python; `aria2c` remains the high-speed subprocess daemon.

### Anti-Goals
- Breaking any existing macOS features, flags, commands, or behavior.
- Adding heavyweight third-party C-extensions (e.g., `pywin32`) that would complicate `pipx` or `uv tool` installation.
- Modifying system-wide registry hives without consent or requiring Administrator privileges for standard user downloads.

---

## 3. Constraints
- Strict adherence to Python 3.11+ (targeting 3.12).
- Zero non-stdlib additions: `rich`, `httpx`, `mutagen`, `yt-dlp[default]` remain the only external dependencies.
- Non-blocking execution: no interactive stdin prompts unless specifically requested via `--select` on a TTY.
- Path cross-platform safety: sanitize file paths for Windows reserved names (`CON`, `PRN`, `AUX`, `NUL`, `COM1-9`, `LPT1-9`) and characters (`:`, `*`, `?`, `"`, `<`, `>`, `|`).

---

## 4. Approaches Considered

### Approach 1: Clean Modular Platform Abstraction (`pdl.platform.windows` + cross-platform dispatcher) [PICKED]
- Create `src/pdl/platform/windows.py` containing native Windows implementations of sleep prevention (`kernel32.SetThreadExecutionState`), power status (`kernel32.GetSystemPowerStatus`), network SSID detection (`netsh wlan`), toast notifications (PowerShell), Zone.Identifier ADS handling, credential management, and Windows utility checks.
- Unify `src/pdl/platform/__init__.py` to dispatch to `windows` or `macos` based on `sys.platform`.
- Update `pdl.engines.pypi`, `pdl.engines.github`, `pdl.toolchain`, and `pdl.doctor` to use the unified platform interface and Windows scoring.
- Add `scripts/install-local.ps1` and update `pyproject.toml` and `README.md`.
- **Why Picked**: Keeps platform-specific system calls cleanly isolated, guarantees zero regressions for macOS, facilitates clear unit/integration testing, and adheres to clean architecture principles.

### Approach 2: Inline OS branching (`if sys.platform == "win32"` scattered across all files) [REJECTED]
- Injects conditional statements directly inside `app.py`, `events.py`, `doctor.py`, `auth.py`, `queue.py`, and `engines/`.
- **Why Rejected**: Spaghettifies codebase, violates separation of concerns, makes test mocking cumbersome, and creates high maintenance overhead.

---

## 5. Detailed Architecture & Design Decisions

### 5.1 Platform Abstraction Layer (`src/pdl/platform/`)
The platform package will provide a unified interface:
- `is_windows() -> bool`: Returns `sys.platform == "win32"`.
- `is_macos() -> bool`: Returns `sys.platform == "darwin"`.
- `sleep_assertion(enabled: bool = True) -> ContextManager`:
  - On macOS: Runs `caffeinate -i -w <pid>`.
  - On Windows: Invokes `ctypes.windll.kernel32.SetThreadExecutionState(0x80000000 | 0x00000001)` (`ES_CONTINUOUS | ES_SYSTEM_REQUIRED`) on enter, and resets with `ES_CONTINUOUS` (0x80000000) on exit. Safe fallback when ctypes/kernel32 is unavailable or mocked.
- `get_power_status() -> PowerStatus`:
  - On macOS: Calls `pmset -g batt`.
  - On Windows: Calls `ctypes.windll.kernel32.GetSystemPowerStatus`, mapping `ACLineStatus == 1` to `on_ac_power=True`, `BatteryLifePercent` (0-100), and `SystemStatusFlag == 1` to `low_power_mode=True`.
- `get_network_id() -> str`:
  - On macOS: Queries `scutil` / AirPort.
  - On Windows: Parses `netsh wlan show interfaces` for `SSID` or `Get-NetConnectionProfile` for active network name / primary interface alias.
- `NetworkWatcher`:
  - On macOS: `MacOSNetworkWatcher` using `scutil` / `route`.
  - On Windows: `WindowsNetworkWatcher` that monitors primary IP/gateway changes and SSID transitions using lightweight polling (debounced ~2s), matching the macOS interface.
- `get_free_disk_bytes(target_path: Path) -> int`:
  - Uses `shutil.disk_usage(probe_dir).free` (cross-platform, works identically on NTFS and APFS).
- `set_where_froms(filepath: Path, urls: list[str]) -> bool`:
  - On macOS: Writes `com.apple.metadata:kMDItemWhereFroms`.
  - On Windows: Writes the origin URL to NTFS Alternate Data Stream `<filepath>:Zone.Identifier` under `[ZoneTransfer]\nHostUrl=<url>\n`. If the file already has `ZoneId`, preserves it.
- `remove_quarantine(filepath: Path) -> bool`:
  - On macOS: `xattr -d com.apple.quarantine`.
  - On Windows: Reads `<filepath>:Zone.Identifier` as an INI stream. If other keys like `HostUrl` exist, rewrites the stream stripping `ZoneId`; if only `ZoneId` was present, removes the `:Zone.Identifier` stream entirely. On non-NTFS volumes, gracefully returns True.
- `has_quarantine(filepath: Path) -> bool`:
  - On macOS: Checks `com.apple.quarantine` via `xattr`.
  - On Windows: Reads `<filepath>:Zone.Identifier`. Returns True only if `ZoneId` is present and `>= 3` (Internet zone).
- `display_notification(title: str, message: str) -> bool`:
  - On macOS: `osascript -e 'display notification ...'`.
  - On Windows: Fires a Windows 10/11 notification via PowerShell using `[Windows.UI.Notifications.ToastNotificationManager]` or notification balloon script.
- `get_auth_token(service: str, account: str) -> str | None`, `set_auth_token`, `delete_auth_token`, `has_auth_token`:
  - On macOS: Apple Keychain via `security`.
  - On Windows: Windows Credential Manager via `advapi32.dll` (`CredReadW`, `CredWriteW`, `CredDeleteW`) targeting `pdl/<service>/<account>`. Reports source `"credential_manager"`. Fallback: DPAPI encrypted storage in `%LOCALAPPDATA%\pdl\credentials.dat`.

### 5.2 Wheel Scoring for Windows (`src/pdl/engines/pypi.py`)
Update `score_wheel_filename`:
- When `plat == "win32"`:
  - Supports `win_amd64` (64-bit x86), `win_arm64` (ARM64 Windows), `win32` (32-bit x86), and `any`.
  - Accurately scores Python ABI tags (`cp312`, `cp311`, `abi3`, `py3`).

### 5.3 GitHub Release Binary Scoring & Installation (`src/pdl/engines/github.py`)
Update `score_release_asset`:
- When `platform.system() == "Windows"`:
  - For `pdl gh:`: Adds positive score for Windows archive formats (`.zip`), executables (`.exe`), and installers (`.msi`).
  - For `pdl install`: Strictly prioritizes standalone executables and portable `.zip` archives over installer packages (`.msi`, `setup.exe`).
  - Prioritizes architecture matching: `x86_64-pc-windows-msvc`, `x86_64-windows`, `win64`, `amd64` on 64-bit, or `arm64-windows` on ARM64.
- `pdl install` binary directory & PATH:
  - Default `bin_dir`: `%LOCALAPPDATA%\Programs\pdl\bin` or `%USERPROFILE%\.local\bin`.
  - Does not silently mutate the user registry environment; audits whether `bin_dir` is in `PATH` and outputs a clear PowerShell setup command if needed.
  - Windows signature check: Inspects Authenticode status via PowerShell `Get-AuthenticodeSignature` during `pdl install`.

### 5.4 Toolchain Discovery on Windows (`src/pdl/toolchain.py`)
Expand `Toolchain` discovery:
- Add Windows search paths:
  - Scoop shims: `Path.home() / "scoop" / "shims"`
  - Chocolatey: `Path("C:/ProgramData/chocolatey/bin")`
  - Winget links: `Path.home() / "AppData/Local/Microsoft/WinGet/Links"`
  - Program Files: `Path("C:/Program Files/aria2")`, `Path("C:/Program Files/7-Zip")`, `Path("C:/Program Files/Git/usr/bin")`
  - Active Python virtual environment `Scripts/` directory (`Path(sys.executable).parent`).
- Ensure Windows executable extensions (`.exe`, `.cmd`, `.bat`) are handled seamlessly.

### 5.5 Environment Diagnostics (`src/pdl/doctor.py`)
- Platform-aware audit:
  - On macOS: Audits `caffeinate, scutil, pmset, security, codesign, spctl, xattr`.
  - On Windows: Audits `powershell` (or `pwsh`), `curl`, `tar`, `winget` (or `scoop`/`choco`), `aria2c.exe`, `ffmpeg.exe`, and Windows Power/Sleep state API.
  - Displays Windows remediation commands (e.g. `winget install aria2.aria2`, `winget install Gyan.FFmpeg`).

### 5.6 Project Metadata & Documentation
- `pyproject.toml`:
  - Add classifiers for `Operating System :: Microsoft :: Windows`, `Operating System :: Microsoft :: Windows :: Windows 10`, `Operating System :: Microsoft :: Windows :: Windows 11`.
  - Update description/keywords to reflect cross-platform Windows & macOS support.
- `scripts/install-local.ps1`:
  - PowerShell installation script for Windows 10 and 11 (`uv tool install --force .`).
- `README.md`:
  - Add Windows badges.
  - Document Windows installation via Winget, Scoop, pipx, and uv tool.
  - Document Windows prerequisites (`winget install aria2.aria2 Gyan.FFmpeg`).
  - Add PowerShell examples for common workflows.

---

## 6. Verification Plan
- Syntax and type checking across all files (`python3 -m py_compile`).
- Automated tests verifying:
  - `pdl.platform.windows` ctypes mock and fallback behaviors (sleep assertion, power status, network ID, Zone.Identifier parsing).
  - PyPI Windows wheel scoring (`win_amd64`, `win_arm64`, `any`).
  - GitHub release asset scoring for Windows assets (`.zip`, `.exe`, msvc).
  - Doctor diagnostics on Windows without failing on macOS tools.
