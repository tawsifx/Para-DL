//! Runnable example: hash a file, verify it, then reject a bad digest.
//!
//! ```sh
//! cargo run -p paradl-postproc --example hash_and_verify
//! ```

use std::path::Path;

use paradl_core::ExitCode;
use paradl_postproc::checksum::{Algorithm, compute_hashes};
use paradl_postproc::{DefaultPostProcessor, Digest, PostProcessor};

fn main() {
    let dir = std::env::temp_dir().join(format!("paradl-postproc-example-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("sample.bin");
    std::fs::write(&path, b"the quick brown fox").expect("write sample");

    let hashes = compute_hashes(
        &path,
        &[Algorithm::Md5, Algorithm::Sha256, Algorithm::Blake2b],
    )
    .expect("compute hashes");
    for algorithm in [Algorithm::Md5, Algorithm::Sha256, Algorithm::Blake2b] {
        println!("{:<10} {}", algorithm.name(), hashes[algorithm.name()]);
    }

    let processor = DefaultPostProcessor;
    let digest = Digest {
        algorithm: Algorithm::Sha256,
        hex: hashes["sha256"].clone(),
    };
    let verification = processor
        .verify(Path::new(&path), Some(&digest))
        .expect("verify");
    println!(
        "verified={} source={:?}",
        verification.verified, verification.source
    );

    let bad = Digest {
        algorithm: Algorithm::Sha256,
        hex: "00".repeat(32),
    };
    match processor.verify(Path::new(&path), Some(&bad)) {
        Ok(_) => println!("unexpected success"),
        Err(error) => println!("mismatch maps to exit code {}", error.exit_code().as_i32()),
    }
    assert_eq!(ExitCode::VerificationFailed.as_i32(), 3);

    std::fs::remove_dir_all(&dir).expect("cleanup");
}
