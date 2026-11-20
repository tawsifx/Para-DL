//! Runnable example: the no-op platform and (when enabled) byte-exact goldens.
//!
//! ```sh
//! cargo run -p paradl-platform --example metadata
//! cargo run -p paradl-platform --features platform-macos,platform-windows --example metadata
//! ```

use std::path::Path;

use paradl_platform::{NoopPlatform, Platform};

fn main() {
    let noop = NoopPlatform;
    noop.set_origin_metadata(
        Path::new("/tmp/sample.bin"),
        &["https://example.com/sample.bin".into()],
    )
    .expect("noop metadata");
    println!(
        "noop secret: {:?}",
        noop.load_secret("pdl", "hf").expect("noop load")
    );

    #[cfg(feature = "platform-macos")]
    {
        use paradl_platform::macos::{binary_plist_string_array, keychain_add_command};
        let urls = vec!["https://github.com/astral-sh/uv".to_string()];
        let plist = binary_plist_string_array(&urls);
        println!("plist xattr hex: {}", hex::encode(&plist));
        println!(
            "keychain stdin: {}",
            keychain_add_command("pdl", "hf", "<token>", None).trim_end()
        );
    }

    #[cfg(feature = "platform-windows")]
    {
        use paradl_platform::windows::{
            ads_path, format_zone_identifier_text, unquarantine_zone_identifier_text,
        };
        let ini = format_zone_identifier_text(Some("https://github.com/astral-sh/uv"), Some(3));
        println!("zone identifier: {:?}", ini);
        println!(
            "ads path: {}",
            ads_path(Path::new("C:\\Downloads\\tool.exe"))
        );
        println!(
            "unquarantined: {:?}",
            unquarantine_zone_identifier_text(&ini)
        );
    }

    #[cfg(not(any(feature = "platform-macos", feature = "platform-windows")))]
    println!("Build with --features platform-macos,platform-windows to see OS goldens.");
}
