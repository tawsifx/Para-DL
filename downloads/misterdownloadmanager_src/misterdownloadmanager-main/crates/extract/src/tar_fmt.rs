//! Tar (#37), plain or in gzip or xz: read entry by entry (never `unpack_in`), so the same rules
//! hold as for every format; links, hard links, devices and pipes are left out.

use std::io::Read;
use std::path::Path;

use crate::safe::Writer;
use crate::{Error, Format};

pub fn extract(archive: &Path, format: Format, w: &mut Writer) -> Result<(), Error> {
    let file = std::io::BufReader::new(std::fs::File::open(archive)?);
    let reader: Box<dyn Read> = match format {
        Format::TarGz => Box::new(flate2::read::MultiGzDecoder::new(file)),
        Format::TarXz => Box::new(lzma_rust2::XzReader::new(file, true)),
        _ => Box::new(file),
    };
    let mut tar = tar::Archive::new(reader);
    let entries = tar.entries().map_err(damaged)?;
    for entry in entries {
        let mut entry = entry.map_err(damaged)?;
        let name = entry
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(damaged)?;
        let kind = entry.header().entry_type();
        let mode = entry.header().mode().ok();
        if kind.is_dir() {
            w.dir(&name)?;
        } else if kind.is_file() || kind.is_contiguous() {
            let size = entry.header().size().ok();
            w.file(&name, size, &mut entry, mode)?;
        } else if kind.is_gnu_longname()
            || kind.is_gnu_longlink()
            || kind.is_pax_global_extensions()
            || kind.is_pax_local_extensions()
        {
            // Read by the tar crate itself for the entries they describe.
        } else {
            w.skip(&name, "link or device");
        }
    }
    Ok(())
}

fn damaged(e: std::io::Error) -> Error {
    if e.kind() == std::io::ErrorKind::NotFound || e.kind() == std::io::ErrorKind::PermissionDenied
    {
        Error::Io(e)
    } else {
        Error::Damaged(e.to_string())
    }
}
