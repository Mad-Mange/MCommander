//! ZIP archives as browsable directories: index, listing, extraction helpers
//! and in-place modification (add / delete) by rebuilding the archive.

use crate::format::{ATTR_ARCHIVE, ATTR_DIRECTORY};
use crate::vfs::FileEntry;
use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;
use zip::write::SimpleFileOptions;

#[derive(Clone, Debug)]
pub struct ZipItem {
    /// Normalized path with `/` separators, no leading or trailing slash.
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub packed: u64,
    pub modified: Option<SystemTime>,
    pub index: usize,
}

#[derive(Debug)]
pub struct ZipIndex {
    pub path: PathBuf,
    pub items: Vec<ZipItem>,
    pub modified: Option<SystemTime>,
}

/// Extensions opened as directories with Enter.
pub fn is_archive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".zip", ".jar", ".war", ".ear", ".apk", ".nupkg", ".vsix", ".xpi", ".cbz", ".epub"]
        .iter()
        .any(|ext| lower.ends_with(ext))
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

impl ZipIndex {
    pub fn open(path: &Path) -> io::Result<ZipIndex> {
        let mut zip = open_archive(path)?;
        let mut items = Vec::with_capacity(zip.len());
        for i in 0..zip.len() {
            let f = zip.by_index_raw(i).map_err(io::Error::other)?;
            let Ok(name) = f.name() else { continue };
            let Some(norm) = normalize_inner(&name) else { continue };
            items.push(ZipItem {
                path: norm,
                is_dir: f.is_dir(),
                size: f.size(),
                packed: f.compressed_size(),
                modified: f.last_modified().and_then(zip_time_to_system),
                index: i,
            });
        }
        let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        Ok(ZipIndex { path: path.to_path_buf(), items, modified })
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

    /// All file items at or below the given inner paths.
    pub fn files_under<'a>(&'a self, paths: &[String]) -> Vec<&'a ZipItem> {
        self.items
            .iter()
            .filter(|it| !it.is_dir && paths.iter().any(|p| it.path == *p || it.path.starts_with(&format!("{p}/"))))
            .collect()
    }

    pub fn unpacked_size(&self, inner_dir: &str) -> u64 {
        let prefix = format!("{inner_dir}/");
        self.items.iter().filter(|i| i.path.starts_with(&prefix)).map(|i| i.size).sum()
    }
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

/// Rewrites `archive` keeping every entry for which `keep` returns true and
/// appending `add`. Creates the archive when it does not exist. Existing
/// entries are copied raw (no recompression), so this is fast.
pub fn rebuild(
    archive: &Path,
    keep: &dyn Fn(&str) -> bool,
    add: &[AddItem],
    progress: &mut ProgressFn,
    cancel: &AtomicBool,
) -> io::Result<()> {
    let tmp = archive.with_extension("mc_tmp");
    let result = (|| -> io::Result<()> {
        let out = File::create(&tmp)?;
        let mut writer = zip::ZipWriter::new(io::BufWriter::new(out));
        let replaced: HashSet<&str> = add.iter().map(|a| a.name.as_str()).collect();
        if archive.exists() {
            let mut zip = open_archive(archive)?;
            for i in 0..zip.len() {
                let f = zip.by_index_raw(i).map_err(io::Error::other)?;
                let name = f.name().map_err(io::Error::other)?.into_owned();
                let norm = normalize_inner(&name).unwrap_or_default();
                if !keep(&norm) || replaced.contains(norm.as_str()) {
                    continue;
                }
                writer.raw_copy_file(f).map_err(io::Error::other)?;
            }
        }
        let mut buf = vec![0u8; 256 * 1024];
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
                writer.add_directory(format!("{}/", item.name), opts).map_err(io::Error::other)?;
                continue;
            }
            writer.start_file(item.name.as_str(), opts).map_err(io::Error::other)?;
            let mut f = File::open(&item.source)?;
            loop {
                let n = f.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                writer.write_all(&buf[..n])?;
                if !progress(&item.name, n as u64) || cancel.load(Ordering::Relaxed) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                }
            }
        }
        writer.finish().map_err(io::Error::other)?.flush()?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            std::fs::rename(&tmp, archive).inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
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

        let idx = ZipIndex::open(&zip).unwrap();
        let root = idx.children("");
        assert!(root.iter().any(|e| e.name == "a.txt" && e.size == 5));
        assert!(root.iter().any(|e| e.name == "sub" && e.is_dir));
        let sub = idx.children("sub");
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0].name, "b.txt");
        assert_eq!(idx.unpacked_size("sub"), 5);
        assert_eq!(idx.files_under(&["sub".into()]).len(), 1);

        // Delete "sub" and add a file into a nested folder.
        let extra = dir.path().join("c.md");
        std::fs::write(&extra, b"gamma").unwrap();
        let add = expand_sources(&[extra], "docs");
        rebuild(&zip, &|p| !(p == "sub" || p.starts_with("sub/")), &add, &mut |_, _| true, &cancel).unwrap();
        let idx = ZipIndex::open(&zip).unwrap();
        let names: Vec<_> = idx.children("").into_iter().map(|e| e.name).collect();
        assert_eq!(names, ["docs", "a.txt"]);
        assert_eq!(idx.children("docs")[0].name, "c.md");

        let mut z = open_archive(&zip).unwrap();
        let mut s = String::new();
        z.by_name("a.txt").unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "alpha");
    }
}
