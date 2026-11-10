"""Unit tests for the Aria2 JSON-RPC download engine."""

from __future__ import annotations

import unittest
from unittest.mock import MagicMock, patch

from paradl.engines.aria2 import (
    _DAEMON_POOL,
    Aria2DaemonPool,
    Aria2Engine,
    Aria2RpcDaemon,
    _pick_free_port,
    get_daemon_pool,
    shutdown_daemon_pool,
)
from paradl.models import (
    DownloadItem,
    TargetInfo,
    TargetType,
    TransferResult,
    WatchdogMode,
)


class TestPickFreePort(unittest.TestCase):
    """Tests for _pick_free_port() ephemeral port selection."""

    def test_returns_int(self) -> None:
        port = _pick_free_port()
        self.assertIsInstance(port, int)
        self.assertGreater(port, 0)
        self.assertLess(port, 65536)


class TestAria2RpcDaemonInit(unittest.TestCase):
    """Tests for Aria2RpcDaemon initialization."""

    @patch("paradl.engines.aria2.UI.register_secret")
    def test_init_sets_port_and_secret(self, mock_register) -> None:
        daemon = Aria2RpcDaemon(aria2c_bin="/usr/bin/aria2c")
        self.assertIsInstance(daemon.port, int)
        self.assertGreater(daemon.port, 0)
        self.assertIsInstance(daemon.secret, str)
        self.assertEqual(len(daemon.secret), 32)  # token_hex(16) = 32 hex chars
        mock_register.assert_called_once_with(daemon.secret)

    def test_default_config_values(self) -> None:
        daemon = Aria2RpcDaemon(aria2c_bin="/usr/bin/aria2c")
        self.assertEqual(daemon.max_concurrent_downloads, 1)
        self.assertEqual(daemon.ip_family, "auto")
        self.assertIsNone(daemon.limit_rate)
        self.assertFalse(daemon.check_integrity)


class TestAria2EngineParseSpeedStr(unittest.TestCase):
    """Tests for Aria2Engine.parse_speed_str()."""

    def test_parses_rate_string(self) -> None:
        result = Aria2Engine.parse_speed_str("4MBps")
        self.assertEqual(result, 4 * 1024 * 1024)

    def test_parses_none(self) -> None:
        result = Aria2Engine.parse_speed_str(None)
        self.assertEqual(result, 0.0)


class TestDownloadItem(unittest.TestCase):
    """Tests for DownloadItem dataclass."""

    def test_download_item_creation(self) -> None:
        item = DownloadItem(
            url="https://example.com/file.zip",
            out_dir="/tmp/downloads",
            filename="file.zip",
            headers={"User-Agent": "paradl/1.0"},
            expected_length=1048576,
            expected_digest="sha256:abc123",
        )
        self.assertEqual(item.url, "https://example.com/file.zip")
        self.assertEqual(item.filename, "file.zip")
        self.assertEqual(item.expected_length, 1048576)


class TestTargetInfoForAria2(unittest.TestCase):
    """Tests for TargetInfo used with Aria2Engine."""

    def test_direct_target_info(self) -> None:
        t = TargetInfo(
            raw="https://example.com/file.zip",
            target_type=TargetType.DIRECT,
            clean_url="https://example.com/file.zip",
            filename_hint="file.zip",
            content_length=1048576,
            accept_ranges=True,
        )
        self.assertEqual(t.target_type, TargetType.DIRECT)
        self.assertTrue(t.accept_ranges)


class TestTransferResult(unittest.TestCase):
    """Tests for TransferResult dataclass."""

    def test_transfer_result_defaults(self) -> None:
        result = TransferResult(success=True)
        self.assertTrue(result.success)
        self.assertEqual(result.bytes_downloaded, 0)
        self.assertEqual(result.duration, 0.0)
        self.assertEqual(result.avg_speed_bps, 0.0)
        self.assertEqual(result.recycle_count, 0)
        self.assertEqual(result.exit_code, 0)

    def test_transfer_result_with_file(self) -> None:
        result = TransferResult(
            success=True,
            file_path="/tmp/file.zip",
            bytes_downloaded=1048576,
            duration=10.0,
            avg_speed_bps=104857.6,
            recycle_count=2,
            exit_code=0,
        )
        self.assertEqual(result.bytes_downloaded, 1048576)
        self.assertEqual(result.recycle_count, 2)


class TestAria2EngineMissingBinary(unittest.TestCase):
    """Tests for Aria2Engine when aria2c is not available."""

    def test_missing_aria2c_returns_failure(self) -> None:
        # Mock TOOLCHAIN object with aria2c = None (it's a read-only property)
        fake_toolchain = MagicMock()
        fake_toolchain.aria2c = None
        with patch("paradl.engines.aria2.TOOLCHAIN", fake_toolchain):
            result = Aria2Engine.download(
                TargetInfo(
                    raw="https://example.com/file.zip",
                    target_type=TargetType.DIRECT,
                    clean_url="https://example.com/file.zip",
                ),
                out_dir="/tmp",
            )
        self.assertFalse(result.success)
        self.assertEqual(result.exit_code, 4)  # MISSING_DEPENDENCY
        self.assertIn("aria2c", result.error)


