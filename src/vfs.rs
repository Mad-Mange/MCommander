//! Virtual file system: local directories, the inside of archives, search
//! result lists ("feed to listbox") and directories on FTP/SFTP servers,
//! presented as one kind of listing.

use crate::archive::ArchiveIndex;
use crate::format::{self, ATTR_DIRECTORY, ATTR_HIDDEN, ATTR_REPARSE, ATTR_SYSTEM};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Location {
    /// A directory on a local or UNC file system.
    Fs(PathBuf),
    /// A directory inside an archive. `inner` uses `/`, no leading or trailing slash.
    Archive { archive: PathBuf, inner: String },
    /// A flat list of search results.
    Results { title: String, items: Arc<Vec<PathBuf>> },
    /// A directory on an FTP/SFTP server; `path` is absolute and uses `/`.
    Remote { conn: Arc<crate::remote::Connection>, path: String },
}

impl Location {
    pub fn fs(p: impl Into<PathBuf>) -> Location {
        Location::Fs(normalize_dir(&p.into()))
    }

    /// Path shown in the panel header and command-line prompt, e.g. `d:\Work\`.
    pub fn display(&self) -> String {
        match self {
            Location::Fs(p) => with_trailing_sep(&p.to_string_lossy()),
            Location::Archive { archive, inner } => {
                let mut s = archive.to_string_lossy().into_owned();
                s.push('\\');
                if !inner.is_empty() {
                    s.push_str(&inner.replace('/', "\\"));
                    s.push('\\');
                }
                s
            }
            Location::Results { title, .. } => format!("{title}\\"),
            Location::Remote { conn, path } => format!("{}{}", conn.url_prefix(), if path.ends_with('/') { path.clone() } else { format!("{path}/") }),
        }
    }

    /// Short caption for folder tabs.
    pub fn caption(&self) -> String {
        match self {
            Location::Fs(p) => match p.file_name() {
                Some(n) => n.to_string_lossy().into_owned(),
                None => p.to_string_lossy().trim_end_matches('\\').to_string(),
            },
            Location::Archive { archive, inner } => match inner.rsplit('/').next() {
                Some(n) if !n.is_empty() => n.to_string(),
                _ => archive
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            },
            Location::Results { title, .. } => title.clone(),
            Location::Remote { conn, path } => match crate::remote::split(path) {
                Some((_, name)) => name,
                None => conn.params.host.clone(),
            },
        }
    }

    pub fn fs_path(&self) -> Option<&Path> {
        match self {
            Location::Fs(p) => Some(p),
            _ => None,
        }
    }

    /// The real directory backing this location (for archives: the folder
    /// containing the archive), used for the command line and free space.
    pub fn host_dir(&self) -> Option<PathBuf> {
        match self {
            Location::Fs(p) => Some(p.clone()),
            Location::Archive { archive, .. } => archive.parent().map(Path::to_path_buf),
            Location::Results { .. } | Location::Remote { .. } => None,
        }
    }

    /// The server connection and directory of a remote location.
    pub fn remote(&self) -> Option<(&Arc<crate::remote::Connection>, &str)> {
        match self {
            Location::Remote { conn, path } => Some((conn, path)),
            _ => None,
        }
    }

    /// Drive letter (upper case) when the location is on a lettered drive.
    pub fn drive_letter(&self) -> Option<char> {
        let p = match self {
            Location::Fs(p) => p.as_path(),
            Location::Archive { archive, .. } => archive.as_path(),
            Location::Results { .. } | Location::Remote { .. } => return None,
        };
        let s = p.to_string_lossy();
        let mut chars = s.chars();
        match (chars.next(), chars.next()) {
            (Some(c), Some(':')) if c.is_ascii_alphabetic() => Some(c.to_ascii_uppercase()),
            _ => None,
        }
    }

