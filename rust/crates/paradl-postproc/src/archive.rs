//! Security-validated archive extraction (ZIP, tar, external 7z/DMG).
//!
//! Port of `src/paradl/postproc/archive.py`. Every member name is checked for
//! absolute paths, drive prefixes, `..` traversal, escaping symlinks/hardlinks,
//! and (for tar) device/FIFO entries **before** any bytes are written. All
//! rejections map to [`paradl_core::Error::VerificationFailed`] (exit code `3`).
//!
//! 7z and DMG handling stays in external processes (`7z`, `hdiutil`); no Python
//! is spawned.
//!
//! ```
//! use std::path::Path;
//! use paradl_postproc::archive::{is_archive, validate_member_path};
//! assert!(is_archive(Path::new("payload.tar.gz")));
//! let dest = std::env::temp_dir();
//! assert!(validate_member_path("../../etc/passwd", &dest).is_err());
//! ```

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use paradl_core::Error;
use tar::EntryType;

use crate::Result;

/// Raised when an archive contains traversal, absolute paths, or escaping links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveSecurityError(pub String);

impl fmt::Display for ArchiveSecurityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ArchiveSecurityError {}

impl From<ArchiveSecurityError> for Error {
    fn from(error: ArchiveSecurityError) -> Self {
        Error::verification_failed(error.0)
    }
}

/// Archive extensions recognised by the Python oracle (`archive.py:28-39`).
pub const ARCHIVE_EXTS: &[&str] = &[
    ".zip", ".tar", ".tgz", ".tar.gz", ".txz", ".tar.xz", ".tbz2", ".tar.bz2", ".7z", ".dmg",
];

fn reject(message: impl Into<String>) -> Error {
    Error::verification_failed(message)
}

/// Return true when `path` has a supported single or double archive extension.
pub fn is_archive(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    ARCHIVE_EXTS
        .iter()
        .any(|extension| name.ends_with(extension))
}

/// Resolve `relative` against `base`, rejecting any escape from `dest`.
fn resolve_within(dest: &Path, base: &Path, relative: &str) -> Result<PathBuf> {
    let mut current = base.to_path_buf();
    for part in relative.replace('\\', "/").split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                current.pop();
            }
            component => {
                current.push(component);
                if current.is_symlink() || current.exists() {
                    let real = fs::canonicalize(&current)?;
                    if !real.starts_with(dest) {
                        return Err(reject(format!(
                            "Archive member escapes destination directory: {relative:?}"
                        )));
                    }
                    current = real;
                }
            }
        }
    }
    if !current.starts_with(dest) {
        return Err(reject(format!(
            "Archive member escapes destination directory: {relative:?}"
        )));
    }
    Ok(current)
}

/// Validate a member name and resolve it within `dest_dir`.
pub fn validate_member_path(name: &str, dest_dir: &Path) -> Result<PathBuf> {
    if name.is_empty() {
        return Err(reject("Empty archive member name"));
    }
    if name.starts_with('/') || name.starts_with('\\') || name.as_bytes().get(1) == Some(&b':') {
        return Err(reject(format!(
            "Absolute path in archive is forbidden: {name:?}"
        )));
    }
    let normalized = name.replace('\\', "/");
    if normalized.split('/').any(|part| part == "..") {
        return Err(reject(format!(
            "Path traversal in archive member is forbidden: {name:?}"
        )));
    }
    let dest = fs::canonicalize(dest_dir)?;
    resolve_within(&dest, &dest, &normalized)
}

/// Validate a symlink/hardlink target so it cannot escape `dest_dir`.
pub fn validate_link_target(
    name: &str,
    target: &str,
    dest_dir: &Path,
    hardlink: bool,
) -> Result<()> {
    if target.is_empty() {
        return Err(reject(format!("Empty link target for {name:?}")));
    }
    if target.starts_with('/')
        || target.starts_with('\\')
        || target.as_bytes().get(1) == Some(&b':')
    {
        return Err(reject(format!(
            "Absolute link target in archive is forbidden: {name:?} -> {target:?}"
        )));
    }
    let dest = fs::canonicalize(dest_dir)?;
    let member_dest = validate_member_path(name, &dest)?;
    let base = if hardlink {
        dest.clone()
    } else {
        member_dest.parent().unwrap_or(&dest).to_path_buf()
    };
    match resolve_within(&dest, &base, target) {
        Ok(_) => Ok(()),
        Err(_) => Err(reject(format!(
            "Symlink/hardlink escapes destination directory: {name:?} -> {target:?}"
        ))),
    }
}

/// Validate every member of an open ZIP archive before extraction.
pub fn validate_zip<R: Read + io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    dest_dir: &Path,
) -> Result<()> {
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| reject(format!("ZIP entry error: {e}")))?;
        let name = entry.name().to_string();
        validate_member_path(&name, dest_dir)?;
        if let Some(mode) = entry.unix_mode()
            && is_symlink_mode(mode)
        {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            let target = String::from_utf8_lossy(&bytes);
            validate_link_target(&name, &target, dest_dir, false)?;
        }
    }
    Ok(())
}

