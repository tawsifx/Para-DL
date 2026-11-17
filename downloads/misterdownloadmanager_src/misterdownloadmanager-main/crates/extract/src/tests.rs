use std::io::Write;
use std::path::{Path, PathBuf};

use super::*;

fn run(
    archive: &Path,
    dest: &Path,
    password: Option<&str>,
    subfolder: Subfolder,
) -> (Result<Report, Error>, Vec<PathBuf>) {
    let mut seen = Vec::new();
    let mut on_file = |p: &Path| seen.push(p.to_path_buf());
    let mut policy = Policy {
        dest: dest.to_path_buf(),
        subfolder,
        password: password.map(str::to_owned),
        limits: Limits::default(),
        tool: find_7zip(),
        on_file: &mut on_file,
        cancel: None,
    };
    let r = extract(archive, &mut policy);
    (r, seen)
}

fn zip_of(path: &Path, entries: &[(&str, &[u8])], aes: Option<&str>) {
    let f = std::fs::File::create(path).unwrap();
    let mut z = zip::ZipWriter::new(f);
    for (name, data) in entries {
        let opts = zip::write::SimpleFileOptions::default();
        let opts = match aes {
            Some(p) => opts.with_aes_encryption(zip::AesMode::Aes256, p),
            None => opts,
        };
        z.start_file(*name, opts).unwrap();
        z.write_all(data).unwrap();
    }
    z.finish().unwrap();
}

/// No leftover staging folder beside the destination.
fn no_staging(dest: &Path) {
    let left: Vec<_> = std::fs::read_dir(dest)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(".mdm-extract-"))
        .collect();
    assert!(left.is_empty(), "staging left behind: {left:?}");
}

#[test]
fn entry_paths_stay_inside() {
    let d = 64;
    assert_eq!(
        safe::rel_path("a/b.txt", d).unwrap(),
        Path::new("a").join("b.txt")
    );
    assert_eq!(
        safe::rel_path("a\\b.txt", d).unwrap(),
        Path::new("a").join("b.txt")
    );
    assert_eq!(
        safe::rel_path("./a//b", d).unwrap(),
        Path::new("a").join("b")
    );
    for bad in [
        "../evil",
        "a/../../b",
        "..\\evil",
        "/abs",
        "\\abs",
        "C:\\x",
        "C:x",
        "x:ads",
        "a.txt:Zone.Identifier",
        "",
        "./",
    ] {
        assert!(safe::rel_path(bad, d).is_err(), "{bad} should be refused");
    }
    // Device names and trailing dots become safe names, not refusals.
    let con = safe::rel_path("CON.txt", d).unwrap();
    assert_ne!(con, Path::new("CON.txt"));
    assert!(safe::rel_path(&"a/".repeat(65), d).is_err());
}

#[test]
fn volume_names() {
    let v = volume_key("Movie.2024.part02.rar").unwrap();
    assert_eq!(
        (v.base.as_str(), v.kind, v.index),
        ("Movie.2024", VolumeKind::RarPart, 2)
    );
    assert_eq!(volume_key("a.PART1.RAR").unwrap().index, 1);
    let old = volume_key("a.rar").unwrap();
    assert_eq!((old.kind, old.index), (VolumeKind::RarOld, 1));
    assert_eq!(volume_key("a.r00").unwrap().index, 2);
    assert_eq!(volume_key("a.r01").unwrap().index, 3);
    let s = volume_key("pics.7z.003").unwrap();
    assert_eq!(
        (s.base.as_str(), s.kind, s.index),
        ("pics", VolumeKind::Split, 3)
    );
    assert!(volume_key("a.zip").is_none());
    assert!(volume_key("report.pdf").is_none());
    assert_eq!(base_name(Path::new("photos.tar.gz")), "photos");
    assert_eq!(base_name(Path::new("movie.part1.rar")), "movie");
}

#[test]
fn zip_smart_folder_and_marks() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("two.zip");
    zip_of(&a, &[("one.txt", b"1"), ("sub/two.txt", b"22")], None);
    let dest = t.path().join("out");
    let (r, seen) = run(&a, &dest, None, Subfolder::Smart);
    let r = r.unwrap();
    // Two things at the top: a folder named after the archive.
    assert_eq!(r.folder, dest.join("two"));
    assert_eq!(std::fs::read(dest.join("two/sub/two.txt")).unwrap(), b"22");
    assert_eq!((r.files, seen.len()), (2, 2));
    no_staging(&dest);

    // One folder at the top: straight into the destination; a second run keeps the first.
    let b = t.path().join("one.zip");
    zip_of(&b, &[("app/readme.txt", b"hi"), ("app/bin/x", b"x")], None);
    let (r, _) = run(&b, &dest, None, Subfolder::Smart);
    assert_eq!(r.unwrap().folder, dest);
    assert!(dest.join("app/readme.txt").is_file());
    let (r, _) = run(&b, &dest, None, Subfolder::Smart);
    assert_eq!(r.unwrap().renamed, 1);
    assert!(dest.join("app (1)/readme.txt").is_file());
    no_staging(&dest);
}

