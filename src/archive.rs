//! Archives as browsable directories. ZIP is read and written in place
//! (add / delete by rebuilding the archive); 7z, tar (plain, gz, xz, bz2,
//! zst) and RAR are read, and 7z/tar archives can be created.

use crate::format::{ATTR_ARCHIVE, ATTR_DIRECTORY};
use crate::vfs::FileEntry;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;
use zip::write::SimpleFileOptions;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    None,
    Gzip,
    Xz,
    Bzip2,
    Zstd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Zip,
    SevenZ,
    Tar(Compression),
    Rar,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Zip => "ZIP",
            Format::SevenZ => "7z",
            Format::Tar(Compression::None) => "TAR",
            Format::Tar(Compression::Gzip) => "TAR.GZ",
            Format::Tar(Compression::Xz) => "TAR.XZ",
            Format::Tar(Compression::Bzip2) => "TAR.BZ2",
            Format::Tar(Compression::Zstd) => "TAR.ZST",
            Format::Rar => "RAR",
        }
    }

    /// Only ZIP archives can be modified in place.
    pub fn writable(self) -> bool {
        self == Format::Zip
    }
}

/// Archive format by file name, `None` for anything that is not an archive.
pub fn format_of(name: &str) -> Option<Format> {
    let lower = name.to_ascii_lowercase();
    let ends = |exts: &[&str]| exts.iter().any(|e| lower.ends_with(e));
    Some(if ends(&[".zip", ".jar", ".war", ".ear", ".apk", ".nupkg", ".vsix", ".xpi", ".cbz", ".epub"]) {
        Format::Zip
    } else if ends(&[".7z", ".cb7"]) {
        Format::SevenZ
    } else if ends(&[".rar", ".cbr"]) {
        Format::Rar
    } else if ends(&[".tar.gz", ".tgz"]) {
        Format::Tar(Compression::Gzip)
    } else if ends(&[".tar.xz", ".txz"]) {
        Format::Tar(Compression::Xz)
    } else if ends(&[".tar.bz2", ".tbz", ".tbz2"]) {
        Format::Tar(Compression::Bzip2)
    } else if ends(&[".tar.zst", ".tzst"]) {
        Format::Tar(Compression::Zstd)
    } else if ends(&[".tar", ".cbt"]) {
        Format::Tar(Compression::None)
    } else {
        return None;
    })
}

/// Extensions opened as directories with Enter.
pub fn is_archive_name(name: &str) -> bool {
    format_of(name).is_some()
}

#[derive(Clone, Debug)]
pub struct ArchiveItem {
    /// Normalized path with `/` separators, no leading or trailing slash.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Position in the archive (ZIP central directory index).
    pub index: usize,
}

#[derive(Debug)]
pub struct ArchiveIndex {
    pub path: PathBuf,
    pub format: Format,
    pub items: Vec<ArchiveItem>,
    pub modified: Option<SystemTime>,
}

pub fn normalize_inner(name: &str) -> Option<String> {
    let name = name.replace('\\', "/");
    let mut parts = Vec::new();
    for part in name.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            p => parts.push(p),
        }
    }
    if parts.is_empty() { None } else { Some(parts.join("/")) }
}

pub fn open_archive(path: &Path) -> io::Result<zip::ZipArchive<BufReader<File>>> {
    let file = File::open(path)?;
    zip::ZipArchive::new(BufReader::new(file)).map_err(io::Error::other)
}

