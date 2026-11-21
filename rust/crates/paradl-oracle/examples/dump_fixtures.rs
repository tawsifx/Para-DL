//! Print the list of captured golden fixtures and their byte sizes.
//!
//! Run with `cargo run -p paradl-oracle --example dump_fixtures`.

use paradl_oracle::fixtures::{FIXTURE_NAMES, fixture_path, fixture_str};

fn main() {
    println!("paradl-oracle golden fixtures:");
    for name in FIXTURE_NAMES {
        let bytes = fixture_str(name).len();
        println!(
            "  {name:<26} {bytes:>8} bytes  ({})",
            fixture_path(name).display()
        );
    }
}
