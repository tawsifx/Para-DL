"""The backward-compatible paradl.config shim must export the schema version."""

from __future__ import annotations


def test_config_shim_reexports_schema_version():
    from paradl.config import CONFIG_SCHEMA_VERSION

    assert CONFIG_SCHEMA_VERSION == 2