#[test]
fn zip_aes_passwords() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("secret.zip");
    zip_of(&a, &[("s.txt", b"secret")], Some("pw1"));
    let dest = t.path().join("out");
    assert!(matches!(
        run(&a, &dest, None, Subfolder::Never).0,
        Err(Error::NeedsPassword)
    ));
    assert!(matches!(
        run(&a, &dest, Some("nope"), Subfolder::Never).0,
        Err(Error::WrongPassword)
    ));
    no_staging(&dest);
    run(&a, &dest, Some("pw1"), Subfolder::Never).0.unwrap();
    assert_eq!(std::fs::read(dest.join("s.txt")).unwrap(), b"secret");
}

#[test]
fn hostile_zip_entries_are_left_out() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("evil.zip");
    let f = std::fs::File::create(&a).unwrap();
    let mut z = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default();
    for name in ["../outside.txt", "ok.txt", "C:/drive.txt"] {
        z.start_file(name, opts).unwrap();
        z.write_all(b"x").unwrap();
    }
    z.add_symlink("link", "../../etc", opts).unwrap();
    z.finish().unwrap();
    let dest = t.path().join("out");
    let (r, _) = run(&a, &dest, None, Subfolder::Never);
    let r = r.unwrap();
    assert!(dest.join("ok.txt").is_file());
    assert!(!t.path().join("outside.txt").exists());
    assert!(!dest.join("link").exists());
    let why: Vec<_> = r.skipped.iter().map(|(n, w)| format!("{n}: {w}")).collect();
    assert_eq!(r.skipped.len(), 3, "{why:?}");
}

#[test]
fn limits_stop_a_bomb() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("big.zip");
    let zeros = vec![0u8; 3 << 20];
    zip_of(&a, &[("z1", &zeros), ("z2", &zeros)], None);
    let dest = t.path().join("out");
    let mut on_file = |_: &Path| {};
    let mut policy = Policy {
        dest: dest.clone(),
        subfolder: Subfolder::Never,
        password: None,
        limits: Limits {
            total: 4 << 20,
            entries: 100,
            depth: 8,
        },
        tool: None,
        on_file: &mut on_file,
        cancel: None,
    };
    assert!(matches!(
        extract(&a, &mut policy),
        Err(Error::TooBig { .. })
    ));
    // Nothing half-written is left in place.
    assert!(!dest.join("z1").exists());
    no_staging(&dest);
}

/// 7-Zip's own archives (`7z a`): plain, encrypted, or split in 100 KB volumes.
fn seven_of(tool: &Path, src: &Path, archive: &Path, extra: &[&str]) {
    let ok = std::process::Command::new(tool)
        .arg("a")
        .arg("-bso0")
        .arg("-bsp0")
        .args(extra)
        .arg(archive)
        .arg(src.join("*"))
        .status()
        .unwrap()
        .success();
    assert!(ok, "7-Zip couldn't make {}", archive.display());
}

