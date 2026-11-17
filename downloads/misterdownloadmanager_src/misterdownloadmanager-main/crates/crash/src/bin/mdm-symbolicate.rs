//! For whoever reads a crash report: a `.dmp` attached to an issue, made readable with that
//! release's symbols (the `MisterDownloadManager-<version>-symbols-<platform>.zip` beside its
//! downloads, or a folder it was unzipped to).
//!
//!     cargo run -p mdm-crash --bin mdm-symbolicate -- <file.dmp> [<symbols.zip | folder> ...]
//!
//! Prints the report: every thread with function, file and line.

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dump) = args.first() else {
        eprintln!("usage: mdm-symbolicate <file.dmp> [<symbols.zip | folder> ...]");
        std::process::exit(2);
    };
    let tmp = std::env::temp_dir().join(format!("mdm-symbolicate-{}", std::process::id()));
    let mut stores: Vec<PathBuf> = Vec::new();
    for (i, s) in args[1..].iter().enumerate() {
        let p = PathBuf::from(s);
        if p.extension().is_some_and(|e| e == "zip") {
            let dir = tmp.join(i.to_string());
            let bytes = std::fs::read(&p).unwrap_or_else(|e| fail(&format!("{s}: {e}")));
            mdm_crash::symbols::extract(&bytes, &dir)
                .unwrap_or_else(|e| fail(&format!("{s}: {e}")));
            stores.push(dir);
        } else {
            stores.push(p);
        }
    }
    let a = mdm_crash::analyze::analyze(std::path::Path::new(dump), &stores)
        .unwrap_or_else(|e| fail(&format!("{dump}: {e}")));
    // A dump asked for is a panic's or a freeze's: the main (window) thread first.
    let (kind, notes) = if a.requested {
        (
            mdm_crash::protocol::Kind::Freeze,
            mdm_crash::protocol::Notes {
                freeze: Some(mdm_crash::protocol::Freeze {
                    main: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
    } else {
        (mdm_crash::protocol::Kind::Crash, Default::default())
    };
    let input = mdm_crash::report::Input {
        kind: Some(kind),
        notes,
        analysis: Some(&a),
        dump: Some(dump.clone()),
        ..Default::default()
    };
    println!("{}", mdm_crash::report::text(&input, None));
    let _ = std::fs::remove_dir_all(&tmp);
}

fn fail(msg: &str) -> ! {
    eprintln!("mdm-symbolicate: {msg}");
    std::process::exit(1);
}