    /// The parent location and the name of the child we came from (to place
    /// the cursor on it after going up).
    pub fn parent(&self) -> Option<(Location, String)> {
        match self {
            Location::Fs(p) => {
                let parent = p.parent()?;
                let name = p.file_name()?.to_string_lossy().into_owned();
                Some((Location::Fs(normalize_dir(parent)), name))
            }
            Location::Archive { archive, inner } => {
                if inner.is_empty() {
                    let parent = archive.parent()?;
                    let name = archive.file_name()?.to_string_lossy().into_owned();
                    Some((Location::Fs(normalize_dir(parent)), name))
                } else {
                    let (up, name) = match inner.rfind('/') {
                        Some(i) => (inner[..i].to_string(), inner[i + 1..].to_string()),
                        None => (String::new(), inner.clone()),
                    };
                    Some((Location::Archive { archive: archive.clone(), inner: up }, name))
                }
            }
            Location::Results { .. } => None,
            Location::Remote { conn, path } => {
                let (up, name) = crate::remote::split(path)?;
                Some((Location::Remote { conn: conn.clone(), path: up }, name))
            }
        }
    }

    pub fn root(&self) -> Location {
        match self {
            Location::Fs(p) => Location::Fs(root_of(p)),
            Location::Archive { archive, .. } => Location::Fs(root_of(archive)),
            Location::Results { .. } => self.clone(),
            Location::Remote { conn, .. } => Location::Remote { conn: conn.clone(), path: "/".into() },
        }
    }

    pub fn child_dir(&self, name: &str) -> Location {
        match self {
            Location::Fs(p) => Location::Fs(p.join(name)),
            Location::Archive { archive, inner } => Location::Archive {
                archive: archive.clone(),
                inner: if inner.is_empty() { name.to_string() } else { format!("{inner}/{name}") },
            },
            Location::Results { .. } => self.clone(),
            Location::Remote { conn, path } => Location::Remote { conn: conn.clone(), path: crate::remote::join(path, name) },
        }
    }

    /// Full path of a child entry on disk (not valid inside archives).
    pub fn child_path(&self, entry: &FileEntry) -> Option<PathBuf> {
        if let Some(p) = &entry.full_path {
            return Some(p.clone());
        }
        match self {
            Location::Fs(p) => Some(p.join(&entry.name)),
            _ => None,
        }
    }

    /// Path inside the archive for an entry in this archive location.
    pub fn inner_path(&self, name: &str) -> Option<String> {
        match self {
            Location::Archive { inner, .. } if inner.is_empty() => Some(name.to_string()),
            Location::Archive { inner, .. } => Some(format!("{inner}/{name}")),
            _ => None,
        }
    }
}

fn with_trailing_sep(s: &str) -> String {
    if s.ends_with('\\') || s.ends_with('/') {
        s.to_string()
    } else {
        format!("{s}\\")
    }
}

/// `c:` -> `c:\`, strips trailing separators elsewhere, removes `\\?\`.
pub fn normalize_dir(p: &Path) -> PathBuf {
    let mut s = p.to_string_lossy().replace('/', "\\");
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        s = format!(r"\\{rest}");
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        s = rest.to_string();
    }
    while s.len() > 3 && s.ends_with('\\') && !is_unc_root(&s) {
        s.pop();
    }
    if s.len() == 2 && s.ends_with(':') {
        s.push('\\');
    }
    PathBuf::from(s)
}

fn is_unc_root(s: &str) -> bool {
    // "\\server\share\" keeps its trailing separator.
    s.starts_with(r"\\") && s.trim_end_matches('\\').matches('\\').count() <= 3
}

pub fn root_of(p: &Path) -> PathBuf {
    p.ancestors().last().map(normalize_dir).unwrap_or_else(|| p.to_path_buf())
}

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_parent: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub attrs: u32,
    pub selected: bool,
    /// Size of a directory's contents once computed (Space / Ctrl+L).
    pub dir_size: Option<u64>,
    /// Set for entries that live elsewhere (search results, branch view).
    pub full_path: Option<PathBuf>,
    /// Name shown in branch view (relative path); defaults to `name`.
    pub display: Option<String>,
    /// Unix permission bits of entries on SFTP/FTP servers.
    pub mode: Option<u32>,
}

impl FileEntry {
    pub fn parent_entry(modified: Option<SystemTime>, attrs: u32) -> FileEntry {
        FileEntry {
            name: "..".into(),
            is_dir: true,
            is_parent: true,
            size: 0,
            modified,
            attrs,
            selected: false,
            dir_size: None,
            full_path: None,
            display: None,
            mode: None,
        }
    }

