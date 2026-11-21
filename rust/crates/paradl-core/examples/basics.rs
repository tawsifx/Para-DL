//! Runnable tour of `paradl-core`'s public API.

use paradl_core::clock::{Clock, FakeClock};
use paradl_core::{
    Error, ExitCode, PDLConfig, TargetType, format_rate_for_aria2, format_size, format_speed,
    has_direct_extension, parse_rate, sanitize_filename,
};

fn main() {
    println!("paradl-core basics");

    // Byte-exact rate parsing and formatting.
    println!("1M       = {} B/s", parse_rate("1M").unwrap());
    println!("1Mbps    = {} B/s", parse_rate("1Mbps").unwrap());
    println!(
        "aria2 4MBps = {:?}",
        format_rate_for_aria2("4MBps").unwrap()
    );
    println!("size 3 MiB  = {}", format_size(Some(3.0 * 1024.0 * 1024.0)));
    println!("speed 1.5M  = {}", format_speed(1536.0));

    // Filename and extension rules.
    println!("sanitize(CON) = {}", sanitize_filename("CON"));
    println!(
        "direct(zip)   = {}",
        has_direct_extension("https://host/file.zip?dl=1")
    );

    // Exit-code model (Rust owns the whole mapping).
    println!("usage exit code = {}", Error::usage("bad flag").exit_code());
    println!(
        "media dependency -> {}",
        Error::from_media_kind("dependency", "no yt-dlp").exit_code()
    );
    println!("verification code = {}", ExitCode::VerificationFailed);

    // Config defaults and validation.
    let mut config = PDLConfig {
        connections: 32,
        ..Default::default()
    };
    config.validate().unwrap();
    println!("clamped connections = {}", config.connections);
    println!("target type = {}", TargetType::Direct);

    // Deterministic clock.
    let mut clock = FakeClock::default();
    clock.sleep(2.0);
    println!(
        "fake wall time = {:?}; monotonic = {}",
        clock.now(),
        clock.monotonic()
    );
}
