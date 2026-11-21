//! Byte-exact golden tests captured from the Python oracle.
//!
//! - macOS: `plistlib.dumps(urls, fmt=plistlib.FMT_BINARY).hex()` from
//!   `src/paradl/platform/macos.py:814` and the `security -i` command string
//!   from `:690-692`.
//! - Windows: `format_zone_identifier_text` / `unquarantine_zone_identifier_text`
//!   from `src/paradl/platform/windows.py:376-396`, exercised by
//!   `tests/unit/test_platform_windows.py::test_zone_identifier_ads_handling`.

use paradl_platform::NoopPlatform;
use paradl_platform::Platform;

#[cfg(feature = "platform-macos")]
use paradl_platform::macos::{
    KEYCHAIN_SERVICE, WHERE_FROMS_ATTR, binary_plist_string_array, keychain_add_command,
    keychain_find_command, where_froms_xattr_args,
};

#[cfg(feature = "platform-windows")]
use paradl_platform::windows::{
    ZONE_IDENTIFIER_STREAM, ads_path, format_zone_identifier_text, parse_zone_identifier_text,
    unquarantine_zone_identifier_text,
};

#[test]
fn noop_platform_is_a_safe_fallback() {
    let p = NoopPlatform;
    p.set_origin_metadata(
        std::path::Path::new("/tmp/x"),
        &["https://example.com".into()],
    )
    .unwrap();
    p.store_secret("pdl", "hf", "token").unwrap();
    assert_eq!(p.load_secret("pdl", "hf").unwrap(), None);
}

// ---------------------------------------------------------------------------
// L09 — macOS plist xattr hex goldens
// ---------------------------------------------------------------------------

#[cfg(feature = "platform-macos")]
#[test]
fn plist_empty_array_golden() {
    let urls: Vec<String> = vec![];
    assert_eq!(
        hex::encode(binary_plist_string_array(&urls)),
        "62706c6973743030a0080000000000000101000000000000000100000000000000000000000000000009"
    );
}

#[cfg(feature = "platform-macos")]
#[test]
fn plist_single_url_golden() {
    let urls = vec!["https://github.com/astral-sh/uv".to_string()];
    assert_eq!(
        hex::encode(binary_plist_string_array(&urls)),
        "62706c6973743030a1015f101f68747470733a2f2f6769746875622e636f6d2f61737472616c2d73682f7576080a000000000000010100000000000000020000000000000000000000000000002c"
    );
}

#[cfg(feature = "platform-macos")]
#[test]
fn plist_two_urls_golden() {
    let urls = vec![
        "https://github.com/astral-sh/uv".to_string(),
        "https://mirror.example.com/uv".to_string(),
    ];
    assert_eq!(
        hex::encode(binary_plist_string_array(&urls)),
        "62706c6973743030a201025f101f68747470733a2f2f6769746875622e636f6d2f61737472616c2d73682f75765f101d68747470733a2f2f6d6972726f722e6578616d706c652e636f6d2f7576080b2d000000000000010100000000000000030000000000000000000000000000004d"
    );
}

#[cfg(feature = "platform-macos")]
#[test]
fn plist_length_boundary_goldens() {
    // 14-char string: inline length marker 0x5e.
    let urls = vec!["https://a.co/a".to_string()];
    assert_eq!(
        hex::encode(binary_plist_string_array(&urls)),
        "62706c6973743030a1015e68747470733a2f2f612e636f2f61080a0000000000000101000000000000000200000000000000000000000000000019"
    );
    // 15-char string: extended length marker 0x5f + integer object.
    let urls = vec!["https://a.co/aa".to_string()];
    assert_eq!(
        hex::encode(binary_plist_string_array(&urls)),
        "62706c6973743030a1015f100f68747470733a2f2f612e636f2f6161080a000000000000010100000000000000020000000000000000000000000000001c"
    );
}

#[cfg(feature = "platform-macos")]
#[test]
fn plist_long_and_unicode_goldens() {
    let urls = vec!["a".repeat(255)];
    let hex = hex::encode(binary_plist_string_array(&urls));
    assert!(hex.starts_with("62706c6973743030a1015f10ff"));
    assert!(
        hex.ends_with("0008000a000000000000020100000000000000020000000000000000000000000000010c")
    );

    let urls = vec!["a".repeat(256)];
    let hex = hex::encode(binary_plist_string_array(&urls));
    assert!(hex.starts_with("62706c6973743030a1015f110100"));

    let urls = vec!["https://example.com/café".to_string()];
    assert_eq!(
        hex::encode(binary_plist_string_array(&urls)),
        "62706c6973743030a1016f101800680074007400700073003a002f002f006500780061006d0070006c0065002e0063006f006d002f00630061006600e9080a000000000000010100000000000000020000000000000000000000000000003d"
    );
}

