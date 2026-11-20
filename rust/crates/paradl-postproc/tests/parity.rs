//! Parity tests with goldens captured from the Python oracle
//! (`src/paradl/postproc/*.py`).

use std::fs;
use std::io::Write;
use std::path::Path;

use paradl_core::ExitCode;
use paradl_postproc::adb;
use paradl_postproc::archive::{
    extract, is_archive, validate_link_target, validate_member_path, validate_tar, validate_zip,
};
use paradl_postproc::checksum::{
    Algorithm, Digest, SUPPORTED_ALGOS, compute_hashes, find_companion_checksum,
    parse_checksum_text,
};
use paradl_postproc::{ChecksumSource, DefaultPostProcessor, PostContext, PostProcessor};

const ABC: &[u8] = b"abc";
const MD5_ABC: &str = "900150983cd24fb0d6963f7d28e17f72";
const SHA1_ABC: &str = "a9993e364706816aba3e25717850c26c9cd0d89d";
const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const SHA512_ABC: &str = "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f";
const BLAKE2B_ABC: &str = "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d17d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923";

fn abc_file(dir: &Path) -> std::path::PathBuf {
    let path = dir.join("abc.bin");
    fs::write(&path, ABC).unwrap();
    path
}

// ---------------------------------------------------------------------------
// Checksum goldens (L22)
// ---------------------------------------------------------------------------

#[test]
fn supported_algos_golden() {
    assert_eq!(
        SUPPORTED_ALGOS,
        &["md5", "sha1", "sha256", "sha512", "blake2b"]
    );
}

#[test]
fn compute_hashes_abc_golden() {
    let dir = tempfile::tempdir().unwrap();
    let path = abc_file(dir.path());
    let algos = [
        Algorithm::Md5,
        Algorithm::Sha1,
        Algorithm::Sha256,
        Algorithm::Sha512,
        Algorithm::Blake2b,
    ];
    let hashes = compute_hashes(&path, &algos).unwrap();
    assert_eq!(hashes["md5"], MD5_ABC);
    assert_eq!(hashes["sha1"], SHA1_ABC);
    assert_eq!(hashes["sha256"], SHA256_ABC);
    assert_eq!(hashes["sha512"], SHA512_ABC);
    assert_eq!(hashes["blake2b"], BLAKE2B_ABC);
}

#[test]
fn algorithm_and_digest_parse_golden() {
    assert_eq!(Algorithm::parse("sha-256"), Some(Algorithm::Sha256));
    assert_eq!(Algorithm::parse("SHA256"), Some(Algorithm::Sha256));
    assert_eq!(Algorithm::parse("blake2b"), Some(Algorithm::Blake2b));
    assert_eq!(Algorithm::parse("nope"), None);
    assert_eq!(Algorithm::Sha256.name(), "sha256");
    assert_eq!(Algorithm::Sha512.hex_len(), 128);

    assert_eq!(
        Digest::parse(SHA256_ABC).unwrap(),
        Digest {
            algorithm: Algorithm::Sha256,
            hex: SHA256_ABC.to_string()
        }
    );
    assert_eq!(Digest::parse(MD5_ABC).unwrap().algorithm, Algorithm::Md5);
    assert_eq!(
        Digest::parse(&format!("SHA-256:{SHA256_ABC}"))
            .unwrap()
            .algorithm,
        Algorithm::Sha256
    );
    assert_eq!(
        Digest::parse(&format!("blake2b:{BLAKE2B_ABC}"))
            .unwrap()
            .algorithm,
        Algorithm::Blake2b
    );
    assert!(Digest::parse("bogus:00").is_err());
}