    /// Attribute column text: Windows attributes, or `rwxr-xr-x` on servers.
    pub fn attr_text(&self) -> String {
        match self.mode {
            Some(m) => crate::remote::mode_string(m),
            None => format::attr_string(self.attrs),
        }
    }

    pub fn ext(&self) -> &str {
        format::split_ext(&self.name, self.is_dir).1
    }

    pub fn name_shown(&self) -> &str {
        self.display.as_deref().unwrap_or(&self.name)
    }

    pub fn is_hidden(&self) -> bool {
        self.attrs & (ATTR_HIDDEN | ATTR_SYSTEM) != 0
    }

    pub fn is_link(&self) -> bool {
        self.attrs & ATTR_REPARSE != 0
    }

    /// Size used for sorting and totals: files' size, directories' computed size.
    pub fn effective_size(&self) -> u64 {
        if self.is_dir { self.dir_size.unwrap_or(0) } else { self.size }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortColumn {
    Name,
    Ext,
    Size,
    Date,
    Attr,
    Unsorted,
}

impl SortColumn {
    pub fn from_index(i: i64) -> SortColumn {
        match i {
            1 => SortColumn::Ext,
            2 => SortColumn::Size,
            3 => SortColumn::Date,
            4 => SortColumn::Attr,
            5 => SortColumn::Unsorted,
            _ => SortColumn::Name,
        }
    }

    pub fn index(self) -> i64 {
        match self {
            SortColumn::Name => 0,
            SortColumn::Ext => 1,
            SortColumn::Size => 2,
            SortColumn::Date => 3,
            SortColumn::Attr => 4,
            SortColumn::Unsorted => 5,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SortSpec {
    pub column: SortColumn,
    /// Reversed relative to the column's natural direction
    /// (names ascending; size and date largest/newest first).
    pub reverse: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        SortSpec { column: SortColumn::Name, reverse: false }
    }
}

/// Sorts like TC: `..` first, directories before files, directories by name
/// unless sorting by date/size.
pub fn sort_entries(entries: &mut [FileEntry], spec: SortSpec) {
    use std::cmp::Ordering;
    if spec.column == SortColumn::Unsorted {
        return;
    }
    let by_name = |a: &FileEntry, b: &FileEntry| format::natural_cmp(a.name_shown(), b.name_shown());
    entries.sort_by(|a, b| {
        if a.is_parent != b.is_parent {
            return if a.is_parent { Ordering::Less } else { Ordering::Greater };
        }
        if a.is_dir != b.is_dir {
            return if a.is_dir { Ordering::Less } else { Ordering::Greater };
        }
        let ord = match spec.column {
            SortColumn::Name => by_name(a, b),
            SortColumn::Ext if a.is_dir => by_name(a, b),
            SortColumn::Ext => format::natural_cmp(a.ext(), b.ext()).then_with(|| by_name(a, b)),
            SortColumn::Size if a.is_dir && a.dir_size.is_none() && b.dir_size.is_none() => by_name(a, b),
            SortColumn::Size => b.effective_size().cmp(&a.effective_size()).then_with(|| by_name(a, b)),
            SortColumn::Date => b.modified.cmp(&a.modified).then_with(|| by_name(a, b)),
            SortColumn::Attr => a.attr_text().cmp(&b.attr_text())
                .then_with(|| by_name(a, b)),
            SortColumn::Unsorted => Ordering::Equal,
        };
        if spec.reverse { ord.reverse() } else { ord }
    });
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ListOptions {
    pub show_hidden: bool,
    pub show_parent_in_root: bool,
}

/// Reads a local directory. Metadata comes from the directory enumeration
/// itself (FindNextFile data), so no extra syscalls per entry.
pub fn list_fs(dir: &Path, opts: ListOptions) -> std::io::Result<Vec<FileEntry>> {
    use std::os::windows::fs::MetadataExt;
    let mut out = Vec::new();
    let is_root = dir.parent().is_none();
    if !is_root || opts.show_parent_in_root {
        let meta = std::fs::metadata(dir).ok();
        out.push(FileEntry::parent_entry(
            meta.as_ref().and_then(|m| m.modified().ok()),
            meta.map(|m| m.file_attributes()).unwrap_or(ATTR_DIRECTORY),
        ));
    }
    for entry in std::fs::read_dir(dir)? {
        let Ok(entry) = entry else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        let attrs = meta.file_attributes();
        if !opts.show_hidden && attrs & (ATTR_HIDDEN | ATTR_SYSTEM) != 0 {
            continue;
        }
        let is_dir = attrs & ATTR_DIRECTORY != 0;
        out.push(FileEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_dir,
            is_parent: false,
            size: if is_dir { 0 } else { meta.file_size() },
            modified: meta.modified().ok(),
            attrs,
            selected: false,
            dir_size: None,
            full_path: None,
            display: None,
            mode: None,
        });
    }
    Ok(out)
}

/// Lists an archive directory from a cached index.
pub fn list_archive(index: &ArchiveIndex, inner: &str) -> Vec<FileEntry> {
    let mut out = vec![FileEntry::parent_entry(index.modified, ATTR_DIRECTORY)];
    out.extend(index.children(inner));
    out
}

/// Lists search results as entries with absolute paths.
pub fn list_results(items: &[PathBuf]) -> Vec<FileEntry> {
    use std::os::windows::fs::MetadataExt;
    items
        .iter()
        .filter_map(|p| {
            let meta = std::fs::symlink_metadata(p).ok()?;
            let attrs = meta.file_attributes();
            let is_dir = attrs & ATTR_DIRECTORY != 0;
            Some(FileEntry {
                name: p.file_name()?.to_string_lossy().into_owned(),
                is_dir,
                is_parent: false,
                size: if is_dir { 0 } else { meta.file_size() },
                modified: meta.modified().ok(),
                attrs,
                selected: false,
                dir_size: None,
                full_path: Some(p.clone()),
                display: Some(p.to_string_lossy().into_owned()),
                mode: None,
            })
        })
        .collect()
}

/// Recursively lists all files below `dir` (branch view, Ctrl+B).
pub fn list_branch(dir: &Path, opts: ListOptions, limit: usize) -> Vec<FileEntry> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(list) = list_fs(&d, ListOptions { show_parent_in_root: false, ..opts }) else { continue };
        for mut e in list.into_iter().filter(|e| !e.is_parent) {
            let full = d.join(&e.name);
            if e.is_dir {
                if !e.is_link() {
                    stack.push(full);
                }
                continue;
            }
            let rel = full.strip_prefix(dir).unwrap_or(&full).to_string_lossy().into_owned();
            e.display = Some(rel);
            e.full_path = Some(full);
            out.push(e);
            if out.len() >= limit {
                return out;
            }
        }
    }
    out
}

/// Total size and counts of a directory tree; stops early when `cancel` is set.
pub fn dir_size(dir: &Path, cancel: &std::sync::atomic::AtomicBool) -> (u64, u64, u64) {
    use std::os::windows::fs::MetadataExt;
    let (mut bytes, mut files, mut dirs) = (0u64, 0u64, 0u64);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            let attrs = m.file_attributes();
            if attrs & ATTR_DIRECTORY != 0 {
                dirs += 1;
                if attrs & ATTR_REPARSE == 0 {
                    stack.push(e.path());
                }
            } else {
                files += 1;
                bytes += m.file_size();
            }
        }
    }
    (bytes, files, dirs)
}