#[cfg(feature = "platform-macos")]
#[test]
fn xattr_args_are_byte_exact() {
    let path = std::path::Path::new("/tmp/tool.zip");
    let urls = vec!["https://github.com/astral-sh/uv".to_string()];
    let args = where_froms_xattr_args(path, &urls);
    assert_eq!(args.len(), 5);
    assert_eq!(args[0], "-w");
    assert_eq!(args[1], "-x");
    assert_eq!(args[2], WHERE_FROMS_ATTR);
    assert_eq!(
        args[3],
        "62706c6973743030a1015f101f68747470733a2f2f6769746875622e636f6d2f61737472616c2d73682f7576080a000000000000010100000000000000020000000000000000000000000000002c"
    );
    assert_eq!(args[4], "/tmp/tool.zip");
}

// ---------------------------------------------------------------------------
// L11 — keychain secrets-over-stdin goldens (zero argv leak)
// ---------------------------------------------------------------------------

#[cfg(feature = "platform-macos")]
#[test]
fn keychain_commands_go_over_stdin_with_argv_escape() {
    assert_eq!(KEYCHAIN_SERVICE, "pdl");
    assert_eq!(
        keychain_add_command("pdl", "hf", "hf_test_secret_token_123", None),
        "add-generic-password -a \"hf\" -s \"pdl\" -U -w \"hf_test_secret_token_123\"\n"
    );
    assert_eq!(
        keychain_add_command("pdl", "github", "sec\"ret\\x", Some("login.keychain")),
        "add-generic-password -a \"github\" -s \"pdl\" -U -w \"sec\\\"ret\\\\x\" \"login.keychain\"\n"
    );
    assert_eq!(
        keychain_find_command("pdl", "hf", None),
        "find-generic-password -a \"hf\" -s \"pdl\" -w\n"
    );
    assert_eq!(
        keychain_find_command("pdl", "hf", Some("login.keychain")),
        "find-generic-password -a \"hf\" -s \"pdl\" -w \"login.keychain\"\n"
    );
}

// ---------------------------------------------------------------------------
// L10 — Windows Zone.Identifier CRLF INI goldens
// ---------------------------------------------------------------------------

#[cfg(feature = "platform-windows")]
#[test]
fn zone_identifier_format_golden() {
    assert_eq!(
        format_zone_identifier_text(None, None),
        "[ZoneTransfer]\r\n"
    );
    assert_eq!(
        format_zone_identifier_text(Some("https://github.com/astral-sh/uv"), None),
        "[ZoneTransfer]\r\nHostUrl=https://github.com/astral-sh/uv\r\n"
    );
    assert_eq!(
        format_zone_identifier_text(Some("https://example.com/tool.exe"), Some(3)),
        "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=https://example.com/tool.exe\r\n"
    );
}

#[cfg(feature = "platform-windows")]
#[test]
fn zone_identifier_parse_and_unquarantine_golden() {
    let parsed = parse_zone_identifier_text(
        "[ZoneTransfer]\nZoneId=3\nHostUrl=https://example.com/tool.exe\n",
    );
    assert_eq!(parsed.get("ZoneId").map(String::as_str), Some("3"));
    assert_eq!(
        parsed.get("HostUrl").map(String::as_str),
        Some("https://example.com/tool.exe")
    );
    assert_eq!(
        unquarantine_zone_identifier_text(
            "[ZoneTransfer]\nZoneId=3\nHostUrl=https://example.com/tool.exe\n"
        ),
        "[ZoneTransfer]\r\nHostUrl=https://example.com/tool.exe\r\n"
    );
}

#[cfg(feature = "platform-windows")]
#[test]
fn ads_path_golden() {
    assert_eq!(
        ads_path(std::path::Path::new("C:\\Downloads\\tool.exe")),
        "C:\\Downloads\\tool.exe:Zone.Identifier"
    );
    assert_eq!(ZONE_IDENTIFIER_STREAM, "Zone.Identifier");
}

#[cfg(feature = "platform-windows")]
#[test]
fn credential_store_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("credentials.json");
    let p = paradl_platform::windows::WindowsPlatform {
        store_path: Some(store.clone()),
    };
    p.store_secret("pdl", "hf", "hf_test_secret_token_123")
        .unwrap();
    assert_eq!(
        p.load_secret("pdl", "hf").unwrap().as_deref(),
        Some("hf_test_secret_token_123")
    );
}