/// Validate every member of an open tar archive before extraction.
pub fn validate_tar<R: Read>(archive: &mut tar::Archive<R>, dest_dir: &Path) -> Result<()> {
    for entry in archive
        .entries()
        .map_err(|e| reject(format!("tar entries error: {e}")))?
    {
        let entry = entry.map_err(|e| reject(format!("tar entry error: {e}")))?;
        let name = entry
            .path()
            .map_err(|e| reject(format!("tar path error: {e}")))?
            .to_string_lossy()
            .into_owned();
        validate_member_path(&name, dest_dir)?;
        match entry.header().entry_type() {
            EntryType::Symlink | EntryType::Link => {
                let target = entry
                    .link_name()
                    .map_err(|e| reject(format!("tar link error: {e}")))?
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                validate_link_target(
                    &name,
                    &target,
                    dest_dir,
                    entry.header().entry_type() == EntryType::Link,
                )?;
            }
            EntryType::Char | EntryType::Block | EntryType::Fifo => {
                return Err(reject(format!(
                    "Special device/FIFO member forbidden in archive: {name:?}"
                )));
            }
            _ => {}
        }
    }
    Ok(())
}

fn is_symlink_mode(mode: u32) -> bool {
    mode & 0o170000 == 0o120000
}

fn create_link(target: &str, path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, path)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(target, path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, path);
        Err(io::Error::other(
            "symlinks are unsupported on this platform",
        ))
    }
}

fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode & 0o777));
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
}

fn walk_files(directory: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            walk_files(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

/// Sanitise a DMG item name (`archive.py:285-286`).
pub fn sanitize_dmg_item(name: &str) -> String {
    name.replace(['/', '\\'], "_")
        .trim_start_matches('.')
        .to_string()
}

fn extract_zip(archive: &Path, dest: &Path, remove_archive: bool) -> Result<(bool, Vec<PathBuf>)> {
    let file = File::open(archive)?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| reject(format!("Bad ZIP archive: {e}")))?;
    validate_zip(&mut zip, dest)?;
    let mut files = Vec::new();
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| reject(format!("ZIP entry error: {e}")))?;
        let name = entry.name().to_string();
        let target = validate_member_path(&name, dest)?;
        let mode = entry.unix_mode();
        if mode.is_some_and(is_symlink_mode) {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            let link_target = String::from_utf8_lossy(&bytes).into_owned();
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            if target.exists() || target.is_symlink() {
                fs::remove_file(&target)?;
            }
            create_link(&link_target, &target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut output = File::create(&target)?;
        io::copy(&mut entry, &mut output)?;
        if let Some(mode) = mode
            && mode & 0o777 != 0
        {
            set_mode(&target, mode);
        }
        if target.is_file() && !target.is_symlink() {
            files.push(target);
        }
    }
    if remove_archive {
        let _ = fs::remove_file(archive);
    }
    Ok((true, files))
}

fn extract_tar(archive: &Path, dest: &Path, remove_archive: bool) -> Result<(bool, Vec<PathBuf>)> {
    {
        let file = File::open(archive)?;
        let mut reader = tar::Archive::new(file);
        validate_tar(&mut reader, dest)?;
    }
    let file = File::open(archive)?;
    let mut reader = tar::Archive::new(file);
    let mut files = Vec::new();
    for entry in reader
        .entries()
        .map_err(|e| reject(format!("tar entries error: {e}")))?
    {
        let mut entry = entry.map_err(|e| reject(format!("tar entry error: {e}")))?;
        let name = entry
            .path()
            .map_err(|e| reject(format!("tar path error: {e}")))?
            .to_string_lossy()
            .into_owned();
        let target = validate_member_path(&name, dest)?;
        match entry.header().entry_type() {
            EntryType::Directory => {
                fs::create_dir_all(&target)?;
            }
            EntryType::Symlink | EntryType::Link => {
                let link_target = entry
                    .link_name()
                    .map_err(|e| reject(format!("tar link error: {e}")))?
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                validate_link_target(
                    &name,
                    &link_target,
                    dest,
                    entry.header().entry_type() == EntryType::Link,
                )?;
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                if target.exists() || target.is_symlink() {
                    fs::remove_file(&target)?;
                }
                let source = if entry.header().entry_type() == EntryType::Link {
                    resolve_within(dest, dest, &link_target)?
                } else {
                    PathBuf::from(&link_target)
                };
                create_link(&source.to_string_lossy(), &target)?;
            }
            EntryType::Regular => {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                let mut output = File::create(&target)?;
                io::copy(&mut entry, &mut output)?;
                set_mode(&target, entry.header().mode()?);
                if target.is_file() && !target.is_symlink() {
                    files.push(target);
                }
            }
            _ => {}
        }
    }
    if remove_archive {
        let _ = fs::remove_file(archive);
    }
    Ok((true, files))
}

fn find_tool(candidates: &[&str]) -> Option<String> {
    candidates
        .iter()
        .find(|tool| Command::new(tool).arg("--help").output().is_ok())
        .map(|s| (*s).to_string())
}

fn extract_7z(archive: &Path, dest: &Path) -> Result<(bool, Vec<PathBuf>)> {
    let Some(tool) = find_tool(&["7z", "7zz", "p7zip"]) else {
        return Ok((false, Vec::new()));
    };
    let listing = Command::new(&tool)
        .arg("l")
        .arg("-slt")
        .arg(archive)
        .output()?;
    if !listing.status.success() {
        return Ok((false, Vec::new()));
    }
    let text = String::from_utf8_lossy(&listing.stdout);
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("Path = ") {
            let name = name.trim();
            if !name.is_empty()
                && name != archive.to_string_lossy()
                && name != archive.file_name().unwrap_or_default().to_string_lossy()
            {
                validate_member_path(name, dest)?;
            }
        } else if let Some(target) = line.strip_prefix("SymLink = ") {
            let target = target.trim();
            if !target.is_empty() {
                validate_link_target("symlink", target, dest, false)?;
            }
        }
    }
    let status = Command::new(&tool)
        .arg("x")
        .arg(format!("-o{}", dest.display()))
        .arg("-y")
        .arg(archive)
        .status()?;
    if !status.success() {
        return Ok((false, Vec::new()));
    }
    let mut files = Vec::new();
    walk_files(dest, &mut files)?;
    Ok((true, files))
}

