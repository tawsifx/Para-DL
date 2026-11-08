"""Tests for dynamic ISP auto-detection, fallback tiers, and configuration resolution."""

from __future__ import annotations

import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

from pdl.config import PDLConfig
from pdl.net import (
    clean_isp_name,
    detect_isp,
    reset_isp_detector,
    set_isp_detector,
)
from pdl.throttle.cli import (
    build_throttle_parser,
    generate_throttle_launchd_plist,
    run_throttle_cli,
)
from pdl.throttle.models import (
    ClassificationResult,
    MeasurementDataset,
    ThrottleReport,
)


class TestISPDetection(unittest.TestCase):
    def tearDown(self) -> None:
        reset_isp_detector()

    def test_clean_isp_name(self) -> None:
        """Verify cleaning and normalization of raw ISP / ASN names."""
        self.assertEqual(clean_isp_name(""), "")
        self.assertEqual(clean_isp_name("   "), "")
        self.assertEqual(clean_isp_name("AS149048 Duranta Online Limited"), "Duranta Online Limited")
        self.assertEqual(clean_isp_name("AS13335 - Cloudflare, Inc."), "Cloudflare, Inc.")
        self.assertEqual(clean_isp_name("AS7922: Comcast Cable Communications"), "Comcast Cable Communications")
        self.assertEqual(clean_isp_name('"Starlink"'), "Starlink")
        self.assertEqual(clean_isp_name("'Vodafone UK'"), "Vodafone UK")

    def test_detect_isp_custom_detector(self) -> None:
        """Verify injectable custom detector takes precedence for tests."""
        set_isp_detector(lambda: "Mock Satellite Broadband")
        self.assertEqual(detect_isp(force_refresh=True), "Mock Satellite Broadband")

    def test_detect_isp_tier1_ip_api(self) -> None:
        """Verify tier 1 (ip-api.com) success returns cleaned ISP."""
        mock_resp = MagicMock()
        mock_resp.status_code = 200
        mock_resp.json.return_value = {
            "status": "success",
            "isp": "Apex Broadband",
            "org": "Apex Networks Ltd",
            "as": "AS12345 Apex Networks",
        }

        with patch("httpx.Client") as mock_client_cls:
            mock_client = MagicMock()
            mock_client.get.return_value = mock_resp
            mock_client_cls.return_value.__enter__.return_value = mock_client

            isp = detect_isp(force_refresh=True)
            self.assertEqual(isp, "Apex Broadband")

    def test_detect_isp_tier2_ipinfo_fallback(self) -> None:
        """Verify tier 2 (ipinfo.io) fallback when ip-api fails."""
        resp_tier1 = MagicMock()
        resp_tier1.status_code = 500

        resp_tier2 = MagicMock()
        resp_tier2.status_code = 200
        resp_tier2.json.return_value = {
            "ip": "1.2.3.4",
            "org": "AS54321 Global Transit ISP",
        }

        def side_effect(url: str, **kwargs):
            if "ip-api" in url:
                return resp_tier1
            if "ipinfo" in url:
                return resp_tier2
            raise RuntimeError("Unexpected URL")

        with patch("httpx.Client") as mock_client_cls:
            mock_client = MagicMock()
            mock_client.get.side_effect = side_effect
            mock_client_cls.return_value.__enter__.return_value = mock_client

            isp = detect_isp(force_refresh=True)
            self.assertEqual(isp, "Global Transit ISP")

    def test_detect_isp_tier3_trace_rdap_fallback(self) -> None:
        """Verify tier 3 (Cloudflare trace + RDAP) when tier 1 & 2 fail."""
        resp_trace = MagicMock()
        resp_trace.status_code = 200
        resp_trace.text = "fl=123\nip=203.0.113.5\nloc=SG\n"

        resp_rdap = MagicMock()
        resp_rdap.status_code = 200
        resp_rdap.json.return_value = {
            "name": "METRO-FIBER-SG",
            "entities": [
                {
                    "vcardArray": [
                        "vcard",
                        [
                            ["version", {}, "text", "4.0"],
                            ["fn", {}, "text", "Metro Fiber Telecom"],
                        ],
                    ]
                }
            ],
        }

        def side_effect(url: str, **kwargs):
            if "ip-api" in url or "ipinfo" in url:
                raise TimeoutError("Network timeout")
            if "1.1.1.1/cdn-cgi/trace" in url:
                return resp_trace
            if "rdap.org/ip" in url:
                return resp_rdap
            raise RuntimeError("Unexpected URL")

        with patch("httpx.Client") as mock_client_cls:
            mock_client = MagicMock()
            mock_client.get.side_effect = side_effect
            mock_client_cls.return_value.__enter__.return_value = mock_client

            isp = detect_isp(force_refresh=True)
            self.assertEqual(isp, "Metro Fiber Telecom")

    def test_detect_isp_all_fail_fallback_unknown(self) -> None:
        """Verify graceful fallback to 'Unknown ISP' when all network queries fail."""
        with patch("httpx.Client") as mock_client_cls:
            mock_client = MagicMock()
            mock_client.get.side_effect = ConnectionError("No internet connection")
            mock_client_cls.return_value.__enter__.return_value = mock_client

            isp = detect_isp(force_refresh=True)
            self.assertEqual(isp, "Unknown ISP")

    def test_config_isp_defaults_to_auto(self) -> None:
        """Verify PDLConfig initializes with isp='auto'."""
        cfg = PDLConfig()
        self.assertEqual(cfg.isp, "auto")

    def test_config_get_effective_isp(self) -> None:
        """Verify config.get_effective_isp() uses auto-detection or explicit value."""
        set_isp_detector(lambda: "Detected Fiber Net")

        cfg_auto = PDLConfig(isp="auto")
        self.assertEqual(cfg_auto.get_effective_isp(), "Detected Fiber Net")

        cfg_custom = PDLConfig(isp="Enterprise Leased Line")
        self.assertEqual(cfg_custom.get_effective_isp(), "Enterprise Leased Line")

    def test_throttle_models_default_isp(self) -> None:
        """Verify throttle models default to 'Unknown ISP' instead of a hardcoded personal ISP."""
        ds = MeasurementDataset()
        self.assertEqual(ds.isp, "Unknown ISP")

        cr = ClassificationResult()
        self.assertEqual(cr.isp_evidence.isp, "Unknown ISP")

        rep = ThrottleReport()
        self.assertEqual(rep.isp, "Unknown ISP")

    def test_throttle_cli_resolves_auto_detected_isp(self) -> None:
        """Verify pdl throttle subcommand resolves ISP via auto-detection when unsupplied."""
        set_isp_detector(lambda: "Auto Telecom 5G")
        cfg = PDLConfig(isp="auto")

        with patch("pdl.throttle.cli._handle_targets_cmd", return_value=0) as mock_cmd:
            ret = run_throttle_cli(["targets"], cfg)
            self.assertEqual(ret, 0)
            self.assertTrue(mock_cmd.called)
            args = mock_cmd.call_args[0][0]
            self.assertEqual(args.isp, "Auto Telecom 5G")

    def test_throttle_cli_explicit_isp_flag_overrides(self) -> None:
        """Verify --isp flag overrides both config and auto-detection."""
        set_isp_detector(lambda: "Auto Telecom 5G")
        cfg = PDLConfig(isp="auto")

        with patch("pdl.throttle.cli._handle_targets_cmd", return_value=0) as mock_cmd:
            ret = run_throttle_cli(["targets", "--isp", "Special Satellite"], cfg)
            self.assertEqual(ret, 0)
            args = mock_cmd.call_args[0][0]
            self.assertEqual(args.isp, "Special Satellite")

    def test_generate_throttle_launchd_plist_no_hardcoded_isp(self, tmp_path_factory=None) -> None:
        """Verify launchd plist generator does not hardcode personal ISP when auto/unset."""
        import tempfile
        with tempfile.TemporaryDirectory() as tmp_dir:
            out_file = Path(tmp_dir) / "test.plist"
            generate_throttle_launchd_plist(out_file, isp="auto")
            content = out_file.read_text(encoding="utf-8")
            self.assertNotIn("Duronto", content)
            self.assertNotIn("--isp", content)


if __name__ == "__main__":
    unittest.main()
