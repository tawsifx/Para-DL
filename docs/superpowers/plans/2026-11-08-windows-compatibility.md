# Windows 10 & 11 Compatibility Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `parallel-dl` (`paradl` / `pdl`) fully compatible with Windows 10 and Windows 11 (x64 and ARM64) while updating the documentation (`README.md`, `pyproject.toml`, and context references) to reflect native dual-platform support.

**Architecture:** Implement a modular `pdl.platform.windows` subsystem using the Python standard library (`ctypes`, `subprocess`, `os`, `shutil`) that mirrors `pdl.platform.macos` functionality (sleep prevention, power policy, network ID, Zone.Identifier ADS metadata, Windows Credential Manager, and toast notifications). Unify `pdl.platform` dispatching, update PyPI wheel and GitHub release asset scoring for Windows targets, expand toolchain binary discovery, implement platform-aware `doctor` diagnostics, and add PowerShell installation tooling.

**Tech Stack:** Python 3.11+ (targeting 3.12), `ctypes` (Win32 `kernel32.dll`, `advapi32.dll`), `subprocess` (PowerShell, `netsh`, `cmdkey`), `rich`, `httpx`, `mutagen`, `yt-dlp`.

**Spec:** [docs/superpowers/specs/2026-11-08-windows-compatibility-design.md](file:///Users/tawsifx/pdl/docs/superpowers/specs/2026-11-08-windows-compatibility-design.md)

## Global Constraints

- Python version: >= 3.11 (Python 3.12 targeted).
- Dependencies: Zero new external dependencies (runtime deps remain strictly `rich`, `httpx`, `mutagen`, `yt-dlp[default]`).
- Platforms: Windows 10 (build 1809+), Windows 11 (x64 and ARM64), and macOS (Apple Silicon & Intel).
- Backward Compatibility: Keep all existing macOS functionality, CLI subcommands, options, and event schemas fully working without regression.
- Security: Never expose secrets on the command line or unredacted in logs. Never mutate system registry hives without consent or require elevation for standard operations.

## Review Focus

1. **Non-NTFS or Restricted Filesystems**: Writing Zone.Identifier or querying ADS on FAT32/exFAT drives raises `OSError` or `FileNotFoundError` — handlers must catch filesystem errors and cleanly fall back.
2. **Missing `advapi32` or Access Denied in Credential Manager**: Headless or constrained user environments must seamlessly fall back to encrypted DPAPI/user file storage without crashing.
3. **PowerShell Execution Policy Restrictions**: PowerShell commands must run with `-NoProfile -NonInteractive -ExecutionPolicy Bypass` when executing helper scripts (e.g. notifications, Authenticode checks).
4. **Platform Wheel & Asset Disambiguation**: When running on Windows, `win_amd64` must beat `win32` on 64-bit OS, and `.msi` installers must be excluded from `pdl install` binary directory placement.
5. **Doctor Diagnostic Cross-Platform Cleanliness**: Running `paradl doctor` on Windows must never report missing macOS utilities as failures.

---

### Task 1: Windows Platform Subsystem (`pdl.platform.windows` and `pdl.platform.__init__`)

**Files:**
- Create: `src/pdl/platform/windows.py`
- Modify: `src/pdl/platform/__init__.py:1-7`
- Test: `tests/test_platform_windows.py`

**Interfaces:**
- Consumes: Standard library (`ctypes`, `subprocess`, `os`, `shutil`, `re`, `pathlib`)
- Produces:
  - `is_windows() -> bool`
  - `is_macos() -> bool`
  - `WindowsSleepAssertion(enabled: bool = True, pid: int | None = None)`
  - `get_power_status() -> PowerStatus`
  - `get_network_id() -> str`
  - `WindowsNetworkWatcher`
  - `set_where_froms(filepath: Path, urls: list[str]) -> bool`
  - `get_where_froms(filepath: Path) -> list[str]`
  - `has_quarantine(filepath: Path) -> bool`
  - `remove_quarantine(filepath: Path) -> bool`
  - `display_notification(title: str, message: str) -> bool`
  - `get_auth_token(account: str, service: str = "pdl") -> str | None`
  - `set_auth_token(account: str, token: str, service: str = "pdl") -> bool`
  - `delete_auth_token(account: str, service: str = "pdl") -> bool`
  - `has_auth_token(account: str, service: str = "pdl") -> bool`
  - `check_windows_utilities() -> dict[str, dict[str, Any]]`

- [ ] **Step 1: Write the failing tests in `tests/test_platform_windows.py`**
  Write tests covering:
  - `WindowsSleepAssertion` invokes kernel32 `SetThreadExecutionState` or safely falls back.
  - `get_power_status()` correctly decodes simulated `SYSTEM_POWER_STATUS` struct fields (AC power, battery %, low power mode).
  - Zone.Identifier ADS reader/writer parses INI, writes `HostUrl`, checks `ZoneId >= 3`, and removes `ZoneId` without destroying `HostUrl`.
  - Credential Manager operations handle token setting, retrieval, deletion, and DPAPI fallback.
  - `check_windows_utilities()` returns readiness info for powershell, curl, tar, winget, aria2c.

- [ ] **Step 2: Run test to verify it fails**
  Run: `python3 -m unittest tests/test_platform_windows.py`
  Expected: FAIL (ModuleNotFoundError: No module named 'pdl.platform.windows')

- [ ] **Step 3: Implement `src/pdl/platform/windows.py` and update `src/pdl/platform/__init__.py`**
  - Implement ctypes kernel32 sleep assertion (`ES_CONTINUOUS | ES_SYSTEM_REQUIRED` = 0x80000001) and power status struct.
  - Implement `netsh wlan show interfaces` and `Get-NetConnectionProfile` parsing for network identity.
  - Implement `WindowsNetworkWatcher` polling thread for SSID/IP changes.
  - Implement INI-based `:Zone.Identifier` stream reader/writer with non-NTFS error suppression.
  - Implement `advapi32.dll` credential manager (`CredReadW`, `CredWriteW`, `CredDeleteW`) with DPAPI user file fallback.
  - Implement PowerShell toast notification caller (`-NoProfile -NonInteractive`).
  - Implement `check_windows_utilities()`.
  - In `src/pdl/platform/__init__.py`, export platform selectors and dispatchers while keeping `macos` exported for backwards compatibility.

- [ ] **Step 4: Run tests to verify they pass**
  Run: `python3 -m unittest tests/test_platform_windows.py`
  Expected: PASS

- [ ] **Step 5: Commit**
  ```bash
  git add src/pdl/platform/windows.py src/pdl/platform/__init__.py tests/test_platform_windows.py
  git commit -m "feat(platform): implement native Windows 10 and 11 subsystem"
  ```

---

### Task 2: Platform Unification across Core Modules (`paths`, `config`, `events`, `auth`, `hooks`, `doctor`)

**Files:**
- Modify: `src/pdl/paths.py:1-49`
- Modify: `src/pdl/config.py:40-70`
- Modify: `src/pdl/events.py:10-70, 240-295`
- Modify: `src/pdl/auth.py:15-125`
- Modify: `src/pdl/hooks.py:15-165`
- Modify: `src/pdl/doctor.py:15-225`
- Test: `tests/test_platform_unification.py`

**Interfaces:**
- Consumes: `pdl.platform` unified dispatchers (`is_windows`, `is_macos`, `get_power_status`, `get_network_id`, `display_notification`, `get_auth_token`, etc.)
- Produces:
  - Cross-platform state directory pathing (`%LOCALAPPDATA%\pdl` on Windows vs `~/.config/pdl` on macOS/Linux).
  - Platform-adaptive `bin_dir` (`%LOCALAPPDATA%\Programs\pdl\bin` on Windows vs `~/.local/bin`).
  - Cross-platform network and power watchers in `events.py`.
  - Cross-platform auth status reporting (`source: "credential_manager"` or `"keychain"` or `"dpapi"`).
  - Platform-adaptive doctor output auditing Windows tools on Windows and macOS tools on macOS.

- [ ] **Step 1: Write the failing tests in `tests/test_platform_unification.py`**
  Write tests covering:
  - `paths.get_pdl_home()` returns `%LOCALAPPDATA%\pdl` or `%APPDATA%\pdl` on Windows when `PDL_HOME` is unset.
  - `auth.get_auth_status()` queries platform credential store and reports the appropriate source tag.
  - `hooks.notify_download_complete()` calls platform-appropriate notification.
  - `doctor.run_doctor()` executes cleanly on both platforms, selecting appropriate utility checks.

- [ ] **Step 2: Run test to verify it fails**
  Run: `python3 -m unittest tests/test_platform_unification.py`
  Expected: FAIL

- [ ] **Step 3: Implement core modules platform unification**
  - Update `src/pdl/paths.py` with Windows local appdata resolution.
  - Update `src/pdl/config.py` default `bin_dir`.
  - Update `src/pdl/events.py` to use `pdl.platform.get_network_watcher_cls()`.
  - Update `src/pdl/auth.py` to use `pdl.platform` token functions.
  - Update `src/pdl/hooks.py` to use `pdl.platform.display_notification`.
  - Update `src/pdl/doctor.py` to switch between `check_macos_utilities` and `check_windows_utilities` based on `is_windows()`.

- [ ] **Step 4: Run tests to verify they pass**
  Run: `python3 -m unittest tests/test_platform_unification.py`
  Expected: PASS

- [ ] **Step 5: Commit**
  ```bash
  git add src/pdl/paths.py src/pdl/config.py src/pdl/events.py src/pdl/auth.py src/pdl/hooks.py src/pdl/doctor.py tests/test_platform_unification.py
  git commit -m "feat(core): integrate cross-platform platform dispatchers"
  ```

---

### Task 3: Engine Adaptations for Windows (`pypi`, `github`, `aria2`, `toolchain`)

**Files:**
- Modify: `src/pdl/engines/pypi.py:90-140`
- Modify: `src/pdl/engines/github.py:255-320`
- Modify: `src/pdl/engines/aria2.py:40-60, 580-600, 760-800, 1010-1030`
- Modify: `src/pdl/toolchain.py:45-125, 195-230`
- Test: `tests/test_engines_windows.py`

**Interfaces:**
- Consumes: `pdl.platform` (`is_windows`, `is_macos`, `sleep_assertion`, `set_where_froms`)
- Produces:
  - `score_wheel_filename`: accurately scores `win_amd64`, `win_arm64`, `win32`, and `any` wheels when on Windows.
  - `score_release_asset`: accurately scores Windows `.zip`, `.exe`, `.msi`, and msvc binaries on Windows; filters out installer packages for `pdl install`.
  - `aria2.py`: uses unified sleep assertion and origin tracking.
  - `Toolchain`: discovers `.exe` binaries across Windows PATH, Scoop, Chocolatey, Winget, and Program Files.

- [ ] **Step 1: Write the failing tests in `tests/test_engines_windows.py`**
  Write tests covering:
  - `score_wheel_filename` on `plat == "win32"` scores `numpy-2.1.0-cp312-cp312-win_amd64.whl` with high score.
  - `score_release_asset` on Windows scores `ripgrep-14.1.1-x86_64-pc-windows-msvc.zip` highest, and downvotes `.msi` for `install_mode=True`.
  - `Toolchain._find_bin` discovers binaries with `.exe` extension and across Windows directory candidates.

- [ ] **Step 2: Run test to verify it fails**
  Run: `python3 -m unittest tests/test_engines_windows.py`
  Expected: FAIL

- [ ] **Step 3: Implement engine updates**
  - In `src/pdl/engines/pypi.py`, add `win32` platform scoring logic supporting `win_amd64`, `win_arm64`, `win32`, and `any`.
  - In `src/pdl/engines/github.py`, add Windows asset scoring with distinction between standalone CLI downloads and package installers.
  - In `src/pdl/engines/aria2.py`, replace direct `macos.CaffeinateAssertion` and `macos.set_where_froms` with unified `pdl.platform` calls.
  - In `src/pdl/toolchain.py`, add `.exe` extension lookup, Scoop shims (`%USERPROFILE%/scoop/shims`), Chocolatey (`C:/ProgramData/chocolatey/bin`), Winget, and Python `Scripts/` paths.

- [ ] **Step 4: Run tests to verify they pass**
  Run: `python3 -m unittest tests/test_engines_windows.py`
  Expected: PASS

- [ ] **Step 5: Commit**
  ```bash
  git add src/pdl/engines/pypi.py src/pdl/engines/github.py src/pdl/engines/aria2.py src/pdl/toolchain.py tests/test_engines_windows.py
  git commit -m "feat(engines): add Windows wheelhouse, GitHub asset scoring, and toolchain discovery"
  ```

---

### Task 4: Packaging, Scripts & Comprehensive Documentation Update

**Files:**
- Create: `scripts/install-local.ps1`
- Modify: `pyproject.toml:20-40`
- Modify: `README.md:1-170`
- Modify: `PARALLEL_DL_DEV_CONTEXT.md:1-60`

**Interfaces:**
- Consumes: Validated cross-platform pdl codebase
- Produces:
  - Windows PowerShell local installation script (`scripts/install-local.ps1`).
  - PyPI metadata with Windows 10 & 11 classifiers and dual-platform keywords.
  - Fully updated `README.md` with Windows badges, prerequisites, install guides, and PowerShell recipes.
  - Updated `PARALLEL_DL_DEV_CONTEXT.md` architecture specification.

- [ ] **Step 1: Create `scripts/install-local.ps1`**
  Write PowerShell installer that:
  - Verifies `uv` or `pipx` presence.
  - Executes `uv tool install --force .`.
  - Checks if installation directory is in user `PATH` and displays helpful remediation instructions.

- [ ] **Step 2: Update `pyproject.toml`**
  Add:
  - `Operating System :: Microsoft :: Windows`
  - `Operating System :: Microsoft :: Windows :: Windows 10`
  - `Operating System :: Microsoft :: Windows :: Windows 11`
  - Keywords: `windows`, `windows-10`, `windows-11`.

- [ ] **Step 3: Update `README.md`**
  - Add Windows platform badges (`Windows 10 / 11 | x64 & ARM64`).
  - Add Windows installation section (`winget`, `scoop`, `uv tool`, `pipx`, PowerShell).
  - Add Windows prerequisites section (`winget install aria2.aria2 Gyan.FFmpeg`).
  - Add PowerShell command recipes alongside Bash.
  - Update Architecture section to document the dual-platform macOS/Windows platform abstraction.

- [ ] **Step 4: Update `PARALLEL_DL_DEV_CONTEXT.md`**
  Document the Windows platform subsystem (`pdl.platform.windows`), `SetThreadExecutionState`, `GetSystemPowerStatus`, and Zone.Identifier ADS handling.

- [ ] **Step 5: Run full verification suite**
  Run:
  - `python3 -m unittest discover tests`
  - `python3 -m py_compile src/pdl/**/*.py parallel-dl.py`
  Expected: All tests pass and all files compile cleanly with zero errors.

- [ ] **Step 6: Commit**
  ```bash
  git add scripts/install-local.ps1 pyproject.toml README.md PARALLEL_DL_DEV_CONTEXT.md
  git commit -m "docs: document Windows 10 & 11 support, add install script, and update package metadata"
  ```