#[test]
fn sevenz_plain_encrypted_and_split_through_7zip() {
    let Some(tool) = find_7zip() else {
        eprintln!("7-Zip not found: skipped (no_7zip_says_so covers its absence)");
        return;
    };
    let t = tempfile::tempdir().unwrap();
    let src = t.path().join("src");
    std::fs::create_dir_all(src.join("inner")).unwrap();
    std::fs::write(src.join("a.txt"), b"alpha").unwrap();
    // Not compressible: the split below makes several volumes.
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let noise: Vec<u8> = (0..300_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    std::fs::write(src.join("inner/b.bin"), &noise).unwrap();
    let plain = t.path().join("plain.7z");
    seven_of(&tool, &src, &plain, &[]);
    let dest = t.path().join("o1");
    let r = run(&plain, &dest, None, Subfolder::Smart).0.unwrap();
    assert_eq!(std::fs::read(r.folder.join("a.txt")).unwrap(), b"alpha");
    assert_eq!(std::fs::read(r.folder.join("inner/b.bin")).unwrap(), noise);
    no_staging(&dest);

    let enc = t.path().join("enc.7z");
    seven_of(&tool, &src, &enc, &["-ps3cret"]);
    let dest = t.path().join("o2");
    assert!(matches!(
        run(&enc, &dest, None, Subfolder::Smart).0,
        Err(Error::NeedsPassword | Error::WrongPassword)
    ));
    assert!(matches!(
        run(&enc, &dest, Some("bad"), Subfolder::Smart).0,
        Err(Error::WrongPassword)
    ));
    run(&enc, &dest, Some("s3cret"), Subfolder::Smart)
        .0
        .unwrap();
    no_staging(&dest);

    // 7-Zip's "Split to volumes": split.7z.001, .002…
    let split = t.path().join("split.7z");
    seven_of(&tool, &src, &split, &["-v100k"]);
    let first = t.path().join("split.7z.001");
    assert!(t.path().join("split.7z.003").is_file());
    let dest = t.path().join("o3");
    let r = run(&first, &dest, None, Subfolder::Smart).0.unwrap();
    assert_eq!(r.folder, dest.join("split"));
    assert_eq!(std::fs::read(r.folder.join("inner/b.bin")).unwrap(), noise);
    std::fs::remove_file(t.path().join("split.7z.002")).unwrap();
    let dest = t.path().join("o4");
    assert!(matches!(
        run(&first, &dest, None, Subfolder::Smart).0,
        Err(Error::MissingVolume(2))
    ));
    no_staging(&dest);
}

#[test]
fn no_7zip_says_so() {
    // A 7z (and a RAR) without 7-Zip: NoTool, which the app words as "7z and RAR archives need
    // 7-Zip"; nothing is left behind.
    let t = tempfile::tempdir().unwrap();
    for (name, head) in [
        ("x.7z", &[b'7', b'z', 0xBC, 0xAF, 0x27, 0x1C, 0, 4][..]),
        ("y.rar", &b"Rar!\x1A\x07\x01\x00"[..]),
    ] {
        let a = t.path().join(name);
        std::fs::write(&a, head).unwrap();
        let dest = t.path().join("out");
        let mut on_file = |_: &Path| {};
        let mut policy = Policy {
            dest: dest.clone(),
            subfolder: Subfolder::Smart,
            password: None,
            limits: Limits::default(),
            tool: None,
            on_file: &mut on_file,
            cancel: None,
        };
        assert!(
            matches!(extract(&a, &mut policy), Err(Error::NoTool)),
            "{name}"
        );
        no_staging(&dest);
    }
}

#[test]
fn tar_gz_and_links() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("pack.tar.gz");
    let f = std::fs::File::create(&a).unwrap();
    let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
    let mut b = tar::Builder::new(gz);
    let mut h = tar::Header::new_gnu();
    h.set_size(5);
    h.set_mode(0o755);
    h.set_cksum();
    b.append_data(&mut h, "pack/run.sh", &b"hello"[..]).unwrap();
    let mut l = tar::Header::new_gnu();
    l.set_entry_type(tar::EntryType::Symlink);
    l.set_size(0);
    b.append_link(&mut l, "pack/evil", "/etc/passwd").unwrap();
    b.into_inner().unwrap().finish().unwrap();
    let dest = t.path().join("out");
    let r = run(&a, &dest, None, Subfolder::Smart).0.unwrap();
    assert_eq!(std::fs::read(dest.join("pack/run.sh")).unwrap(), b"hello");
    assert!(std::fs::symlink_metadata(dest.join("pack/evil")).is_err());
    assert_eq!(r.skipped.len(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let m = std::fs::metadata(dest.join("pack/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(m & 0o777, 0o755);
    }
}

#[test]
fn single_gz() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("report.csv.gz");
    let mut gz = flate2::write::GzEncoder::new(
        std::fs::File::create(&a).unwrap(),
        flate2::Compression::fast(),
    );
    gz.write_all(b"a,b\n1,2\n").unwrap();
    gz.finish().unwrap();
    let dest = t.path().join("out");
    run(&a, &dest, None, Subfolder::Smart).0.unwrap();
    assert_eq!(
        std::fs::read(dest.join("report.csv")).unwrap(),
        b"a,b\n1,2\n"
    );
}

#[test]
fn not_an_archive() {
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("notes.zip");
    std::fs::write(&a, b"just text").unwrap();
    assert!(matches!(
        run(&a, t.path(), None, Subfolder::Smart).0,
        Err(Error::Unsupported)
    ));
}

#[test]
fn external_7zip_goes_through_the_same_checks() {
    // RAR archives can't be made without RAR's own compressor, so the external path is checked
    // with a zip handed to 7-Zip the same way. Skipped where 7-Zip isn't installed.
    let Some(tool) = find_7zip() else {
        eprintln!("7-Zip not found: skipped");
        return;
    };
    let t = tempfile::tempdir().unwrap();
    let a = t.path().join("via7z.zip");
    zip_of(&a, &[("x/one.txt", b"1"), ("x/two.txt", b"2")], Some("pw"));
    let staging = safe::Staging::new(&t.path().join("out")).unwrap();
    let mut w = safe::Writer::new(staging.path(), Limits::default(), None);
    assert!(matches!(
        external::extract(&tool, &a, &mut w, None),
        Err(Error::NeedsPassword)
    ));
    assert!(matches!(
        external::extract(&tool, &a, &mut w, Some("bad")),
        Err(Error::WrongPassword)
    ));
    external::extract(&tool, &a, &mut w, Some("pw")).unwrap();
    let mut seen = 0;
    let (r, placed) = staging
        .place("via7z", Subfolder::Smart, &mut |_| seen += 1)
        .unwrap();
    assert_eq!((placed, seen), (2, 2));
    assert!(r.folder.join("x").join("two.txt").is_file());
}
