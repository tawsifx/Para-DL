//! The flags of the table's countries drawn into one picture (#140), in the table's order: cell
//! `i` (row-major, `COLUMNS` a row) is country `i + 1`'s. From lipis/flag-icons' 4x3 SVGs (MIT).
//!
//!   cargo run -p mdm-geo --example flag_atlas -- <flag-icons>/flags/4x3
//!
//! Writes crates/geo/data/flags.png.

use std::path::{Path, PathBuf};

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("the 4x3 flags folder"));
    let codes = mdm_geo::countries();
    // FLAGS (lib.rs) says how many cells the app may use.
    assert_eq!(
        codes.len() as u32,
        mdm_geo::FLAGS,
        "set FLAGS in lib.rs to the table's country count"
    );
    let (w, h) = (mdm_geo::FLAG_W, mdm_geo::FLAG_H);
    let cols = mdm_geo::COLUMNS;
    let rows = codes.len().div_ceil(cols as usize) as u32;
    let mut atlas = resvg::tiny_skia::Pixmap::new(w * cols, h * rows).expect("atlas");
    let opts = resvg::usvg::Options::default();
    for (i, code) in codes.iter().enumerate() {
        let file = dir.join(format!("{}.svg", code.to_ascii_lowercase()));
        let tree = resvg::usvg::Tree::from_data(&std::fs::read(&file).expect("flag"), &opts)
            .expect("flag read");
        let size = tree.size();
        let (x, y) = ((i as u32 % cols) * w, (i as u32 / cols) * h);
        let t = resvg::tiny_skia::Transform::from_row(
            w as f32 / size.width(),
            0.0,
            0.0,
            h as f32 / size.height(),
            x as f32,
            y as f32,
        );
        resvg::render(&tree, t, &mut atlas.as_mut());
    }
    let out = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join("flags.png");
    let file = std::io::BufWriter::new(std::fs::File::create(&out).expect("written"));
    let mut enc = png::Encoder::new(file, atlas.width(), atlas.height());
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::High);
    // tiny-skia's pixels are premultiplied; flags are opaque, so they are as they are.
    enc.write_header()
        .expect("header")
        .write_image_data(atlas.data())
        .expect("pixels");
    println!(
        "{} flags of {w}x{h} -> {} ({} bytes)",
        codes.len(),
        out.display(),
        std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0)
    );
}