#[test]
fn gnu_and_bsd_checksum_text_golden() {
    let gnu = format!("{SHA256_ABC}  payload.bin\n");
    assert_eq!(
        parse_checksum_text(&gnu, None),
        Some((Algorithm::Sha256, SHA256_ABC.into()))
    );
    assert_eq!(
        parse_checksum_text(&gnu, Some("payload.bin")),
        Some((Algorithm::Sha256, SHA256_ABC.into()))
    );
    assert_eq!(parse_checksum_text(&gnu, Some("other.bin")), None);

    let gnu_binary = format!("{SHA256_ABC} *payload.bin\n");
    assert_eq!(
        parse_checksum_text(&gnu_binary, Some("payload.bin"))
            .unwrap()
            .1,
        SHA256_ABC
    );

    let bsd = format!("SHA256 (payload.bin) = {SHA256_ABC}\n");
    assert_eq!(
        parse_checksum_text(&bsd, Some("payload.bin")),
        Some((Algorithm::Sha256, SHA256_ABC.into()))
    );
    let bsd_alt = format!("SHA-256 (payload.bin) = {SHA256_ABC}\n");
    assert_eq!(
        parse_checksum_text(&bsd_alt, None).unwrap().0,
        Algorithm::Sha256
    );
    let bsd_b2 = format!("BLAKE2b (payload.bin) = {BLAKE2B_ABC}\n");
    assert_eq!(
        parse_checksum_text(&bsd_b2, None).unwrap().0,
        Algorithm::Blake2b
    );

    let upper = format!("{}  payload.bin\n", SHA256_ABC.to_ascii_uppercase());
    assert_eq!(parse_checksum_text(&upper, None).unwrap().1, SHA256_ABC);

    let with_comments = format!("# generated\n\n{SHA256_ABC}  payload.bin\n");
    assert_eq!(
        parse_checksum_text(&with_comments, None).unwrap().1,
        SHA256_ABC
    );
    assert_eq!(parse_checksum_text("", None), None);
    assert_eq!(
        parse_checksum_text(&format!("{MD5_ABC}  x\n"), None)
            .unwrap()
            .0,
        Algorithm::Md5
    );
    assert_eq!(
        parse_checksum_text(SHA256_ABC, None),
        Some((Algorithm::Sha256, SHA256_ABC.into()))
    );
}

#[test]
fn companion_discovery_golden() {
    let dir = tempfile::tempdir().unwrap();
    let path = abc_file(dir.path());
    let direct = dir.path().join("abc.bin.sha256");
    fs::write(&direct, format!("{SHA256_ABC}  abc.bin\n")).unwrap();
    assert_eq!(
        find_companion_checksum(&path),
        Some((Algorithm::Sha256, SHA256_ABC.into()))
    );

    fs::remove_file(&direct).unwrap();
    fs::write(
        dir.path().join("sha256sums.txt"),
        format!("{SHA256_ABC}  abc.bin\n"),
    )
    .unwrap();
    assert_eq!(
        find_companion_checksum(&path),
        Some((Algorithm::Sha256, SHA256_ABC.into()))
    );
}

#[test]
fn verification_maps_mismatch_to_exit_code_three() {
    let dir = tempfile::tempdir().unwrap();
    let path = abc_file(dir.path());
    let processor = DefaultPostProcessor;

    let good = Digest {
        algorithm: Algorithm::Sha256,
        hex: SHA256_ABC.into(),
    };
    let verification = processor.verify(&path, Some(&good)).unwrap();
    assert!(verification.verified);
    assert_eq!(verification.computed, SHA256_ABC);
    assert_eq!(verification.source, ChecksumSource::Explicit);

    let bad = Digest {
        algorithm: Algorithm::Sha256,
        hex: "00".repeat(32),
    };
    let err = processor.verify(&path, Some(&bad)).unwrap_err();
    assert_eq!(err.exit_code(), ExitCode::VerificationFailed);

    let companion = dir.path().join("abc.bin.sha256");
    fs::write(&companion, format!("{SHA256_ABC}  abc.bin\n")).unwrap();
    let verification = processor.verify(&path, None).unwrap();
    assert!(verification.verified);
    assert_eq!(verification.source, ChecksumSource::Companion);
}

// ---------------------------------------------------------------------------
// Archive security goldens (C10 / E38)
// ---------------------------------------------------------------------------

#[test]
fn archive_extension_and_path_validation_golden() {
    assert!(is_archive(Path::new("a.zip")));
    assert!(is_archive(Path::new("a.tar.gz")));
    assert!(is_archive(Path::new("a.7z")));
    assert!(!is_archive(Path::new("a.txt")));

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path();
    assert!(validate_member_path("ok/file.txt", dest).is_ok());
    for bad in [
        "../evil",
        "../../etc/passwd",
        "/abs/file",
        "C:\\evil",
        "a/b/../c",
        "",
    ] {
        assert!(validate_member_path(bad, dest).is_err(), "accepted: {bad}");
    }
    assert!(validate_link_target("f", "../../outside", dest, false).is_err());
    assert!(validate_link_target("f", "/abs", dest, true).is_err());
    assert!(validate_link_target("f", "", dest, false).is_err());
    assert!(validate_link_target("f", "sibling", dest, false).is_ok());
}

#[cfg(unix)]
#[test]
fn symlink_escape_is_rejected() {
    let base = tempfile::tempdir().unwrap();
    let dest = base.path().join("dest");
    let outside = base.path().join("outside");
    fs::create_dir_all(&dest).unwrap();
    fs::create_dir_all(&outside).unwrap();
    std::os::unix::fs::symlink(&outside, dest.join("link")).unwrap();
    assert!(validate_member_path("link/evil.txt", &dest).is_err());
}