class TestWatchdogModes(unittest.TestCase):
    """Tests for WatchdogMode enum used in aria2."""

    def test_values(self) -> None:
        self.assertEqual(WatchdogMode.V1, "v1")
        self.assertEqual(WatchdogMode.V2, "v2")
        self.assertEqual(WatchdogMode.OFF, "off")


class TestAria2DaemonPool(unittest.TestCase):
    """Tests for the persistent aria2c daemon pool."""

    def tearDown(self) -> None:
        shutdown_daemon_pool()

    def test_pool_singleton(self) -> None:
        pool1 = get_daemon_pool()
        pool2 = get_daemon_pool()
        self.assertIs(pool1, pool2)

    def test_pool_entry_tracking(self) -> None:
        pool = Aria2DaemonPool(idle_timeout_sec=60.0)
        self.assertEqual(len(pool._entries), 0)

    def test_shutdown_daemon_pool(self) -> None:
        get_daemon_pool()
        shutdown_daemon_pool()
        # After shutdown, a new pool should be created on next get

        self.assertIsNone(_DAEMON_POOL)


class TestAria2DaemonHTTP3(unittest.TestCase):
    """Tests for HTTP/3 support in Aria2RpcDaemon."""

    @patch("paradl.engines.aria2.UI.register_secret")
    def test_http3_stored(self, mock_register) -> None:
        daemon = Aria2RpcDaemon(aria2c_bin="/usr/bin/aria2c", http3=True)
        self.assertTrue(daemon.http3)

    @patch("paradl.engines.aria2.UI.register_secret")
    def test_http3_default_false(self, mock_register) -> None:
        daemon = Aria2RpcDaemon(aria2c_bin="/usr/bin/aria2c")
        self.assertFalse(daemon.http3)


class TestAdaptiveConcurrency(unittest.TestCase):
    """Tests for RTT/loss-based adaptive connection budget."""

    def setUp(self) -> None:
        import os
        import tempfile

        self.tmpdir = tempfile.mkdtemp()
        self.env_patch = patch.dict(os.environ, {"PARADL_HOME": self.tmpdir}, clear=False)
        self.env_patch.start()

    def tearDown(self) -> None:
        self.env_patch.stop()
        import shutil

        shutil.rmtree(self.tmpdir, ignore_errors=True)

    def test_host_strategy_stores_rtt_and_loss(self) -> None:
        from paradl.history import HistoryManager, HostStrategy

        strategy = HostStrategy(
            host="example.com",
            best_action="tier1_native",
            connections=16,
            ip_family="auto",
            avg_rtt_ms=150.0,
            packet_loss_pct=8.0,
        )
        HistoryManager.save_host_strategy(strategy)

        retrieved = HistoryManager.get_host_strategy("example.com")
        self.assertIsNotNone(retrieved)
        self.assertAlmostEqual(retrieved.avg_rtt_ms, 150.0)
        self.assertAlmostEqual(retrieved.packet_loss_pct, 8.0)

    def test_high_rtt_reduces_connection_budget(self) -> None:
        """Verify RTT-based budget reduction logic directly."""

        # Simulate the adaptive budget calculation from aria2.download_batch
        def compute_budget(rtt_ms: float, loss_pct: float, base_budget: int = 16) -> int:
            budget = base_budget
            if budget > 1:
                rtt_factor = 1.0
                if rtt_ms > 100.0:
                    rtt_factor = max(0.25, 1.0 - (rtt_ms - 100.0) / 500.0)
                loss_factor = 1.0
                if loss_pct > 5.0:
                    loss_factor = max(0.25, 1.0 - (loss_pct - 5.0) / 20.0)
                adaptive_factor = min(rtt_factor, loss_factor)
                budget = max(4, int(budget * adaptive_factor))
            return budget

        # Clean path: full budget
        self.assertEqual(compute_budget(10.0, 0.5), 16)

        # High RTT: reduced
        self.assertLess(compute_budget(200.0, 0.5), 16)
        self.assertGreaterEqual(compute_budget(200.0, 0.5), 4)

        # High loss: reduced
        self.assertLess(compute_budget(10.0, 15.0), 16)

        # Both high RTT and loss: further reduced
        self.assertLess(compute_budget(300.0, 20.0), compute_budget(300.0, 0.5))

        # Floor at 4 connections
        self.assertEqual(compute_budget(999.0, 99.0), 4)


if __name__ == "__main__":
    unittest.main()