fn other(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

/// Decompressing reader for a tar archive.
fn tar_reader(path: &Path, c: Compression) -> io::Result<tar::Archive<Box<dyn Read>>> {
    let file = BufReader::new(File::open(path)?);
    let reader: Box<dyn Read> = match c {
        Compression::None => Box::new(file),
        Compression::Gzip => Box::new(flate2::read::MultiGzDecoder::new(file)),
        Compression::Xz => Box::new(lzma_rust2::XzReader::new(file, true)),
        Compression::Bzip2 => Box::new(bzip2::read::MultiBzDecoder::new(file)),
        Compression::Zstd => Box::new(ruzstd::decoding::StreamingDecoder::new(file).map_err(other)?),
    };
    Ok(tar::Archive::new(reader))
}

fn rar_time(dos: u32) -> Option<SystemTime> {
    zip::DateTime::try_from_msdos((dos >> 16) as u16, (dos & 0xFFFF) as u16).ok().and_then(zip_time_to_system)
}

impl ArchiveIndex {
    /// Opens an archive by its extension; unknown extensions are tried as ZIP
    /// (Ctrl+PgDn on a self-extracting or renamed archive).
    pub fn open(path: &Path) -> io::Result<ArchiveIndex> {
        let format = format_of(&path.to_string_lossy()).unwrap_or(Format::Zip);
        let mut items = Vec::new();
        match format {
            Format::Zip => {
                let mut zip = open_archive(path)?;
                for i in 0..zip.len() {
                    let f = zip.by_index_raw(i).map_err(other)?;
                    let Ok(name) = f.name() else { continue };
                    let Some(norm) = normalize_inner(&name) else { continue };
                    items.push(ArchiveItem {
                        path: norm,
                        is_dir: f.is_dir(),
                        size: f.size(),
                        modified: f.last_modified().and_then(zip_time_to_system),
                        index: i,
                    });
                }
            }
            Format::SevenZ => {
                let archive = sevenz_rust2::Archive::open(path).map_err(other)?;
                for (i, f) in archive.files.iter().enumerate() {
                    let Some(norm) = normalize_inner(f.name()) else { continue };
                    items.push(ArchiveItem {
                        path: norm,
                        is_dir: f.is_directory(),
                        size: f.size(),
                        modified: f.has_last_modified_date.then(|| SystemTime::from(f.last_modified_date())),
                        index: i,
                    });
                }
            }
            Format::Tar(c) => {
                let mut tar = tar_reader(path, c)?;
                for (i, entry) in tar.entries()?.enumerate() {
                    let entry = entry?;
                    let Some(norm) = normalize_inner(&entry.path()?.to_string_lossy()) else { continue };
                    let header = entry.header();
                    items.push(ArchiveItem {
                        path: norm,
                        is_dir: header.entry_type().is_dir(),
                        size: header.size().unwrap_or(0),
                        modified: header.mtime().ok().map(|s| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(s)),
                        index: i,
                    });
                }
            }
            Format::Rar => {
                let listing = unrar::Archive::new(path).open_for_listing().map_err(other)?;
                for (i, header) in listing.enumerate() {
                    let header = header.map_err(other)?;
                    let Some(norm) = normalize_inner(&header.filename.to_string_lossy()) else { continue };
                    items.push(ArchiveItem {
                        path: norm,
                        is_dir: header.is_directory(),
                        size: header.unpacked_size,
                        modified: rar_time(header.file_time),
                        index: i,
                    });
                }
            }
        }
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        Ok(ArchiveIndex { path: path.to_path_buf(), format, items, modified })
    }

    /// Direct children of `inner` (empty = root), including directories that
    /// only exist implicitly as path prefixes.
    pub fn children(&self, inner: &str) -> Vec<FileEntry> {
        let prefix = if inner.is_empty() { String::new() } else { format!("{inner}/") };
        let mut dirs: BTreeMap<String, Option<SystemTime>> = BTreeMap::new();
        let mut files = Vec::new();
        for item in &self.items {
            let Some(rest) = item.path.strip_prefix(&prefix) else { continue };
            if rest.is_empty() {
                continue;
            }
            match rest.split_once('/') {
                Some((dir, _)) => {
                    dirs.entry(dir.to_string()).or_insert(None);
                }
                None if item.is_dir => {
                    dirs.insert(rest.to_string(), item.modified);
                }
                None => files.push(FileEntry {
                    name: rest.to_string(),
                    is_dir: false,
                    is_parent: false,
                    size: item.size,
                    modified: item.modified,
                    attrs: ATTR_ARCHIVE,
                    selected: false,
                    dir_size: None,
                    full_path: None,
                    display: None,
                }),
            }
        }
        let mut out: Vec<FileEntry> = dirs
            .into_iter()
            .map(|(name, modified)| FileEntry {
                name,
                is_dir: true,
                is_parent: false,
                size: 0,
                modified,
                attrs: ATTR_DIRECTORY,
                selected: false,
                dir_size: None,
                full_path: None,
                display: None,
            })
            .collect();
        out.extend(files);
        out
    }

    /// All file items at or below the given inner paths ("" = everything).
    pub fn files_under<'a>(&'a self, paths: &[String]) -> Vec<&'a ArchiveItem> {
        self.items.iter().filter(|it| !it.is_dir && paths.iter().any(|p| within(&it.path, p))).collect()
    }
}

