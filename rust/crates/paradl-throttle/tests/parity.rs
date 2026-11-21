//! Additional byte-exact values captured from the live Python oracle modules.

use paradl_throttle::budget::{DataBudgetTracker, clamp_connections, parse_byte_budget};
use paradl_throttle::classifier::mann_whitney_u_test;
use paradl_throttle::numeric::py_round;
use paradl_throttle::parsers::{
    parse_netstat_tcp, parse_nettop, parse_networksetup, parse_ping, parse_scutil_env,
    parse_traceroute,
};
use paradl_throttle::probes::analyze_burst_series;
use paradl_throttle::server::build_payload_block;

#[test]
fn python_round_is_bankers_rounding() {
    assert_eq!(py_round(2.5, 0), 2.0);
    assert_eq!(py_round(3.5, 0), 4.0);
    assert_eq!(py_round(2.675, 2), 2.67);
    assert_eq!(py_round(0.35 * 3.0, 2), 1.05);
    assert_eq!(py_round(1.05, 1), 1.1);
    assert_eq!(py_round(0.85, 1), 0.8);
}

#[test]
fn mann_whitney_matches_oracle() {
    let cases: &[(&[f64], &[f64], (f64, f64))] = &[
        (&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0], (0.0, 0.080_856)),
        (&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0], (4.5, 1.0)),
        (&[], &[1.0], (0.0, 1.0)),
        (
            &[0.0, 0.0, 0.0, 0.0],
            &[1.0, 1.0, 1.0, 1.0],
            (0.0, 0.013_124),
        ),
        (&[5.0, 7.5, 9.1, 2.2], &[3.3, 8.8, 1.1], (4.0, 0.595_883)),
    ];
    for (a, b, expected) in cases {
        assert_eq!(mann_whitney_u_test(a, b), *expected, "a={a:?} b={b:?}");
    }
}

#[test]
fn budget_parser_matches_oracle() {
    let default = 300 * 1024 * 1024;
    for (input, expected) in [
        (None, default),
        (Some("300MB"), default),
        (Some("3GB"), 3 * 1024 * 1024 * 1024),
        (Some("50M"), 50 * 1024 * 1024),
        (Some("1048576"), 1_048_576),
        (Some("1.5GB"), 1_610_612_736),
        (Some("2KiB"), 2048),
        (Some(""), default),
    ] {
        assert_eq!(
            parse_byte_budget(input, default).unwrap(),
            expected,
            "{input:?}"
        );
    }
    assert!(parse_byte_budget(Some("0"), default).is_err());
    assert!(parse_byte_budget(Some("-5"), default).is_err());
    assert!(parse_byte_budget(Some("12XB"), default).is_err());
}

#[test]
fn connection_clamp_matches_oracle() {
    for (n, allow_32, expected) in [
        (0, false, 1),
        (0, true, 1),
        (1, false, 1),
        (8, false, 8),
        (16, false, 16),
        (17, false, 16),
        (17, true, 17),
        (40, false, 16),
        (40, true, 32),
    ] {
        assert_eq!(clamp_connections(n, allow_32), expected);
    }
}

#[test]
fn seeded_jitter_matches_python_random() {
    let mut tracker = DataBudgetTracker::new(314_572_800, 42);
    let values: Vec<f64> = (0..5).map(|_| tracker.jitter_value(0.15, 0.45)).collect();
    assert_eq!(
        values,
        [
            0.3418280395373652,
            0.15750322656680008,
            0.23250879551073578,
            0.21696322144464683,
            0.37094136424920376,
        ]
    );
}

#[test]
fn burst_analysis_matches_oracle() {
    let result = analyze_burst_series(
        &[10e6; 4].into_iter().chain([2e6; 8]).collect::<Vec<_>>(),
        0.25,
    );
    assert_eq!(result.initial_burst_bps, 10_000_000.0);
    assert_eq!(result.burst_duration_sec, 1.0);
    assert_eq!(result.sustained_bps, 2_000_000.0);
    assert_eq!(result.burst_to_sustained_ratio, 5.0);
    assert_eq!(result.estimated_bucket_bytes, 8_000_000);
    assert_eq!(result.drop_shape, "step");

    let short = analyze_burst_series(&[10e6, 2e6], 0.25);
    assert_eq!(short.initial_burst_bps, 6_000_000.0);
    assert_eq!(short.burst_duration_sec, 0.5);
    assert_eq!(short.sustained_bps, 6_000_000.0);
    assert_eq!(short.burst_to_sustained_ratio, 1.0);
    assert_eq!(short.estimated_bucket_bytes, 0);
    assert_eq!(short.drop_shape, "flat");
}

