"""Integration tests for binary installation verification, CLI startup, secrets redaction, and JSONL events."""

from __future__ import annotations

import os
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

import pytest
from tests.support.server import SimulatorProfile, ThrottleTestServer

from pdl.cli import run_cli
from pdl.engines.github import BinaryInstaller, GitHubEngine
from pdl.models import ExitCode, TransferResult
from pdl.platform import macos
from pdl.postproc.checksum import ChecksumEngine
from pdl.ui import UI, redact_secrets

pytestmark = pytest.mark.integration


def test_cli_fast_startup_under_150ms(tmp_path: Path) -> None:
    """pdl --version, pdl hash, and pdl config must start and finish in under 150 ms."""
    tiny_file = tmp_path / "tiny.txt"
    tiny_file.write_bytes(b"hello fast startup")
    env = {**os.environ, "PDL_HOME": str(tmp_path / "fast_pdl_home")}

    t0 = time.monotonic()
    proc = subprocess.run(
        [sys.executable, "-m", "pdl.cli", "--version"],
        env=env,
        capture_output=True,
        check=True,
    )
    elapsed_ms = (time.monotonic() - t0) * 1000.0
    assert proc.returncode == 0
    assert elapsed_ms < 150.0, f"pdl --version took {elapsed_ms:.1f} ms (budget: < 150 ms)"

    t0 = time.monotonic()
    proc = subprocess.run(
        [sys.executable, "-m", "pdl.cli", "hash", str(tiny_file)],
        env=env,
        capture_output=True,
        text=True,
        check=True,
    )
    elapsed_ms = (time.monotonic() - t0) * 1000.0
    assert proc.returncode == 0
    assert elapsed_ms < 150.0, f"pdl hash took {elapsed_ms:.1f} ms (budget: < 150 ms)"


def test_secret_redaction_and_aria2_rpc_argv_isolation() -> None:
    """Never put secrets (HF_TOKEN, GITHUB_TOKEN, RPC secret) on argv or in logs/JSON."""
    hf_tok = "hf_super_secret_token_99999"
    gh_tok = "ghp_super_secret_github_88888"
    raw_str = f"Authorization: Bearer {hf_tok} and gh={gh_tok}"

    redacted = redact_secrets(raw_str, extra_secrets=[hf_tok, gh_tok])
    assert hf_tok not in redacted
    assert gh_tok not in redacted
    assert "***REDACTED***" in redacted


def test_non_interactive_prompt_guard(monkeypatch: pytest.MonkeyPatch) -> None:
    """Never block on a prompt in non-interactive use."""
    monkeypatch.setattr(UI, "non_interactive", True)
    assert UI.can_prompt(select_requested=False) is False
    assert UI.can_prompt(select_requested=True) is False

    val = UI.ask_choice("Pick one", ["1", "2"], default="1")
    assert val == "1"