fn extract_dmg(archive: &Path, dest: &Path) -> Result<(bool, Vec<PathBuf>)> {
    if !cfg!(target_os = "macos") {
        return Ok((false, Vec::new()));
    }
    let output = Command::new("hdiutil")
        .args(["attach", "-nobrowse", "-readonly"])
        .arg(archive)
        .output()?;
    if !output.status.success() {
        return Ok((false, Vec::new()));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mount = text
        .lines()
        .find_map(|line| {
            line.split_once("/Volumes/")
                .map(|(_, rest)| format!("/Volumes/{}", rest.trim()))
        })
        .filter(|mount| Path::new(mount).exists());
    let Some(mount) = mount else {
        return Ok((false, Vec::new()));
    };
    let mut files = Vec::new();
    for entry in fs::read_dir(&mount)? {
        let item = entry?.file_name().to_string_lossy().into_owned();
        if item.starts_with('.') {
            continue;
        }
        let source = Path::new(&mount).join(&item);
        let target = dest.join(sanitize_dmg_item(&item));
        if item.ends_with(".app") && source.is_dir() {
            if target.exists() {
                let _ = fs::remove_dir_all(&target);
            }
            copy_dir(&source, &target)?;
            files.push(target);
        } else if source.is_file() && !source.is_symlink() {
            fs::copy(&source, &target)?;
            files.push(target);
        }
    }
    let _ = Command::new("hdiutil").arg("detach").arg(&mount).output();
    Ok((true, files))
}

fn copy_dir(source: &Path, target: &Path) -> io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let destination = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &destination)?;
        } else {
            fs::copy(entry.path(), destination)?;
        }
    }
    Ok(())
}

/// Extract `archive` into `dest_dir` after full security validation.
///
/// Returns `(success, extracted_files)`. Security violations are returned as
/// [`Error::VerificationFailed`]; unsupported formats return `(false, vec![])`.
pub fn extract(
    archive: &Path,
    dest_dir: &Path,
    remove_archive: bool,
) -> Result<(bool, Vec<PathBuf>)> {
    let archive = fs::canonicalize(archive)?;
    if !archive.is_file() {
        return Ok((false, Vec::new()));
    }
    fs::create_dir_all(dest_dir)?;
    let dest = fs::canonicalize(dest_dir)?;
    let name = archive.to_string_lossy().to_ascii_lowercase();

    if name.ends_with(".zip") {
        extract_zip(&archive, &dest, remove_archive)
    } else if [
        ".tar", ".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz2",
    ]
    .iter()
    .any(|extension| name.ends_with(extension))
    {
        extract_tar(&archive, &dest, remove_archive)
    } else if name.ends_with(".dmg") {
        extract_dmg(&archive, &dest)
    } else if name.ends_with(".7z") {
        extract_7z(&archive, &dest)
    } else {
        Ok((false, Vec::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dmg_item_sanitization_matches_oracle() {
        assert_eq!(sanitize_dmg_item("../Foo.app"), "_Foo.app");
        assert_eq!(sanitize_dmg_item("a\\b"), "a_b");
    }
}