/// True when `path` is `sel` or lies below it (`sel` "" selects everything).
pub fn within(path: &str, sel: &str) -> bool {
    sel.is_empty() || path == sel || (path.len() > sel.len() && path.starts_with(sel) && path.as_bytes()[sel.len()] == b'/')
}

/// Streams the files of an archive for which `wanted` is true, in storage
/// order (solid 7z/RAR archives can only be read front to back). `f` gets the
/// item and its data; returning `Ok(false)` stops early.
pub fn for_each_entry(
    index: &ArchiveIndex,
    wanted: &dyn Fn(&str) -> bool,
    f: &mut dyn FnMut(&ArchiveItem, &mut dyn Read) -> io::Result<bool>,
    cancel: &AtomicBool,
) -> io::Result<()> {
    let by_path: HashMap<&str, &ArchiveItem> = index.items.iter().filter(|i| !i.is_dir).map(|i| (i.path.as_str(), i)).collect();
    let stopped = || io::Error::new(io::ErrorKind::Interrupted, "cancelled");
    match index.format {
        Format::Zip => {
            let mut zip = open_archive(&index.path)?;
            for item in index.items.iter().filter(|i| !i.is_dir && wanted(&i.path)) {
                if cancel.load(Ordering::Relaxed) {
                    return Err(stopped());
                }
                let mut entry = zip.by_index(item.index).map_err(other)?;
                if !f(item, &mut entry)? {
                    break;
                }
            }
        }
        Format::SevenZ => {
            let mut reader = sevenz_rust2::ArchiveReader::open(&index.path, sevenz_rust2::Password::empty()).map_err(other)?;
            let mut failure: Option<io::Error> = None;
            let result = reader.for_each_entries(|entry, data| {
                if cancel.load(Ordering::Relaxed) {
                    failure = Some(stopped());
                    return Ok(false);
                }
                let item = normalize_inner(entry.name()).and_then(|p| by_path.get(p.as_str()).copied());
                match item {
                    Some(item) if !entry.is_directory() && wanted(&item.path) => match f(item, data) {
                        Ok(go_on) => Ok(go_on),
                        Err(e) => {
                            failure = Some(e);
                            Ok(false)
                        }
                    },
                    // Skipped entries of a solid block must still be read through.
                    _ => io::copy(data, &mut io::sink()).map(|_| true).map_err(Into::into),
                }
            });
            if let Some(e) = failure {
                return Err(e);
            }
            result.map_err(other)?;
        }
        Format::Tar(c) => {
            let mut tar = tar_reader(&index.path, c)?;
            for entry in tar.entries()? {
                if cancel.load(Ordering::Relaxed) {
                    return Err(stopped());
                }
                let mut entry = entry?;
                if entry.header().entry_type().is_dir() {
                    continue;
                }
                let path = normalize_inner(&entry.path()?.to_string_lossy());
                if let Some(item) = path.and_then(|p| by_path.get(p.as_str()).copied()) {
                    if wanted(&item.path) && !f(item, &mut entry)? {
                        break;
                    }
                }
            }
        }
        Format::Rar => {
            // unrar extracts to files: go through a temporary file per entry.
            let tmp_dir = std::env::temp_dir().join("MCommander").join(format!("rar-{}", std::process::id()));
            std::fs::create_dir_all(&tmp_dir)?;
            let tmp = tmp_dir.join("entry.bin");
            let mut archive = unrar::Archive::new(&index.path).open_for_processing().map_err(other)?;
            let result = (|| -> io::Result<()> {
                while let Some(header) = archive.read_header().map_err(other)? {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(stopped());
                    }
                    let entry = header.entry();
                    let item = normalize_inner(&entry.filename.to_string_lossy()).and_then(|p| by_path.get(p.as_str()).copied());
                    match item {
                        Some(item) if entry.is_file() && wanted(&item.path) => {
                            archive = header.extract_to(&tmp).map_err(other)?;
                            let mut file = File::open(&tmp)?;
                            let go_on = f(item, &mut file)?;
                            drop(file);
                            let _ = std::fs::remove_file(&tmp);
                            if !go_on {
                                break;
                            }
                        }
                        _ => archive = header.skip().map_err(other)?,
                    }
                }
                Ok(())
            })();
            let _ = std::fs::remove_dir_all(&tmp_dir);
            result?;
        }
    }
    Ok(())
}

