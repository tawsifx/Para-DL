// Link the GUI with Windows subsystem version 5.02 (D-008). Windows gives windows of programs
// linked this way the legacy frame metrics — dialogs get IDM's fixed frame (3 px borders,
// 26 px caption, measured: 566×122 outer for a 560×93 client) instead of the padded modern
// frame (8 px, 31 px). Resizable windows keep their 8 px frame, as IDM's main window has.
//
// Also embeds the exe's icon (Explorer, taskbar before the window sets its own) and version
// info.
fn main() {
    catalogs();
    commit();
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let windows = os == "windows";
    // `cfg(gpu)`: wgpu is built, with this system's own graphics API (renderer.rs, #89, #206):
    // Direct3D 12 on Windows, Vulkan on Linux, Metal on macOS (Cargo.toml).
    println!("cargo::rustc-check-cfg=cfg(gpu)");
    if matches!(os.as_str(), "windows" | "linux" | "macos") {
        println!("cargo::rustc-cfg=gpu");
    }
    if windows && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/SUBSYSTEM:WINDOWS,5.02");
    }
    // The icon and version info are for a Windows program only (not one built on Windows for
    // Linux or macOS); winresource builds on Windows only.
    #[cfg(windows)]
    if windows {
        println!("cargo:rerun-if-changed=assets/icons/app/mdm.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icons/app/mdm.ico")
            .set("ProductName", "Mister Download Manager")
            .set("FileDescription", "Mister Download Manager")
            .set("CompanyName", "Ahmed Nefzaoui")
            .set(
                "LegalCopyright",
                "Copyright (C) 2026 Ahmed Nefzaoui. GPL-3.0-or-later.",
            );
        if let Err(e) = res.compile() {
            // A missing resource compiler must not break the build; the window still sets
            // its icon at runtime.
            println!("cargo:warning=exe icon not embedded: {e}");
        }
    }
}

/// Embed every translation catalog, `i18n/<locale>.tsv` at the repository root (craft-i18n's
/// format; D-019), packed for `i18n::catalogs` to unpack once at start (#25):
/// `OUT_DIR/catalogs.br`, all of them in one brotli stream (quality 11: 12.4 MB of text in
/// about 0.8 MB, about 11 ms to unpack), and `OUT_DIR/catalogs.rs`, each locale's code and its
/// range in the unpacked text. The comment lines (`#`: translators' notes, `#, machine`) are
/// more than half of the text and craft-i18n skips them unread, so they are blanked, each kept as
/// an empty line so a problem's "line N" still points into the catalog file.
fn catalogs() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../i18n");
    println!("cargo:rerun-if-changed={}", root.display());
    let out_dir = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let mut text_all = String::new();
    let mut rows = Vec::new();
    if let Ok(dir) = std::fs::read_dir(&root) {
        let mut files: Vec<_> = dir.flatten().map(|e| e.path()).collect();
        files.sort();
        for path in files {
            let (Some(stem), Some("tsv")) = (
                path.file_stem().and_then(|s| s.to_str()).map(str::to_owned),
                path.extension().and_then(|s| s.to_str()),
            ) else {
                continue;
            };
            // `en.tsv` lists the source texts (for translators and checks); English needs no
            // catalog at run time. `languages.tsv` is the list of languages, not a catalog.
            if stem == "en" || stem == "languages" {
                continue;
            }
            println!("cargo:rerun-if-changed={}", path.display());
            let text = std::fs::read_to_string(&path).expect("a translation catalog");
            let rows_only: Vec<&str> = text
                .split('\n')
                .map(|line| if line.starts_with('#') { "" } else { line })
                .collect();
            let start = text_all.len();
            text_all.push_str(&rows_only.join("\n"));
            rows.push(format!("    ({stem:?}, {start}, {}),", text_all.len()));
        }
    }
    let params = brotli::enc::BrotliEncoderParams {
        quality: 11,
        lgwin: 24,
        size_hint: text_all.len(),
        ..Default::default()
    };
    let mut packed = Vec::new();
    brotli::BrotliCompress(&mut text_all.as_bytes(), &mut packed, &params)
        .expect("packing the catalogs");
    std::fs::write(out_dir.join("catalogs.br"), packed).unwrap();
    std::fs::write(
        out_dir.join("catalogs.rs"),
        format!("&[\n{}\n]\n", rows.join("\n")),
    )
    .unwrap();
    languages(&root);
}

/// `i18n/languages.tsv` (D-019) as `(code, English name, own name, flag SVG)` rows, the flags
/// from `assets/flags/` embedded: `OUT_DIR/languages.rs`, for the language picker.
fn languages(root: &std::path::Path) {
    let table = root.join("languages.tsv");
    println!("cargo:rerun-if-changed={}", table.display());
    let flags = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/flags");
    println!("cargo:rerun-if-changed={}", flags.display());
    let text = std::fs::read_to_string(&table).expect("i18n/languages.tsv");
    let mut header: Vec<&str> = Vec::new();
    let mut rows = Vec::new();
    for line in text.lines() {
        if let Some(h) = line.strip_prefix("# code\t") {
            header = std::iter::once("code").chain(h.split('\t')).collect();
            continue;
        }
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        assert_eq!(cols.len(), header.len(), "i18n/languages.tsv: {line}");
        let col = |name: &str| cols[header.iter().position(|h| *h == name).expect(name)];
        let flag = flags.join(format!("{}.svg", col("flag")));
        assert!(flag.exists(), "missing flag {}", flag.display());
        let flag = flag.canonicalize().unwrap_or(flag);
        rows.push(format!(
            "    ({:?}, {:?}, {:?}, include_bytes!({:?})),",
            col("code"),
            col("english"),
            col("native"),
            flag.display().to_string()
        ));
    }
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("languages.rs");
    std::fs::write(out, format!("&[\n{}\n]\n", rows.join("\n"))).unwrap();
}

/// The commit this is built from (`MDM_COMMIT`, "unknown" outside a git checkout), for Help →
/// About and problem reports.
fn commit() {
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/logs/HEAD");
    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=MDM_COMMIT={hash}");
}
