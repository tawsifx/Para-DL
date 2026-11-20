//! Golden assertions for the `.pdl.part` / `.pdl.json` sidecar byte layout and rapids
//! planner/hash constants from `rust/rapids/src/tests.rs`.

use crate::fixtures::fixture;

#[test]
fn golden_sidecar_names_and_version() {
    let g = fixture("sidecar.json");
    assert_eq!(g["part_suffix"], ".pdl.part");
    assert_eq!(g["state_suffix"], ".pdl.json");
    assert_eq!(g["state_version"].as_i64().unwrap(), 1);
}

#[test]
fn golden_sidecar_pretty_json_layout() {
    let g = fixture("sidecar.json");
    let template = g["sidecar_pretty_json"].as_str().unwrap();
    // A future engine must emit exactly this byte layout (2-space pretty JSON, field order
    // as declared in rapids::state::ResumeState) when the dest path is substituted.
    let fixed_dest = "/tmp/pdl-oracle/a.bin";
    let rendered = template.replace("<DEST>", "/tmp/pdl-oracle");
    let expected = format!(
        "{{\n  \"version\": 1,\n  \"url\": \"http://x/a.bin\",\n  \"final_url\": \"http://x/a.bin\",\n  \"dest\": \"{fixed_dest}\",\n  \"size\": 100,\n  \"validators\": {{\n    \"etag\": \"\\\"e\\\"\",\n    \"etag_host\": \"x\",\n    \"last_modified\": null\n  }},\n  \"segments\": [\n    {{\n      \"start\": 0,\n      \"cursor\": 10,\n      \"end\": 50\n    }},\n    {{\n      \"start\": 50,\n      \"cursor\": 60,\n      \"end\": 100\n    }}\n  ],\n  \"content_type\": null,\n  \"checksum\": null\n}}"
    );
    assert_eq!(rendered, expected);
    // No request headers / credentials are ever stored in the sidecar.
    assert!(!rendered.contains("cookie"));
    assert!(!rendered.contains("authorization"));
}

#[test]
fn golden_validators_and_planner_numbers() {
    let g = fixture("sidecar.json");
    assert_eq!(g["validators"]["etag"], "\"e\"");
    assert_eq!(g["validators"]["etag_host"], "x");
    assert!(g["validators"]["last_modified"].is_null());
    let p = &g["planner"];
    let mb = 1024u64 * 1024;
    assert_eq!(p["from0"].as_u64().unwrap(), 0);
    assert_eq!(p["to0"].as_u64().unwrap(), 100 * mb);
    assert_eq!(p["from1"].as_u64().unwrap(), 50 * mb);
    assert_eq!(p["to1"].as_u64().unwrap(), 100 * mb);
    assert_eq!(g["hash_algo"], "Sha256");
    assert_eq!(
        g["sha256_digest"],
        "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
    );
}

/// Contract test for the future `paradl-engine` sidecar writer (W1-2).
#[test]
#[ignore = "depends on paradl-engine (W1-2): remove ignore once the crate exists"]
fn contract_sidecar_bytes_match_golden() {
    let g = fixture("sidecar.json");
    assert_eq!(g["state_suffix"], ".pdl.json");
    unimplemented!("paradl-engine (W1-2) not yet implemented");
}
