"""Integration tests: Keychain token management, zero-argv secret safety, and auth cascade."""

from __future__ import annotations

import subprocess
from typing import Any

import pytest
from tests.support.fakes import FakeKeychain

from pdl.auth import (
    get_auth_status,
    handle_auth_command,
    set_auth_token,
)
from pdl.engines.github import GitHubEngine
from pdl.engines.hf import HuggingFaceEngine
from pdl.models import ExitCode
from pdl.platform import macos
from pdl.ui import UI

pytestmark = pytest.mark.integration


def test_keychain_set_never_puts_secret_in_argv(monkeypatch: pytest.MonkeyPatch) -> None:
    """Verify storing a token via security uses -i over stdin and never exposes secret in argv."""
    captured_args: list[list[str]] = []
    captured_stdin: list[str] = []

    class DummyPopen:
        def __init__(self, args: list[str], **kwargs: Any) -> None:
            captured_args.append(args)
            self.returncode = 0

        def communicate(self, input: str | None = None, timeout: float | None = None) -> tuple[str, str]:
            if input is not None:
                captured_stdin.append(input)
            return "", ""

    monkeypatch.setattr(subprocess, "Popen", DummyPopen)
    monkeypatch.setattr(macos, "is_effective_macos", lambda: True)

    secret = "hf_realSecret_12345!@#$%"
    ok = macos.set_keychain_token("hf", secret, service="pdl")

    assert ok is True
    assert len(captured_args) == 1
    assert captured_args[0][-1] == "-i"
    assert secret not in " ".join(captured_args[0])
    assert len(captured_stdin) == 1
    assert secret in captured_stdin[0]
    assert 'add-generic-password -a "hf" -s "pdl" -U -w' in captured_stdin[0]


def test_auth_lookup_order_hf(monkeypatch: pytest.MonkeyPatch) -> None:
    """HF token lookup order: HF_TOKEN -> HUGGING_FACE_HUB_TOKEN -> Keychain -> None."""
    fake_kc = FakeKeychain()
    monkeypatch.setattr(macos, "get_keychain_token", lambda acc, **kw: fake_kc.get("pdl", acc))

    # 1. When HF_TOKEN is set, it wins
    monkeypatch.setenv("HF_TOKEN", "token_from_hf_env")
    monkeypatch.setenv("HUGGING_FACE_HUB_TOKEN", "token_from_hub_env")
    fake_kc.set("pdl", "hf", "token_from_keychain")
    assert HuggingFaceEngine.get_token() == "token_from_hf_env"

    # 2. When HF_TOKEN is unset, HUGGING_FACE_HUB_TOKEN wins over Keychain
    monkeypatch.delenv("HF_TOKEN", raising=False)
    assert HuggingFaceEngine.get_token() == "token_from_hub_env"

    # 3. When both env vars are unset, Keychain wins
    monkeypatch.delenv("HUGGING_FACE_HUB_TOKEN", raising=False)
    assert HuggingFaceEngine.get_token() == "token_from_keychain"

    # 4. When Keychain returns None, returns None
    fake_kc.clear()
    assert HuggingFaceEngine.get_token() is None


def test_auth_lookup_order_github(monkeypatch: pytest.MonkeyPatch) -> None:
    """GitHub token lookup order: GITHUB_TOKEN -> GH_TOKEN -> Keychain -> None."""
    fake_kc = FakeKeychain()
    monkeypatch.setattr(macos, "get_keychain_token", lambda acc, **kw: fake_kc.get("pdl", acc))

    # 1. GITHUB_TOKEN wins
    GitHubEngine.reset_token_cache()
    monkeypatch.setenv("GITHUB_TOKEN", "token_from_gh_env")
    monkeypatch.setenv("GH_TOKEN", "token_from_gh_short_env")
    fake_kc.set("pdl", "github", "token_from_gh_keychain")
    assert GitHubEngine.get_token() == "token_from_gh_env"

    # 2. GH_TOKEN wins over Keychain
    GitHubEngine.reset_token_cache()
    monkeypatch.delenv("GITHUB_TOKEN", raising=False)
    assert GitHubEngine.get_token() == "token_from_gh_short_env"

    # 3. Keychain wins when env is unset
    GitHubEngine.reset_token_cache()
    monkeypatch.delenv("GH_TOKEN", raising=False)
    assert GitHubEngine.get_token() == "token_from_gh_keychain"

    # 4. None when Keychain empty and gh CLI absent
    GitHubEngine.reset_token_cache()
    fake_kc.clear()
    from pdl.toolchain import TOOLCHAIN

    monkeypatch.setattr(type(TOOLCHAIN), "gh", property(lambda self: None))
    assert GitHubEngine.get_token() is None


