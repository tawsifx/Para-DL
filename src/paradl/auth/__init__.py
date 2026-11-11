"""Authentication layer for pdl — token management for GitHub, Hugging Face, and other services.

Provides:
- handle_auth_command(): CLI entry for 'pdl auth <action> [service] [token]'.
- is_windows(): platform check (re-exported for backward compatibility).
- Secure keychain-backed token storage via the platform abstraction.

Used by the install flow for private GitHub releases and authenticated registries.
"""

from __future__ import annotations

from paradl.auth.manager import (
    get_auth_status,
    handle_auth_command,
    is_windows,
    remove_auth_token,
    set_auth_token,
)
from paradl.platform import (
    delete_keychain_token,
    get_keychain_token,
    has_keychain_token,
    set_keychain_token,
)

__all__ = [
    "delete_keychain_token",
    "get_auth_status",
    "get_keychain_token",
    "handle_auth_command",
    "has_keychain_token",
    "is_windows",
    "remove_auth_token",
    "set_auth_token",
    "set_keychain_token",
]