/// Resolves user input (`..`, `\`, `d:`, relative names, env vars) against a base directory.
pub fn resolve_input_path(base: &Path, input: &str) -> PathBuf {
    let expanded = expand_env(input.trim().trim_matches('"'));
    let s = expanded.replace('/', "\\");
    if s.is_empty() {
        return base.to_path_buf();
    }
    if s == "\\" {
        return root_of(base);
    }
    let p = if s.len() == 2 && s.ends_with(':') {
        PathBuf::from(format!("{s}\\"))
    } else if s.starts_with('\\') && !s.starts_with("\\\\") {
        root_of(base).join(&s[1..])
    } else {
        base.join(&s)
    };
    clean_path(&p)
}

/// Lexically resolves `.` and `..` components.
pub fn clean_path(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::ParentDir => {
                if out.parent().is_some() {
                    out.pop();
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    normalize_dir(&out)
}

/// Expands `%VAR%` references.
pub fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match std::env::var(name) {
                    Ok(v) if !name.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, is_dir: bool, size: u64) -> FileEntry {
        FileEntry {
            name: name.into(),
            is_dir,
            is_parent: false,
            size,
            modified: None,
            attrs: 0,
            selected: false,
            dir_size: None,
            full_path: None,
            display: None,
            mode: None,
        }
    }

    #[test]
    fn sorting_puts_parent_and_dirs_first() {
        let mut v = vec![
            e("b.txt", false, 5),
            e("Zeta", true, 0),
            FileEntry::parent_entry(None, 0),
            e("a.rs", false, 50),
            e("alpha", true, 0),
        ];
        sort_entries(&mut v, SortSpec::default());
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["..", "alpha", "Zeta", "a.rs", "b.txt"]);

        sort_entries(&mut v, SortSpec { column: SortColumn::Size, reverse: false });
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["..", "alpha", "Zeta", "a.rs", "b.txt"]);

        sort_entries(&mut v, SortSpec { column: SortColumn::Ext, reverse: true });
        let names: Vec<_> = v.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["..", "Zeta", "alpha", "b.txt", "a.rs"]);
    }

    #[test]
    fn locations_navigate_up() {
        let loc = Location::fs(r"d:\Work\MCommander");
        assert_eq!(loc.display(), r"d:\Work\MCommander\");
        let (up, name) = loc.parent().unwrap();
        assert_eq!(up.display(), r"d:\Work\");
        assert_eq!(name, "MCommander");
        let root = Location::fs("d:");
        assert_eq!(root.display(), r"d:\");
        assert!(root.parent().is_none());
        assert!(root.parent().is_none());
        assert_eq!(loc.drive_letter(), Some('D'));

        let arc = Location::Archive { archive: r"c:\x\a.zip".into(), inner: "sub/deep".into() };
        assert_eq!(arc.display(), r"c:\x\a.zip\sub\deep\");
        let (up, name) = arc.parent().unwrap();
        assert_eq!(name, "deep");
        let (up2, name2) = up.parent().unwrap();
        assert_eq!(name2, "sub");
        let (up3, name3) = up2.parent().unwrap();
        assert_eq!(name3, "a.zip");
        assert_eq!(up3, Location::fs(r"c:\x"));
    }

    #[test]
    fn input_paths_resolve_like_cd() {
        let base = Path::new(r"d:\Work\MCommander");
        assert_eq!(resolve_input_path(base, ".."), PathBuf::from(r"d:\Work"));
        assert_eq!(resolve_input_path(base, "\\"), PathBuf::from(r"d:\"));
        assert_eq!(resolve_input_path(base, "c:"), PathBuf::from(r"c:\"));
        assert_eq!(resolve_input_path(base, "src"), PathBuf::from(r"d:\Work\MCommander\src"));
        assert_eq!(resolve_input_path(base, r"\Windows"), PathBuf::from(r"d:\Windows"));
        assert_eq!(resolve_input_path(base, r"e:\x\..\y"), PathBuf::from(r"e:\y"));
    }

    #[test]
    fn env_expansion() {
        unsafe { std::env::set_var("MC_TEST_VAR", "hello") };
        assert_eq!(expand_env("%MC_TEST_VAR%\\x"), "hello\\x");
        assert_eq!(expand_env("100%"), "100%");
        assert_eq!(expand_env("%NOPE_NOT_SET%"), "%NOPE_NOT_SET%");
    }

    #[test]
    fn lists_real_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let list = list_fs(dir.path(), ListOptions::default()).unwrap();
        assert!(list[0].is_parent);
        let file = list.iter().find(|e| e.name == "a.txt").unwrap();
        assert_eq!(file.size, 5);
        assert!(list.iter().any(|e| e.name == "sub" && e.is_dir));
        let cancel = std::sync::atomic::AtomicBool::new(false);
        assert_eq!(dir_size(dir.path(), &cancel), (5, 1, 1));
    }
}
