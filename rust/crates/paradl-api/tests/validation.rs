//! Oracle-derived validation tests for the `api` feature.
//!
//! `src/paradl/api/server.py:117-119` declares
//! `connections: int = Field(default=16, ge=1, le=16)`. FastAPI/pydantic v2
//! answers a violation with HTTP 422 and a `detail` array of issues shaped as
//! `{type, loc, msg, input, ctx}`. These tests pin the exact bytes of that
//! shape (the Python oracle test `test_api_optional_pydantic.py` only asserts
//! that the module imports without pydantic; the shapes below are derived from
//! the declared `Field` constraints themselves).

#![cfg(feature = "api")]

use paradl_api::{CreateTaskRequest, ResolveRequest, ValidatedRequest, ValidationIssue};
use serde_json::{Value, json};

#[test]
fn connections_below_ge_matches_pydantic_shape() {
    let value = json!({"target": "https://example.test/f", "connections": 0});
    let issues = CreateTaskRequest::validate(&value).expect_err("0 must fail ge=1");
    assert_eq!(issues.len(), 1, "exactly one issue: {issues:?}");

    assert_eq!(
        issues[0],
        ValidationIssue::greater_than_equal(vec!["body".into(), "connections".into()], json!(0), 1)
    );
    assert_eq!(
        serde_json::to_value(&issues[0]).unwrap(),
        json!({
            "type": "greater_than_equal",
            "loc": ["body", "connections"],
            "msg": "Input should be greater than or equal to 1",
            "input": 0,
            "ctx": {"ge": 1}
        })
    );
}

#[test]
fn connections_above_le_matches_pydantic_shape() {
    let value = json!({"target": "https://example.test/f", "connections": 17});
    let issues = CreateTaskRequest::validate(&value).expect_err("17 must fail le=16");
    assert_eq!(issues.len(), 1, "exactly one issue: {issues:?}");

    assert_eq!(
        serde_json::to_value(&issues[0]).unwrap(),
        json!({
            "type": "less_than_equal",
            "loc": ["body", "connections"],
            "msg": "Input should be less than or equal to 16",
            "input": 17,
            "ctx": {"le": 16}
        })
    );
}

#[test]
fn missing_target_matches_pydantic_shape() {
    let value = json!({"connections": 4});
    let issues = CreateTaskRequest::validate(&value).expect_err("target is required");
    assert_eq!(issues, vec![ValidationIssue::missing(
        vec!["body".into(), "target".into()],
        value.clone(),
    )]);
    assert_eq!(
        serde_json::to_value(&issues[0]).unwrap(),
        json!({
            "type": "missing",
            "loc": ["body", "target"],
            "msg": "Field required",
            "input": {"connections": 4}
        })
    );
}

#[test]
fn boundary_values_and_default_are_accepted() {
    for connections in [1, 16] {
        let value = json!({"target": "t", "connections": connections});
        let req = CreateTaskRequest::validate(&value).unwrap();
        assert_eq!(req.connections, connections);
    }
    let defaulted = CreateTaskRequest::validate(&json!({"target": "t"})).unwrap();
    assert_eq!(defaulted.connections, 16);
    assert_eq!(defaulted.watchdog, "v2");
    assert_eq!(defaulted.anti_throttle.as_deref(), Some("4MBps"));
    assert_eq!(defaulted.audio_codec, "aac");
    assert!(defaulted.is_audio);
    assert!(defaulted.embed_thumbnail);
}

#[test]
fn unknown_fields_are_ignored_like_pydantic_extra_ignore() {
    let value = json!({"target": "t", "bogus": 123});
    let req = CreateTaskRequest::validate(&value).unwrap();
    assert_eq!(req.target, "t");
    assert_eq!(req.connections, 16);
}

#[test]
fn resolve_request_requires_target() {
    assert!(ResolveRequest::validate(&json!({"target": "t"})).is_ok());
    let issues = ResolveRequest::validate(&Value::Null).expect_err("target is required");
    assert_eq!(
        serde_json::to_value(&issues[0]).unwrap(),
        json!({"type": "missing", "loc": ["body", "target"], "msg": "Field required", "input": null})
    );
}