def test_auth_status_does_not_print_secret(monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]) -> None:
    """Verify 'pdl auth status' reports configuration source without leaking secrets."""
    monkeypatch.setenv("HF_TOKEN", "super_secret_hf_token")
    monkeypatch.setattr(macos, "has_keychain_token", lambda acc, **kw: acc == "github")

    UI.json_mode = False
    UI.quiet = False
    code = handle_auth_command("status")
    assert code == int(ExitCode.OK)

    captured = capsys.readouterr().out
    assert "super_secret_hf_token" not in captured
    assert "Hugging Face" in captured
    assert "GitHub" in captured


def test_auth_set_and_remove_commands(monkeypatch: pytest.MonkeyPatch) -> None:
    """Verify 'pdl auth set' and 'pdl auth remove' CLI flows and service validation."""
    fake_kc = FakeKeychain()
    monkeypatch.setattr(
        macos,
        "set_keychain_token",
        lambda account, token, **kw: (fake_kc.set(kw.get("service", "pdl"), account, token), True)[1],
    )
    monkeypatch.setattr(
        macos, "delete_keychain_token", lambda account, **kw: fake_kc.delete(kw.get("service", "pdl"), account)
    )
    monkeypatch.setattr(
        macos, "get_keychain_token", lambda account, **kw: fake_kc.get(kw.get("service", "pdl"), account)
    )
    monkeypatch.setattr(
        macos, "has_keychain_token", lambda account, **kw: fake_kc.get(kw.get("service", "pdl"), account) is not None
    )

    ok = set_auth_token("hf", "my_hf_token")
    assert ok is True
    assert fake_kc.get("pdl", "hf") == "my_hf_token"
    status = get_auth_status()
    assert status["hf"]["configured"] is True

    rc = handle_auth_command("remove", "hf")
    assert rc == int(ExitCode.OK)
    assert fake_kc.get("pdl", "hf") is None

    rc_inv = handle_auth_command("set", "invalid_service")
    assert rc_inv == int(ExitCode.USAGE)

    rc_inv_rem = handle_auth_command("remove", "invalid_service")
    assert rc_inv_rem == int(ExitCode.USAGE)

    rc_inv_cmd = handle_auth_command("unknown_action")
    assert rc_inv_cmd == int(ExitCode.USAGE)

    # Error branches for full coverage
    assert set_auth_token("invalid_srv", "tok") is False
    assert set_auth_token("hf", "") is False

    monkeypatch.setattr(macos, "set_keychain_token", lambda *a, **k: False)
    assert set_auth_token("hf", "tok") is False
    monkeypatch.setattr("pdl.auth.set_auth_token", lambda *a, **k: False)
    assert handle_auth_command("set", "hf") == int(ExitCode.FAILURE)

    monkeypatch.setattr("pdl.auth.remove_auth_token", lambda *a, **k: False)
    assert handle_auth_command("remove", "hf") == int(ExitCode.FAILURE)

    # JSON mode coverage
    monkeypatch.setattr(UI, "json_mode", True)
    assert handle_auth_command("status") == int(ExitCode.OK)
    monkeypatch.setattr("pdl.auth.set_auth_token", lambda *a, **k: True)
    assert handle_auth_command("set", "hf") == int(ExitCode.OK)
    monkeypatch.setattr("pdl.auth.remove_auth_token", lambda *a, **k: True)
    assert handle_auth_command("remove", "hf") == int(ExitCode.OK)
