"""Tests for Windows platform subsystem (paradl.platform.windows)."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

from paradl.platform import windows


class TestWindowsPlatform(unittest.TestCase):
    def test_sleep_assertion_kernel32(self) -> None:
        """Verify WindowsSleepAssertion calls SetThreadExecutionState on kernel32."""
        mock_kernel32 = MagicMock()
        mock_kernel32.SetThreadExecutionState.return_value = 0x80000001

        with patch.object(windows, "get_kernel32", return_value=mock_kernel32):
            with windows.WindowsSleepAssertion(enabled=True):
                mock_kernel32.SetThreadExecutionState.assert_called_with(
                    windows.ES_CONTINUOUS | windows.ES_SYSTEM_REQUIRED
                )
            # On exit, it resets to ES_CONTINUOUS
            mock_kernel32.SetThreadExecutionState.assert_called_with(windows.ES_CONTINUOUS)

    def test_sleep_assertion_disabled_or_unavailable(self) -> None:
        """Verify WindowsSleepAssertion is safe when disabled or kernel32 is None."""
        with windows.WindowsSleepAssertion(enabled=False):
            pass

        with (
            patch.object(windows, "get_kernel32", return_value=None),
            windows.WindowsSleepAssertion(enabled=True),
        ):
            pass

    def test_parse_netsh_wlan(self) -> None:
        """Verify parsing SSID from netsh wlan show interfaces output."""
        sample_output = """
There is 1 interface on the system:

    Name                   : Wi-Fi
    Description            : Intel(R) Wi-Fi 6 AX201 160MHz
    GUID                   : a1b2c3d4-e5f6-7890-abcd-ef0123456789
    State                  : connected
    SSID                   : FiberMesh_5G
    BSSID                  : 00:11:22:33:44:55
    Network type           : Infrastructure
    Radio type             : 802.11ax
    Authentication         : WPA2-Personal
    Cipher                 : CCMP
    Connection mode        : Profile
    Channel                : 36
    Receive rate (Mbps)    : 1201
    Transmit rate (Mbps)   : 1201
    Signal                 : 95%
"""
        ssid = windows.parse_netsh_wlan_ssid(sample_output)
        self.assertEqual(ssid, "FiberMesh_5G")

    def test_get_power_status_simulated(self) -> None:
        """Verify decoding SYSTEM_POWER_STATUS struct."""
        mock_status = windows.SYSTEM_POWER_STATUS()
        mock_status.ACLineStatus = 1  # Online (AC)
        mock_status.BatteryFlag = 0
        mock_status.BatteryLifePercent = 88
        mock_status.SystemStatusFlag = 0  # Battery Saver off

        mock_kernel32 = MagicMock()

        def fake_get_power(byref_arg):
            byref_arg._obj.ACLineStatus = 1
            byref_arg._obj.BatteryLifePercent = 88
            byref_arg._obj.SystemStatusFlag = 0
            return 1

        mock_kernel32.GetSystemPowerStatus.side_effect = fake_get_power

        with patch.object(windows, "get_kernel32", return_value=mock_kernel32):
            status = windows.get_power_status()
            self.assertTrue(status.on_ac_power)
            self.assertEqual(status.battery_percent, 88)
            self.assertFalse(status.low_power_mode)
            self.assertEqual(status.raw_source, "AC Power")

    def test_zone_identifier_ads_handling(self) -> None:
        """Verify Zone.Identifier reading, writing, and quarantine clearing."""
        with tempfile.TemporaryDirectory() as tmp_dir:
            fpath = Path(tmp_dir) / "sample_binary.exe"
            fpath.write_bytes(b"MZtestbinary")

            # We test the parser/format helper functions directly to be cross-platform
            parsed_initial = windows.parse_zone_identifier_text(
                "[ZoneTransfer]\nZoneId=3\nHostUrl=https://example.com/tool.exe\n"
            )
            self.assertEqual(parsed_initial.get("ZoneId"), "3")
            self.assertEqual(parsed_initial.get("HostUrl"), "https://example.com/tool.exe")

            # Unquarantining strips ZoneId but preserves HostUrl
            updated_ini = windows.unquarantine_zone_identifier_text(
                "[ZoneTransfer]\nZoneId=3\nHostUrl=https://example.com/tool.exe\n"
            )
            self.assertNotIn("ZoneId=3", updated_ini)
            self.assertIn("HostUrl=https://example.com/tool.exe", updated_ini)

            # Setting origin URL generates valid INI
            generated = windows.format_zone_identifier_text(
                "https://github.com/astral-sh/uv", zone_id=None
            )
            self.assertIn("HostUrl=https://github.com/astral-sh/uv", generated)

    def test_get_power_status_desktop_unknown_battery(self) -> None:
        """Verify SYSTEM_POWER_STATUS with BatteryLifePercent=255 produces battery_percent=None, not -1."""
        mock_kernel32 = MagicMock()

        def fake_get_power(byref_arg):
            byref_arg._obj.ACLineStatus = 1
            byref_arg._obj.BatteryLifePercent = 255  # Unknown on AC desktop
            byref_arg._obj.SystemStatusFlag = 0
            return 1

        mock_kernel32.GetSystemPowerStatus.side_effect = fake_get_power

        with patch.object(windows, "get_kernel32", return_value=mock_kernel32):
            status = windows.get_power_status()
            self.assertTrue(status.on_ac_power)
            self.assertIsNone(status.battery_percent)

    def test_windows_network_watcher_interface(self) -> None:
        """Verify WindowsNetworkWatcher implements poll_notifications, inject_notification, and backend."""
        watcher = windows.WindowsNetworkWatcher(on_change=lambda d: None)
        self.assertEqual(watcher.backend, "windows_netsh")
        self.assertEqual(watcher.poll_notifications(), [])

        watcher.inject_notification("network changed")
        self.assertEqual(watcher.poll_notifications(), ["network changed"])
        # Second poll should be empty (drained)
        self.assertEqual(watcher.poll_notifications(), [])

    def test_windows_credential_roundtrip(self) -> None:
        """Verify Windows credential set, get, has, and delete roundtrip."""
        with tempfile.TemporaryDirectory() as tmp_dir:
            store_file = Path(tmp_dir) / "credentials.json"
            with (
                patch.object(windows, "_get_dpapi_store_path", return_value=store_file),
                patch.object(windows, "get_advapi32", return_value=None),
            ):
                self.assertTrue(windows.set_keychain_token("hf", "hf_test_secret_token_123"))
                self.assertTrue(windows.has_keychain_token("hf"))
                self.assertEqual(windows.get_keychain_token("hf"), "hf_test_secret_token_123")
                self.assertTrue(windows.delete_keychain_token("hf"))
                self.assertFalse(windows.has_keychain_token("hf"))
                self.assertIsNone(windows.get_keychain_token("hf"))

    def test_check_windows_utilities(self) -> None:
        """Verify check_windows_utilities audits powershell, curl, tar, winget."""
        with patch(
            "shutil.which",
            side_effect=lambda name: (
                f"C:\\Windows\\System32\\{name}.exe"
                if name in ("powershell", "curl", "tar")
                else None
            ),
        ):
            res = windows.check_windows_utilities()
            self.assertTrue(res["powershell"]["available"])
            self.assertTrue(res["curl"]["available"])
            self.assertTrue(res["tar"]["available"])
            self.assertFalse(res["winget"]["available"])
            self.assertIn("winget", res["winget"]["fix"])


if __name__ == "__main__":
    unittest.main()