#[test]
fn ping_parser_matches_oracle() {
    let ping = "PING example.com (1.1.1.1): 56 data bytes\n64 bytes from 1.1.1.1: seq=0 ttl=57 time=12.3 ms\n\n--- example.com ping statistics ---\n5 packets transmitted, 4 packets received, 20.0% packet loss\nround-trip min/avg/max/stddev = 10.1/12.3/15.5/1.2 ms\n";
    let result = parse_ping(ping);
    assert!(result.ok);
    assert_eq!(result.transmitted, 5);
    assert_eq!(result.received, 4);
    assert_eq!(result.loss_pct, 20.0);
    assert_eq!(result.rtt_min_ms, 10.1);
    assert_eq!(result.rtt_avg_ms, 12.3);
    assert_eq!(result.rtt_max_ms, 15.5);
    assert_eq!(result.jitter_ms, 1.2);
    assert!(!result.df_fragmentation_needed);

    let df = parse_ping(
        "Message too long\n5 packets transmitted, 0 packets received, 100.0% packet loss",
    );
    assert!(!df.ok);
    assert!(df.df_fragmentation_needed);
    assert_eq!(df.transmitted, 5);
}

#[test]
fn traceroute_parser_matches_oracle() {
    let trace = "traceroute to example.com (1.1.1.1), 64 hops max\n 1  192.168.1.1 (192.168.1.1)  1.2 ms  0.9 ms  0.8 ms\n 2  * * *\n 3  1.1.1.1 (1.1.1.1)  12.4 ms\n";
    let hops = parse_traceroute(trace);
    assert_eq!(hops.len(), 3);
    assert_eq!(hops[0].hop, 1);
    assert_eq!(hops[0].ip, "192.168.1.1");
    assert_eq!(hops[0].rtt_ms, Some(1.2));
    assert_eq!(hops[1].ip, "*");
    assert_eq!(hops[1].rtt_ms, None);
    assert_eq!(hops[2].rtt_ms, Some(12.4));
}

#[test]
fn netstat_parser_matches_oracle() {
    let text = "TcpExt:\n    1000 packets sent\n    900 data packets (450000 bytes)\n    90 data packets (45000 bytes) retransmitted\n    3 connection resets received\n";
    let r = parse_netstat_tcp(text);
    assert_eq!(r.packets_sent, 1000);
    assert_eq!(r.data_packets, 900);
    assert_eq!(r.data_bytes, 450_000);
    assert_eq!(r.retransmitted_packets, 90);
    assert_eq!(r.retransmitted_bytes, 45_000);
    assert_eq!(r.retransmit_rate_pct, 9.0);
    assert_eq!(r.connection_resets, 3);
}

#[test]
fn nettop_parser_matches_oracle() {
    let text = "Process  Interface  State  Bytes_In  Bytes_Out\nChrome  en0  Established  1000  2000\nsafari,en0,Established,3000,4000\nskip bytes_in line\n";
    let procs = parse_nettop(text);
    assert_eq!(procs.len(), 2);
    assert_eq!(procs[0].process, "Chrome");
    assert_eq!(procs[0].interface, "en0");
    assert_eq!(procs[0].bytes_in, 1000);
    assert_eq!(procs[0].bytes_out, 2000);
    assert_eq!(procs[1].process, "safari");
    assert_eq!(procs[1].bytes_in, 3000);
}

#[test]
fn networksetup_and_scutil_parsers_match_oracle() {
    let net = parse_networksetup("Current Wi-Fi Network: MyNet\nlastTxRate: 866");
    assert_eq!(net.ssid, "MyNet");
    assert!(net.wifi_associated);
    assert_eq!(net.wifi_link_mbps, Some(866.0));

    let sc = parse_scutil_env(
        "PrimaryInterface : en0\nSSID_STR : MyNet\n<interface: utun3>\nlink rate: 500",
    );
    assert_eq!(sc.primary_interface, "en0");
    assert_eq!(sc.ssid, "MyNet");
    assert_eq!(sc.interface_type, "Wi-Fi");
    assert_eq!(sc.tunnel_interfaces, vec!["utun3"]);
    assert!(sc.vpn_or_tunnel_active);
    assert_eq!(sc.wifi_link_mbps, Some(500.0));
}

#[test]
fn dns_txt_query_and_locality_match_oracle() {
    use paradl_throttle::targets::{build_dns_txt_query, classify_locality, loopback_asn};
    let query = build_dns_txt_query("1.0.0.127.origin.asn.cymru.com");
    assert_eq!(
        query.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        "42420100000100000000000001310130013003313237066f726967696e0361736e0563796d727503636f6d0000100001"
    );
    assert_eq!(
        classify_locality("global-cdn", "https://x.example/a", "US", "BD"),
        "international"
    );
    assert_eq!(
        classify_locality("domestic", "https://x.example/a", "US", "BD"),
        "domestic"
    );
    assert_eq!(
        classify_locality("isp-cached", "https://x.example/a", "US", "BD"),
        "isp-cached"
    );
    assert_eq!(
        loopback_asn("domestic:bdix"),
        ("AS58717", "BD", "BDIX-Domestic-Sim")
    );
}

#[test]
fn payload_block_is_byte_deterministic() {
    for (kind, expected_ctype) in [
        ("random", "application/octet-stream"),
        ("video", "video/mp4"),
        ("file", "application/zip"),
        ("bogus", "application/octet-stream"),
    ] {
        let (block, ctype) = build_payload_block(kind, 65_536);
        assert_eq!(block.len(), 65_536);
        assert_eq!(ctype, expected_ctype);
        let (again, _) = build_payload_block(kind, 65_536);
        assert_eq!(block, again);
    }
}
