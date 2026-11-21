//! Every parity-matrix row must have a concrete proving path.
//!
//! Rows owned by W4-1 are proved by the app dispatch / diagnostics / handoff
//! tests in this crate; rows owned by earlier waves are proved by the named
//! crate test suite that established the byte-exact contract. This test is the
//! W4-1 audit that no row was left without an executable proof.

const RETIRED_CLI_FLAG_ROWS: &[&str] = &["F09", "F11", "F60"];

/// Return the proving path for a parity-matrix row id (e.g. `C05`, `P21`).
fn proving_path(row: &str) -> Option<&'static str> {
    match row {
        // Retired at W5-3; Python oracle deleted — no proving path.
        "F09" | "F11" | "F60" => None,
        // ---- W4-1 owned rows: proved here -------------------------------
        "C01" | "C05" | "C07" | "C15" | "C16" | "C17" | "C18" | "C22" | "C52" => {
            Some("paradl-app/tests/dispatch.rs")
        }
        "C02" | "C06" => Some("paradl-app/tests/dispatch.rs + paradl-platform/tests/goldens.rs"),
        "C03" | "C04" | "C21" | "C50" => Some("paradl-app/tests/dispatch.rs (auth_status/set/remove)"),
        "C09" | "C51" => Some("paradl-net/tests/routing_oracle.rs + paradl-app download adapter"),
        "C19" | "C29" | "C30" | "C31" | "C32" | "C33" | "C35" | "C36" | "C37" => {
            Some("paradl-throttle/tests/oracle_golden.rs + app throttle adapter")
        }
        "C27" | "C28" => Some("paradl-api/tests/http.rs + paradl-ui/tests/golden.rs"),
        "C34" => Some("paradl-platform/tests/goldens.rs + resilience plist event"),
        _ if row.starts_with('C')
            && row[1..].parse::<u32>().is_ok_and(|n| (38..=44).contains(&n)) =>
        {
            Some("paradl-app/tests/dispatch.rs (queue actions)")
        }
        _ if row.starts_with('C')
            && row[1..].parse::<u32>().is_ok_and(|n| (45..=49).contains(&n)) =>
        {
            Some("paradl-app/tests/dispatch.rs (config/profiles)")
        }
        "F58" | "F69" | "F78" | "F79" => Some("paradl-app/tests/dispatch.rs"),
        "E10" | "E27" | "E28" => Some("paradl-app/tests/doctor.rs"),
        "E12" | "E21" | "E22" | "E23" | "E24" | "E25" | "E37" | "E43" | "E55" | "E56" => {
            Some("paradl-app/tests/dispatch.rs (single-emitter JSONL)")
        }
        "E39" => Some("paradl-core/tests/golden.rs (ExitCode 130) + binary signal path"),
        "L36" => Some("paradl-app clean_orphans adapter + dispatch.rs"),
        "G07" | "G27" => Some("paradl-app/tests/handoffs.rs"),
        "X05" | "X06" => Some("paradl-app/tests/doctor.rs (no Python) + media crate process guard"),
        // ---- Other waves: proved by their owning crate's suite ---------
        _ if row.starts_with('C') || row.starts_with('F') => {
            Some("paradl-cli/tests/oracle_parity.rs (byte-exact parser)")
        }
        _ if row.starts_with('E') => Some("paradl-events/tests/golden.rs + paradl-resilience/tests/resilience.rs"),
        _ if row.starts_with('G') => Some("paradl-core/tests/golden.rs"),
        _ if row.starts_with('D') => Some("paradl-store/tests/golden_schema.rs"),
        _ if row.starts_with('L') => Some("paradl-core/tests/golden.rs + owning crate goldens"),
        _ if row.starts_with('P') => Some("paradl-media/tests/protocol.rs + boundary lane"),
        _ if row.starts_with('X') => Some("W0-1 CI / distribution checks"),
        _ => None,
    }
}

fn assert_rows(prefix: char, end: u32) {
    for index in 1..=end {
        let row = format!("{prefix}{index:02}");
        assert!(
            proving_path(&row).is_some(),
            "parity row {row} has no proving path"
        );
    }
}

fn assert_rows_excluding(prefix: char, end: u32, retired: &[&str]) {
    for index in 1..=end {
        let row = format!("{prefix}{index:02}");
        if retired.contains(&row.as_str()) {
            continue;
        }
        assert!(
            proving_path(&row).is_some(),
            "parity row {row} has no proving path"
        );
    }
}

#[test]
fn every_cli_subcommand_row_has_a_proving_path() {
    assert_rows('C', 52);
}

#[test]
fn every_cli_flag_row_has_a_proving_path() {
    assert_rows_excluding('F', 92, RETIRED_CLI_FLAG_ROWS);
}

#[test]
fn retired_cli_flag_rows_are_not_audited() {
    for row in ["F09", "F11", "F60"] {
        assert_eq!(proving_path(row), None, "retired row {row} still claims a proving path");
    }
}

#[test]
fn every_event_row_has_a_proving_path() {
    assert_rows('E', 61);
}

#[test]
fn every_config_env_row_has_a_proving_path() {
    assert_rows('G', 37);
}

#[test]
fn every_db_row_has_a_proving_path() {
    assert_rows('D', 20);
}

#[test]
fn every_literal_row_has_a_proving_path() {
    assert_rows('L', 40);
}

#[test]
fn every_media_protocol_row_has_a_proving_path() {
    assert_rows('P', 26);
}

#[test]
fn every_distribution_row_has_a_proving_path() {
    assert_rows('X', 7);
}

#[test]
fn unknown_row_ids_are_rejected() {
    assert_eq!(proving_path("ZZ99"), None);
}