/// Extracts one file of an archive to `target`.
pub fn extract_one(index: &ArchiveIndex, inner: &str, target: &Path) -> io::Result<()> {
    let mut found = false;
    for_each_entry(
        index,
        &|p| p == inner,
        &mut |_, data| {
            let mut out = File::create(target)?;
            io::copy(data, &mut out)?;
            found = true;
            Ok(false)
        },
        &AtomicBool::new(false),
    )?;
    if found { Ok(()) } else { Err(io::Error::new(io::ErrorKind::NotFound, format!("{inner} not found in archive"))) }
}

/// Reads every file of the archive, verifying checksums. Returns the file count.
pub fn test_archive(path: &Path) -> io::Result<usize> {
    let index = ArchiveIndex::open(path)?;
    let mut n = 0;
    for_each_entry(
        &index,
        &|_| true,
        &mut |_, data| {
            io::copy(data, &mut io::sink())?;
            n += 1;
            Ok(true)
        },
        &AtomicBool::new(false),
    )?;
    Ok(n)
}

pub fn zip_time_to_system(dt: zip::DateTime) -> Option<SystemTime> {
    use chrono::{Local, NaiveDate, TimeZone};
    let date = NaiveDate::from_ymd_opt(dt.year() as i32, dt.month() as u32, dt.day() as u32)?;
    let naive = date.and_hms_opt(dt.hour() as u32, dt.minute() as u32, dt.second() as u32)?;
    Local.from_local_datetime(&naive).earliest().map(SystemTime::from)
}

pub fn system_to_zip_time(t: SystemTime) -> zip::DateTime {
    use chrono::{Datelike, Timelike};
    let dt: chrono::DateTime<chrono::Local> = t.into();
    zip::DateTime::from_date_and_time(
        dt.year().clamp(1980, 2107) as u16,
        dt.month() as u8,
        dt.day() as u8,
        dt.hour() as u8,
        dt.minute() as u8,
        dt.second() as u8,
    )
    .unwrap_or_default()
}

/// Builds the destination path for an archive member relative to `base`,
/// refusing anything that would escape `dest` (zip-slip).
pub fn safe_join(dest: &Path, rel: &str) -> Option<PathBuf> {
    let rel = normalize_inner(rel)?;
    let mut out = dest.to_path_buf();
    for part in rel.split('/') {
        let comp = Path::new(part);
        if comp.components().count() != 1 || !matches!(comp.components().next(), Some(Component::Normal(_))) {
            return None;
        }
        out.push(part);
    }
    Some(out)
}

/// A file or directory to add to an archive.
#[derive(Clone, Debug)]
pub struct AddItem {
    pub source: PathBuf,
    /// Name inside the archive (normalized, `/` separated).
    pub name: String,
}