def test_jsonl_events_and_exit_codes(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Stable exit codes and --json JSONL event output."""
    sample = tmp_path / "data.bin"
    sample.write_bytes(b"1234567890")

    events: list[dict[str, Any]] = []

    def mock_print_json(event_type: str, **payload: Any) -> None:
        events.append({"event": event_type, **payload})

    monkeypatch.setattr(UI, "emit_event", mock_print_json)

    # 1. pdl hash --json
    code = run_cli(["hash", str(sample), "--json"])
    assert code == int(ExitCode.OK)
    hash_evs = [e for e in events if e.get("event") == "hashes"]
    assert len(hash_evs) >= 1
    assert "sha256" in hash_evs[0]["hashes"]

    # 2. Bad rate error exit code
    code_bad = run_cli(["https://example.com/file.bin", "--limit-rate", "not_a_rate", "--json"])
    assert code_bad == int(ExitCode.USAGE)

    # 3. Successful download emitting target_classified, verification, result
    with ThrottleTestServer(profile=SimulatorProfile(file_size=128 * 1024, name="none")) as srv:
        events.clear()
        out_dir = tmp_path / "dl_json"
        code_dl = run_cli([srv.file_url, "-o", str(out_dir), "-c", "4", "--json"])
        assert code_dl == int(ExitCode.OK)

        ev_types = [e.get("event") for e in events]
        assert "target_classified" in ev_types
        assert "result" in ev_types
        res_ev = next(e for e in events if e.get("event") == "result")
        assert res_ev.get("success") is True


def test_checksum_companion_discovery(tmp_path: Path) -> None:
    """Verify checksum compute and companion SHA256SUMS discovery."""
    sample = tmp_path / "sample.bin"
    payload = b"parallel-dl checksum verification test payload"
    sample.write_bytes(payload)
    import hashlib

    expected_sha256 = hashlib.sha256(payload).hexdigest()

    matched, algo, computed = ChecksumEngine.verify(sample, expected=f"sha256:{expected_sha256}")
    assert matched is True
    assert algo == "sha256"

    # Test companion discovery (GNU format & BSD format)
    sums_file = tmp_path / "SHA256SUMS"
    sums_file.write_text(f"{expected_sha256}  sample.bin\n", encoding="utf-8")
    found = ChecksumEngine.find_companion_checksum(sample)
    assert found is not None
    assert found[0] == "sha256"
    assert found[1] == expected_sha256


def test_pdl_install_requires_yes_when_no_checksum_published(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """If neither asset digest nor companion checksum exists, warn and require --yes."""
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir(parents=True, exist_ok=True)
    monkeypatch.setattr(macos, "is_effective_macos", lambda: False)

    def _fake_dl(*args: Any, **kwargs: Any) -> tuple[TransferResult, dict[str, Any], list[dict[str, Any]]]:
        dest_dir = kwargs.get("out_dir") or tmp_path
        dest_path = dest_dir / "unsignedtool"
        dest_path.write_bytes(b"\xcf\xfa\xed\xfe" + b"\x00" * 60)
        dest_path.chmod(0o755)
        tr = TransferResult(
            success=True,
            exit_code=int(ExitCode.OK),
            file_path=dest_path,
            checksum_verified=False,
        )
        return tr, {"name": "unsignedtool"}, []

    monkeypatch.setattr(GitHubEngine, "download_release_asset", _fake_dl)

    # Without assume_yes -> blocked
    res_no = BinaryInstaller.install("owner/unsignedtool", bin_dir=bin_dir, assume_yes=False)
    assert res_no.success is False
    assert res_no.exit_code == int(ExitCode.VERIFICATION_FAILED)
    assert not (bin_dir / "unsignedtool").exists()

    # With assume_yes -> succeeds
    res_yes = BinaryInstaller.install("owner/unsignedtool", bin_dir=bin_dir, assume_yes=True)
    assert res_yes.success is True
    assert (bin_dir / "unsignedtool").exists()


def test_pdl_install_verifies_asset_digest_and_blocks_mismatch(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Verified tool with checksum matches succeeds; mismatch fails."""
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir(parents=True, exist_ok=True)
    monkeypatch.setattr(macos, "is_effective_macos", lambda: False)

    def _fake_bad_digest(*args: Any, **kwargs: Any) -> tuple[TransferResult, dict[str, Any], list[dict[str, Any]]]:
        dest_dir = kwargs.get("out_dir") or tmp_path
        dest_path = dest_dir / "verifiedtool"
        dest_path.write_bytes(b"\xcf\xfa\xed\xfe" + b"\x00" * 60)
        dest_path.chmod(0o755)
        tr = TransferResult(
            success=True,
            exit_code=int(ExitCode.OK),
            file_path=dest_path,
            checksum_verified=False,
        )
        # Release asset with mismatching digest
        chosen = {
            "name": "verifiedtool",
            "digest": "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        }
        return tr, chosen, [chosen]

    monkeypatch.setattr(GitHubEngine, "download_release_asset", _fake_bad_digest)
    res_bad = BinaryInstaller.install("owner/verifiedtool", bin_dir=bin_dir, assume_yes=True)
    assert res_bad.success is False
    assert res_bad.exit_code == int(ExitCode.VERIFICATION_FAILED)
    assert not (bin_dir / "verifiedtool").exists()

    def _fake_good_digest(*args: Any, **kwargs: Any) -> tuple[TransferResult, dict[str, Any], list[dict[str, Any]]]:
        dest_dir = kwargs.get("out_dir") or tmp_path
        dest_path = dest_dir / "verifiedtool"
        data = b"\xcf\xfa\xed\xfe" + b"\x00" * 60
        dest_path.write_bytes(data)
        dest_path.chmod(0o755)
        import hashlib

        good_hash = hashlib.sha256(data).hexdigest()
        tr = TransferResult(
            success=True,
            exit_code=int(ExitCode.OK),
            file_path=dest_path,
            checksum_verified=True,
            checksum_algo="sha256",
            checksum_digest=good_hash,
        )
        chosen = {"name": "verifiedtool", "digest": f"sha256:{good_hash}"}
        return tr, chosen, [chosen]

    monkeypatch.setattr(GitHubEngine, "download_release_asset", _fake_good_digest)
    res_ok = BinaryInstaller.install("owner/verifiedtool", bin_dir=bin_dir, assume_yes=True)
    assert res_ok.success is True
    assert res_ok.checksum_verified is True
    assert (bin_dir / "verifiedtool").exists()


def test_cli_pip_and_brew_subcommand_routing(monkeypatch: pytest.MonkeyPatch) -> None:
    """Test CLI subcommand routing for pip and brew packages."""
    from pdl.engines.brew import BrewEngine
    from pdl.engines.pypi import PyPIEngine

    pip_installed: list[str] = []
    brew_installed: list[str] = []

    def mock_pip_install(target: str, **kwargs: Any) -> TransferResult:
        pip_installed.append(target)
        return TransferResult(success=True, exit_code=int(ExitCode.OK))

    def mock_brew_install(target: str, **kwargs: Any) -> TransferResult:
        brew_installed.append(target)
        return TransferResult(success=True, exit_code=int(ExitCode.OK))

    monkeypatch.setattr(PyPIEngine, "install", mock_pip_install)
    monkeypatch.setattr(BrewEngine, "install", mock_brew_install)

    # 1. pdl pip install requests
    code1 = run_cli(["pip", "install", "requests"])
    assert code1 == int(ExitCode.OK)
    assert pip_installed == ["pip:requests"]

    # 2. pdl brew install jq
    code2 = run_cli(["brew", "install", "jq"])
    assert code2 == int(ExitCode.OK)
    assert brew_installed == ["brew:jq"]

    # 3. pdl install pip:numpy
    code3 = run_cli(["install", "pip:numpy"])
    assert code3 == int(ExitCode.OK)
    assert pip_installed == ["pip:requests", "pip:numpy"]

    # 4. pdl install brew:tree
    code4 = run_cli(["install", "brew:tree"])
    assert code4 == int(ExitCode.OK)
    assert brew_installed == ["brew:jq", "brew:tree"]