#[test]
fn zip_security_and_extraction() {
    let base = tempfile::tempdir().unwrap();
    let safe = base.path().join("safe.zip");
    {
        let file = fs::File::create(&safe).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .unix_permissions(0o644);
        zip.start_file("dir/hello.txt", options).unwrap();
        zip.write_all(b"hello").unwrap();
        zip.finish().unwrap();
    }
    let out = base.path().join("out");
    let (ok, files) = extract(&safe, &out, false).unwrap();
    assert!(ok);
    assert_eq!(
        fs::read_to_string(out.join("dir/hello.txt")).unwrap(),
        "hello"
    );
    assert!(files.iter().any(|p| p.ends_with("hello.txt")));

    let evil = base.path().join("evil.zip");
    {
        let file = fs::File::create(&evil).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("../../escape.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"pwned").unwrap();
        zip.finish().unwrap();
    }
    let out2 = base.path().join("out2");
    let err = extract(&evil, &out2, false).unwrap_err();
    assert_eq!(err.exit_code(), ExitCode::VerificationFailed);
    assert!(!base.path().join("escape.txt").exists());
}

#[test]
fn tar_security_and_extraction() {
    let base = tempfile::tempdir().unwrap();
    let safe = base.path().join("safe.tar");
    {
        let file = fs::File::create(&safe).unwrap();
        let mut builder = tar::Builder::new(file);
        let mut header = tar::Header::new_gnu();
        header.set_size(5);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "dir/hi.txt", &b"hello"[..])
            .unwrap();
        builder.finish().unwrap();
    }
    let out = base.path().join("out");
    let (ok, _) = extract(&safe, &out, false).unwrap();
    assert!(ok);
    assert_eq!(fs::read_to_string(out.join("dir/hi.txt")).unwrap(), "hello");

    let evil = base.path().join("evil.tar");
    {
        let file = fs::File::create(&evil).unwrap();
        let mut builder = tar::Builder::new(file);
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o644);
        // `append_data` rejects `..`; write the raw ustar name field so the
        // malicious member reaches the validator exactly as Python would see it.
        header.as_mut_bytes()[..100].fill(0);
        header.as_mut_bytes()[..16].copy_from_slice(b"../../escape.txt");
        header.set_cksum();
        builder.append(&header, &[][..]).unwrap();
        builder.finish().unwrap();
    }
    let f = fs::File::open(&evil).unwrap();
    let mut archive = tar::Archive::new(f);
    assert!(validate_tar(&mut archive, &out).is_err());

    let fifo = base.path().join("fifo.tar");
    {
        let file = fs::File::create(&fifo).unwrap();
        let mut builder = tar::Builder::new(file);
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Fifo);
        header.set_size(0);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "badfifo", &[][..])
            .unwrap();
        builder.finish().unwrap();
    }
    let f = fs::File::open(&fifo).unwrap();
    let mut archive = tar::Archive::new(f);
    assert!(validate_tar(&mut archive, &out).is_err());
}

#[test]
fn zip_validate_helpers_are_usable() {
    let base = tempfile::tempdir().unwrap();
    let path = base.path().join("one.zip");
    {
        let file = fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("good.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"ok").unwrap();
        zip.finish().unwrap();
    }
    let mut zip = zip::ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
    assert!(validate_zip(&mut zip, base.path()).is_ok());
}

// ---------------------------------------------------------------------------
// ADB goldens
// ---------------------------------------------------------------------------

#[test]
fn adb_parse_devices_golden() {
    let output = "List of devices attached\n\
                  emulator-5554\tdevice product:sdk_gphone model:Pixel_7 device:oriole\n\
                  0A0F1234\tunauthorized model:Some_Phone\n\
                  dead-beef\toffline model:Ignored\n\n";
    let devices = adb::parse_devices(output);
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[0].serial, "emulator-5554");
    assert_eq!(devices[0].status, "device");
    assert_eq!(devices[0].model, "Pixel 7");
    assert_eq!(devices[1].status, "unauthorized");
    assert_eq!(adb::parse_devices(""), Vec::<adb::Device>::new());
}

#[test]
fn post_context_defaults_golden() {
    let ctx = PostContext::new("file.zip");
    assert_eq!(ctx.path, std::path::PathBuf::from("file.zip"));
    assert!(!ctx.extract && !ctx.adb_push);
    assert_eq!(adb::DEFAULT_MUSIC_DIR, "/storage/emulated/0/Music");
}