/// Recursively expands sources into archive items under `prefix`.
pub fn expand_sources(sources: &[PathBuf], prefix: &str) -> Vec<AddItem> {
    let mut out = Vec::new();
    for src in sources {
        let Some(name) = src.file_name() else { continue };
        let base = if prefix.is_empty() {
            name.to_string_lossy().into_owned()
        } else {
            format!("{prefix}/{}", name.to_string_lossy())
        };
        walk_add(src, &base, &mut out);
    }
    out
}

fn walk_add(path: &Path, name: &str, out: &mut Vec<AddItem>) {
    out.push(AddItem { source: path.to_path_buf(), name: name.to_string() });
    if path.is_dir() {
        if let Ok(rd) = std::fs::read_dir(path) {
            let mut children: Vec<_> = rd.flatten().collect();
            children.sort_by_key(|e| e.file_name());
            for c in children {
                walk_add(&c.path(), &format!("{name}/{}", c.file_name().to_string_lossy()), out);
            }
        }
    }
}

/// Progress callback: (current name, bytes just processed). Returns false to abort.
pub type ProgressFn<'a> = dyn FnMut(&str, u64) -> bool + 'a;

/// Reads a source file in chunks, reporting progress and honouring cancel.
struct ProgressReader<'a, 'b> {
    inner: File,
    name: &'a str,
    progress: &'a mut ProgressFn<'b>,
    cancel: &'a AtomicBool,
}

impl Read for ProgressReader<'_, '_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        if (n > 0 && !(self.progress)(self.name, n as u64)) || self.cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
        Ok(n)
    }
}

/// Writes `archive` to a temporary file next to it and moves it into place
/// only when `write` succeeded.
fn write_atomically(archive: &Path, write: impl FnOnce(File) -> io::Result<()>) -> io::Result<()> {
    let tmp = archive.with_extension("mc_tmp");
    let result = File::create(&tmp).and_then(write);
    match result {
        Ok(()) => std::fs::rename(&tmp, archive).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        }),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Rewrites a ZIP `archive` keeping every entry for which `keep` returns true
/// and appending `add`. Creates the archive when it does not exist. Existing
/// entries are copied raw (no recompression), so this is fast.
pub fn rebuild(
    archive: &Path,
    keep: &dyn Fn(&str) -> bool,
    add: &[AddItem],
    progress: &mut ProgressFn,
    cancel: &AtomicBool,
) -> io::Result<()> {
    write_atomically(archive, |out| {
        let mut writer = zip::ZipWriter::new(io::BufWriter::new(out));
        let replaced: HashSet<&str> = add.iter().map(|a| a.name.as_str()).collect();
        if archive.exists() {
            let mut zip = open_archive(archive)?;
            for i in 0..zip.len() {
                let f = zip.by_index_raw(i).map_err(other)?;
                let name = f.name().map_err(other)?.into_owned();
                let norm = normalize_inner(&name).unwrap_or_default();
                if !keep(&norm) || replaced.contains(norm.as_str()) {
                    continue;
                }
                writer.raw_copy_file(f).map_err(other)?;
            }
        }
        for item in add {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
            }
            let meta = std::fs::metadata(&item.source)?;
            let mtime = meta.modified().map(system_to_zip_time).unwrap_or_default();
            let opts = SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
                .last_modified_time(mtime)
                .large_file(meta.len() > u32::MAX as u64);
            if meta.is_dir() {
                writer.add_directory(format!("{}/", item.name), opts).map_err(other)?;
                continue;
            }
            writer.start_file(item.name.as_str(), opts).map_err(other)?;
            let mut reader = ProgressReader { inner: File::open(&item.source)?, name: &item.name, progress, cancel };
            io::copy(&mut reader, &mut writer)?;
        }
        writer.finish().map_err(other)?.flush()
    })
}

