//! Zip (#37): every entry written by us through [`Writer`] (never `ZipArchive::extract`, whose
//! link handling was CVE-2025-29787), links left out, overlapping entries (a non-recursive bomb)
//! refused, encrypted ones (ZipCrypto, WinZip AES) opened with the password.

use std::io::{Read, Seek};
use std::path::Path;

use zip::result::ZipError;

use crate::Error;
use crate::safe::Writer;
use crate::volumes::{MultiFile, VolumeKind, volume_key};

pub fn extract(archive: &Path, w: &mut Writer, password: Option<&str>) -> Result<(), Error> {
    let name = archive
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if volume_key(&name).is_some_and(|v| v.kind == VolumeKind::Split) {
        let multi = MultiFile::open(archive)?.map_err(Error::MissingVolume)?;
        run(multi, w, password)
    } else {
        run(std::fs::File::open(archive)?, w, password)
    }
}

fn zip_error(e: ZipError, password: Option<&str>) -> Error {
    match e {
        ZipError::InvalidPassword => Error::WrongPassword,
        ZipError::UnsupportedArchive(m) if m == ZipError::PASSWORD_REQUIRED => {
            if password.is_some() {
                Error::WrongPassword
            } else {
                Error::NeedsPassword
            }
        }
        ZipError::Io(e) => Error::Io(e),
        other => Error::Damaged(other.to_string()),
    }
}

fn run<R: Read + Seek>(reader: R, w: &mut Writer, password: Option<&str>) -> Result<(), Error> {
    let mut zip = zip::ZipArchive::new(reader).map_err(|e| zip_error(e, password))?;
    refuse_overlaps(&mut zip, password)?;
    for i in 0..zip.len() {
        let mut entry = match password {
            Some(p) => zip.by_index_decrypt(i, p.as_bytes()),
            None => zip.by_index(i),
        }
        .map_err(|e| zip_error(e, password))?;
        let name = entry
            .name()
            .map(|n| n.into_owned())
            .map_err(|e| zip_error(e, password))?;
        if entry.is_symlink() {
            w.skip(&name, "link");
            continue;
        }
        if entry.is_dir() {
            w.dir(&name)?;
            continue;
        }
        let size = entry.size();
        let mode = entry.unix_mode();
        let encrypted = entry.encrypted();
        match w.file(&name, Some(size), &mut entry, mode) {
            // A wrong ZipCrypto password passes its one-byte check now and then, and fails as
            // bad data (its CRC) when read.
            Err(Error::Damaged(_)) if encrypted => return Err(Error::WrongPassword),
            r => r?,
        }
    }
    Ok(())
}

/// Two entries whose data overlap: a zip bomb built from one compressed block (Fifield's), or a
/// damaged archive. Refused before anything is written.
fn refuse_overlaps<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    password: Option<&str>,
) -> Result<(), Error> {
    let mut ranges = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let e = match zip.by_index_raw(i) {
            Ok(e) => e,
            Err(e) => return Err(zip_error(e, password)),
        };
        let start = e.header_start();
        let end = e
            .data_start()
            .unwrap_or(start)
            .saturating_add(e.compressed_size());
        ranges.push((start, end));
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|w| w[1].0 < w[0].1) {
        return Err(Error::Damaged("entries overlap".into()));
    }
    Ok(())
}
