//! One file compressed with gzip or xz (#37): `report.csv.gz` → `report.csv`.

use std::io::Read;
use std::path::Path;

use crate::safe::Writer;
use crate::{Error, Format};

pub fn extract(archive: &Path, format: Format, w: &mut Writer) -> Result<(), Error> {
    let file = std::io::BufReader::new(std::fs::File::open(archive)?);
    let name = archive
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lower = name.to_lowercase();
    let inner = [".gz", ".xz"]
        .iter()
        .find(|e| lower.ends_with(*e) && lower.len() > e.len())
        .map(|e| name[..name.len() - e.len()].to_owned())
        .unwrap_or_else(|| format!("{name}.out"));
    let mut reader: Box<dyn Read> = match format {
        Format::Xz => Box::new(lzma_rust2::XzReader::new(file, true)),
        _ => Box::new(flate2::read::MultiGzDecoder::new(file)),
    };
    w.file(&inner, None, &mut reader, None)
}