/// Creates a new 7z or tar archive (format from the file name) from `add`.
pub fn create(archive: &Path, add: &[AddItem], progress: &mut ProgressFn, cancel: &AtomicBool) -> io::Result<()> {
    let format = format_of(&archive.to_string_lossy()).unwrap_or(Format::Zip);
    match format {
        Format::Zip => rebuild(archive, &|_| true, add, progress, cancel),
        Format::Rar => Err(io::Error::other("RAR archives can only be read, not created.")),
        Format::SevenZ => write_atomically(archive, |out| {
            let mut writer = sevenz_rust2::ArchiveWriter::new(out).map_err(other)?;
            for item in add {
                let mut entry = sevenz_rust2::ArchiveEntry::from_path(&item.source, item.name.clone());
                if item.source.is_dir() {
                    entry.is_directory = true;
                    writer.push_archive_entry::<File>(entry, None).map_err(other)?;
                } else {
                    let reader = ProgressReader { inner: File::open(&item.source)?, name: &item.name, progress, cancel };
                    writer.push_archive_entry(entry, Some(reader)).map_err(other)?;
                }
            }
            writer.finish().map(|_| ())
        }),
        Format::Tar(c) => write_atomically(archive, |out| {
            let out = io::BufWriter::new(out);
            let mut sink: Box<dyn Write> = match c {
                Compression::None => Box::new(out),
                Compression::Gzip => Box::new(flate2::write::GzEncoder::new(out, flate2::Compression::default())),
                Compression::Xz => Box::new(lzma_rust2::XzWriter::new(out, lzma_rust2::XzOptions::with_preset(6)).map_err(other)?),
                Compression::Bzip2 => Box::new(bzip2::write::BzEncoder::new(out, bzip2::Compression::default())),
                Compression::Zstd => return Err(io::Error::other("Creating .tar.zst archives is not supported; use .tar.xz or .tar.gz.")),
            };
            {
                let mut builder = tar::Builder::new(&mut sink);
                for item in add {
                    if item.source.is_dir() {
                        builder.append_dir(&item.name, &item.source)?;
                    } else {
                        let file = File::open(&item.source)?;
                        let mut header = tar::Header::new_gnu();
                        header.set_metadata(&file.metadata()?);
                        let reader = ProgressReader { inner: file, name: &item.name, progress, cancel };
                        builder.append_data(&mut header, &item.name, reader)?;
                    }
                }
                builder.finish()?;
            }
            sink.flush()?;
            // The boxed encoders finish their stream when dropped.
            drop(sink);
            Ok(())
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_rejects_traversal() {
        assert_eq!(normalize_inner("a\\b/c/").as_deref(), Some("a/b/c"));
        assert_eq!(normalize_inner("/./x"), Some("x".to_string()));
        assert_eq!(normalize_inner("../evil"), None);
        assert!(safe_join(Path::new("c:\\t"), "a/b.txt").is_some());
        assert!(safe_join(Path::new("c:\\t"), "a/../../b.txt").is_none());
        assert!(safe_join(Path::new("c:\\t"), "c:/windows/x").is_none());
        assert!(within("a/b", "a"));
        assert!(!within("ab/c", "a"));
        assert!(within("x", ""));
    }

    #[test]
    fn formats_by_extension() {
        assert_eq!(format_of("a.ZIP"), Some(Format::Zip));
        assert_eq!(format_of("a.7z"), Some(Format::SevenZ));
        assert_eq!(format_of("a.tar.gz"), Some(Format::Tar(Compression::Gzip)));
        assert_eq!(format_of("a.tgz"), Some(Format::Tar(Compression::Gzip)));
        assert_eq!(format_of("a.tar.xz"), Some(Format::Tar(Compression::Xz)));
        assert_eq!(format_of("a.tar"), Some(Format::Tar(Compression::None)));
        assert_eq!(format_of("a.rar"), Some(Format::Rar));
        assert_eq!(format_of("a.txt"), None);
    }

    #[test]
    fn pack_list_and_delete_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), b"alpha").unwrap();
        std::fs::write(src.join("sub").join("b.txt"), b"beta!").unwrap();
        let zip = dir.path().join("t.zip");
        let cancel = AtomicBool::new(false);
        let items = expand_sources(&[src.join("a.txt"), src.join("sub")], "");
        rebuild(&zip, &|_| true, &items, &mut |_, _| true, &cancel).unwrap();

        let idx = ArchiveIndex::open(&zip).unwrap();
        let root = idx.children("");
        assert!(root.iter().any(|e| e.name == "a.txt" && e.size == 5));
        assert!(root.iter().any(|e| e.name == "sub" && e.is_dir));
        let sub = idx.children("sub");
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0].name, "b.txt");
        assert_eq!(idx.files_under(&["sub".into()]).len(), 1);

        // Delete "sub" and add a file into a nested folder.
        let extra = dir.path().join("c.md");
        std::fs::write(&extra, b"gamma").unwrap();
        let add = expand_sources(&[extra], "docs");
        rebuild(&zip, &|p| !(p == "sub" || p.starts_with("sub/")), &add, &mut |_, _| true, &cancel).unwrap();
        let idx = ArchiveIndex::open(&zip).unwrap();
        let names: Vec<_> = idx.children("").into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["docs", "a.txt"]);
        assert_eq!(idx.children("docs")[0].name, "c.md");

        let out = dir.path().join("a.out");
        extract_one(&idx, "a.txt", &out).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"alpha");
    }

    /// Creates each writable non-ZIP format, then lists, reads and tests it.
    #[test]
    fn create_and_read_7z_and_tar_formats() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("data");
        std::fs::create_dir_all(src.join("sub").join("empty")).unwrap();
        std::fs::write(src.join("a.txt"), b"alpha").unwrap();
        std::fs::write(src.join("sub").join("\u{e5}\u{e4}\u{f6} \u{20ac} \u{65e5}\u{672c}.txt"), b"unicode name").unwrap();
        std::fs::write(src.join("zero.bin"), b"").unwrap();
        let cancel = AtomicBool::new(false);
        let add = expand_sources(&[src.clone()], "");
        for name in ["t.7z", "t.tar", "t.tar.gz", "t.tar.xz", "t.tar.bz2"] {
            let path = dir.path().join(name);
            create(&path, &add, &mut |_, _| true, &cancel).unwrap_or_else(|e| panic!("{name}: {e}"));
            let idx = ArchiveIndex::open(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(idx.format, format_of(name).unwrap());
            let root: Vec<_> = idx.children("data").into_iter().map(|e| e.name).collect();
            assert!(root.contains(&"a.txt".to_string()), "{name}: {root:?}");
            assert!(root.contains(&"sub".to_string()), "{name}: {root:?}");
            let sub: Vec<_> = idx.children("data/sub").into_iter().map(|e| e.name).collect();
            assert!(sub.iter().any(|n| n.contains('\u{65e5}')), "{name}: unicode name lost: {sub:?}");
            let out = dir.path().join(format!("{name}.out"));
            let inner = format!("data/sub/\u{e5}\u{e4}\u{f6} \u{20ac} \u{65e5}\u{672c}.txt");
            extract_one(&idx, &inner, &out).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(std::fs::read(&out).unwrap(), b"unicode name", "{name}");
            assert!(test_archive(&path).unwrap() >= 3, "{name}");
        }
    }

    #[test]
    fn reads_rar_fixtures() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures");
        for name in ["unicode.rar", "solid.rar"] {
            let path = fixtures.join(name);
            let idx = ArchiveIndex::open(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(idx.format, Format::Rar);
            let files = idx.files_under(&[String::new()]);
            assert!(!files.is_empty(), "{name}");
            let first = files[0].path.clone();
            let out = std::env::temp_dir().join(format!("mc-rar-test-{name}.out"));
            extract_one(&idx, &first, &out).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(std::fs::metadata(&out).unwrap().len(), files[0].size, "{name}");
            let _ = std::fs::remove_file(&out);
            assert_eq!(test_archive(&path).unwrap(), files.len(), "{name}");
        }
    }
}
