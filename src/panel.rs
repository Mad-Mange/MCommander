//! File panel model: tabs, listing, cursor, selection, history and filters.

use crate::archive::ArchiveIndex;
use crate::config::ViewMode;
use crate::mask::MaskSet;
use crate::vfs::{self, FileEntry, ListOptions, Location, SortSpec};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn other(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// Which files the panel shows (Ctrl+F10/F11/F12).
#[derive(Clone, Debug, PartialEq)]
pub enum FileFilter {
    All,
    Programs,
    Custom(String),
}

/// Caches opened ZIP indexes by path and modification time.
#[derive(Default)]
pub struct ArchiveCache {
    map: HashMap<PathBuf, (Option<SystemTime>, Arc<ArchiveIndex>)>,
}

impl ArchiveCache {
    pub fn get(&mut self, path: &std::path::Path) -> Result<Arc<ArchiveIndex>, String> {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if let Some((t, idx)) = self.map.get(path) {
            if *t == mtime {
                return Ok(idx.clone());
            }
        }
        let idx = Arc::new(ArchiveIndex::open(path).map_err(|e| format!("{}: {e}", path.display()))?);
        self.map.insert(path.to_path_buf(), (mtime, idx.clone()));
        Ok(idx)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub sel_bytes: u64,
    pub total_bytes: u64,
    pub sel_files: usize,
    pub total_files: usize,
    pub sel_dirs: usize,
    pub total_dirs: usize,
}

/// Result of reading a location: entries plus the archive index, if any.
pub type Listing = Result<(Vec<FileEntry>, Option<Arc<ArchiveIndex>>), String>;

static NEXT_TAB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Reads a file-system directory; safe to call from a worker thread.
pub fn read_fs(path: &std::path::Path, branch: bool, opts: ListOptions) -> Listing {
    if branch {
        Ok((vfs::list_branch(path, opts, 200_000), None))
    } else {
        vfs::list_fs(path, opts).map(|v| (v, None)).map_err(|e| format!("{}: {}", path.display(), e))
    }
}

/// Reads a directory on a server; runs on a worker thread.
pub fn read_remote(conn: &crate::remote::Connection, path: &str, opts: ListOptions) -> Listing {
    let list = conn.with(|fs| fs.list(path)).map_err(|e| format!("{}{path}\n\n{e}", conn.url_prefix()))?;
    let mut out = Vec::new();
    if path != "/" {
        out.push(FileEntry::parent_entry(None, crate::format::ATTR_DIRECTORY));
    }
    out.extend(crate::remote::to_file_entries(list, opts.show_hidden));
    Ok((out, None))
}

pub struct Tab {
    /// Stable identity, used to route results of background listings.
    pub id: u64,
    /// Location being read in the background (slow drives, servers); the
    /// old entries stay visible meanwhile.
    pub loading: Option<Location>,
    /// History to restore once a background history jump has finished.
    pub pending_history: Option<(Vec<Location>, usize)>,
    pub loc: Location,
    pub entries: Vec<FileEntry>,
    pub cursor: usize,
    /// First visible row (full view) or first visible column * rows (brief view).
    pub top: usize,
    pub sort: SortSpec,
    pub view: ViewMode,
    pub filter: FileFilter,
    pub quick_filter: Option<String>,
    pub branch: bool,
    pub history: Vec<Location>,
    pub hist_pos: usize,
    pub error: Option<String>,
    pub saved_selection: Vec<String>,
    pub zip: Option<Arc<ArchiveIndex>>,
    /// Anchor for Shift+click range selection.
    pub anchor: Option<usize>,
    /// Scroll so the cursor is visible on the next draw.
    pub follow_cursor: bool,
    /// Rows that fit in the list area (updated while drawing).
    pub page_rows: usize,
    /// Columns of the thumbnails grid (1 in the other views).
    pub grid_cols: usize,
    pub loaded_at: Instant,
    pub locked: bool,
}

impl Tab {
    pub fn new(loc: Location, sort: SortSpec, view: ViewMode) -> Tab {
        Tab {
            id: NEXT_TAB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            loading: None,
            pending_history: None,
            history: vec![loc.clone()],
            loc,
            entries: Vec::new(),
            cursor: 0,
            top: 0,
            sort,
            view,
            filter: FileFilter::All,
            quick_filter: None,
            branch: false,
            hist_pos: 0,
            error: None,
            saved_selection: Vec::new(),
            zip: None,
            anchor: None,
            follow_cursor: true,
            page_rows: 20,
            grid_cols: 1,
            loaded_at: Instant::now(),
            locked: false,
        }
    }

    fn read(&self, loc: &Location, branch: bool, opts: ListOptions, zips: &mut ArchiveCache) -> Listing {
        match loc {
            Location::Fs(p) => read_fs(p, branch, opts),
            Location::Archive { archive, inner } => {
                let idx = zips.get(archive)?;
                Ok((vfs::list_archive(&idx, inner), Some(idx)))
            }
            Location::Results { items, .. } => Ok((vfs::list_results(items), None)),
            Location::Remote { conn, path } => read_remote(conn, path, opts),
        }
    }

    fn apply(&mut self, mut entries: Vec<FileEntry>) {
        let filter = match &self.filter {
            FileFilter::All => None,
            FileFilter::Programs => Some(MaskSet::parse("*.exe;*.com;*.bat;*.cmd;*.ps1;*.msi;*.lnk")),
            FileFilter::Custom(m) => Some(MaskSet::parse(m)),
        };
        let quick = self.quick_filter.as_deref().filter(|q| !q.is_empty()).map(MaskSet::parse_loose);
        if filter.is_some() || quick.is_some() {
            entries.retain(|e| {
                if e.is_parent {
                    return true;
                }
                let by_filter = e.is_dir || filter.as_ref().is_none_or(|f| f.matches(&e.name, false));
                let by_quick = quick.as_ref().is_none_or(|q| q.matches(&e.name, e.is_dir));
                by_filter && by_quick
            });
        }
        vfs::sort_entries(&mut entries, self.sort);
        self.entries = entries;
    }

    /// Changes to a new location. On failure the tab stays where it was.
    pub fn go(&mut self, loc: Location, focus: Option<&str>, opts: ListOptions, zips: &mut ArchiveCache) -> Result<(), String> {
        let branch = self.branch && loc == self.loc;
        let listing = self.read(&loc, branch, opts, zips);
        self.loading = None;
        self.finish_go(loc, listing, focus)
    }

    /// Applies the result of reading `loc` for a navigation.
    pub fn finish_go(&mut self, loc: Location, listing: Listing, focus: Option<&str>) -> Result<(), String> {
        match listing {
            Ok((entries, zip)) => {
                if loc != self.loc {
                    self.branch = false;
                }
                self.zip = zip;
                self.error = None;
                if loc != self.loc {
                    self.quick_filter = None;
                    self.history.truncate(self.hist_pos + 1);
                    if self.history.last() != Some(&loc) {
                        self.history.push(loc.clone());
                    }
                    if self.history.len() > 100 {
                        self.history.remove(0);
                    }
                    self.hist_pos = self.history.len() - 1;
                }
                self.loc = loc;
                self.apply(entries);
                self.cursor = focus.and_then(|f| self.index_of(f)).unwrap_or(0);
                self.top = 0;
                self.anchor = None;
                self.follow_cursor = true;
                self.loaded_at = Instant::now();
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// Re-reads the current location, keeping cursor, selection and computed sizes.
    pub fn refresh(&mut self, opts: ListOptions, zips: &mut ArchiveCache) {
        let listing = self.read(&self.loc.clone(), self.branch, opts, zips);
        self.finish_refresh(listing, opts, zips);
    }

    /// Applies a re-read of the current location.
    pub fn finish_refresh(&mut self, listing: Listing, opts: ListOptions, zips: &mut ArchiveCache) {
        let cursor_name = self.cursor_entry().map(|e| e.name.clone());
        let selected: HashSet<String> = self.entries.iter().filter(|e| e.selected).map(|e| key_of(e)).collect();
        let sizes: HashMap<String, u64> =
            self.entries.iter().filter_map(|e| e.dir_size.map(|s| (key_of(e), s))).collect();
        let old_cursor = self.cursor;
        match listing {
            Ok((mut entries, zip)) => {
                for e in entries.iter_mut() {
                    let k = key_of(e);
                    e.selected = selected.contains(&k);
                    if e.is_dir {
                        e.dir_size = sizes.get(&k).copied();
                    }
                }
                self.zip = zip;
                self.error = None;
                self.apply(entries);
                self.cursor = cursor_name
                    .and_then(|n| self.index_of(&n))
                    .unwrap_or_else(|| old_cursor.min(self.entries.len().saturating_sub(1)));
                self.loaded_at = Instant::now();
            }
            Err(e) => {
                // The directory vanished (e.g. deleted elsewhere): go to the nearest existing parent.
                if let Location::Fs(p) = &self.loc {
                    let mut up = p.clone();
                    while up.pop() {
                        if up.is_dir() {
                            let _ = self.go(Location::fs(up.clone()), None, opts, zips);
                            return;
                        }
                    }
                }
                self.error = Some(e);
            }
        }
    }

    /// Re-sorts / re-filters without touching the disk.
    pub fn resort(&mut self) {
        let cursor_name = self.cursor_entry().map(|e| e.name.clone());
        vfs::sort_entries(&mut self.entries, self.sort);
        self.cursor = cursor_name.and_then(|n| self.index_of(&n)).unwrap_or(0);
        self.follow_cursor = true;
    }

    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.name == name)
            .or_else(|| self.entries.iter().position(|e| e.name.eq_ignore_ascii_case(name)))
    }

    pub fn cursor_entry(&self) -> Option<&FileEntry> {
        self.entries.get(self.cursor)
    }

    pub fn set_cursor(&mut self, i: usize) {
        if self.entries.is_empty() {
            self.cursor = 0;
            return;
        }
        self.cursor = i.min(self.entries.len() - 1);
        self.follow_cursor = true;
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let n = self.entries.len() as isize;
        if n == 0 {
            return;
        }
        let c = (self.cursor as isize + delta).clamp(0, n - 1);
        self.set_cursor(c as usize);
    }

    /// Indices of the selected entries, or the cursor entry when nothing is
    /// selected (never `..`).
    pub fn targets(&self) -> Vec<usize> {
        let sel: Vec<usize> = self.entries.iter().enumerate().filter(|(_, e)| e.selected).map(|(i, _)| i).collect();
        if !sel.is_empty() {
            return sel;
        }
        match self.cursor_entry() {
            Some(e) if !e.is_parent => vec![self.cursor],
            _ => Vec::new(),
        }
    }

    pub fn has_selection(&self) -> bool {
        self.entries.iter().any(|e| e.selected)
    }

    pub fn toggle(&mut self, i: usize) {
        if let Some(e) = self.entries.get_mut(i) {
            if !e.is_parent {
                e.selected = !e.selected;
            }
        }
    }

    pub fn select_range(&mut self, a: usize, b: usize, state: bool) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        for e in self.entries.iter_mut().take(hi + 1).skip(lo) {
            if !e.is_parent {
                e.selected = state;
            }
        }
    }

    pub fn select_all(&mut self, state: bool, include_dirs: bool) {
        for e in self.entries.iter_mut().filter(|e| !e.is_parent) {
            if state && e.is_dir && !include_dirs {
                continue;
            }
            e.selected = state;
        }
    }

    pub fn invert(&mut self, include_dirs: bool) {
        for e in self.entries.iter_mut().filter(|e| !e.is_parent && (include_dirs || !e.is_dir)) {
            e.selected = !e.selected;
        }
    }

    /// Num+ / Num-: (un)select entries matching a mask. Returns how many changed.
    pub fn select_mask(&mut self, spec: &str, state: bool, include_dirs: bool) -> usize {
        let mask = MaskSet::parse(spec);
        let mut n = 0;
        for e in self.entries.iter_mut().filter(|e| !e.is_parent) {
            if e.is_dir && !include_dirs {
                continue;
            }
            if mask.matches(&e.name, e.is_dir) && e.selected != state {
                e.selected = state;
                n += 1;
            }
        }
        n
    }

    pub fn select_same_ext(&mut self, state: bool) {
        let Some(cur) = self.cursor_entry() else { return };
        if cur.is_dir {
            return;
        }
        let ext = cur.ext().to_lowercase();
        for e in self.entries.iter_mut().filter(|e| !e.is_dir) {
            if e.ext().to_lowercase() == ext {
                e.selected = state;
            }
        }
    }

    pub fn save_selection(&mut self) {
        self.saved_selection = self.entries.iter().filter(|e| e.selected).map(|e| e.name.clone()).collect();
    }

    pub fn restore_selection(&mut self) {
        let set: HashSet<&String> = self.saved_selection.iter().collect();
        for e in self.entries.iter_mut() {
            e.selected = set.contains(&e.name);
        }
    }

    pub fn unselect_names(&mut self, names: &[String]) {
        let set: HashSet<&String> = names.iter().collect();
        for e in self.entries.iter_mut() {
            if set.contains(&e.name) {
                e.selected = false;
            }
        }
    }

    pub fn stats(&self) -> Stats {
        let mut s = Stats::default();
        for e in self.entries.iter().filter(|e| !e.is_parent) {
            if e.is_dir {
                s.total_dirs += 1;
                s.total_bytes += e.dir_size.unwrap_or(0);
                if e.selected {
                    s.sel_dirs += 1;
                    s.sel_bytes += e.dir_size.unwrap_or(0);
                }
            } else {
                s.total_files += 1;
                s.total_bytes += e.size;
                if e.selected {
                    s.sel_files += 1;
                    s.sel_bytes += e.size;
                }
            }
        }
        s
    }

    pub fn can_go_back(&self) -> bool {
        self.hist_pos > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.hist_pos + 1 < self.history.len()
    }

    /// Moves through history without adding entries.
    pub fn history_jump(&mut self, pos: usize, opts: ListOptions, zips: &mut ArchiveCache) -> Result<(), String> {
        let Some(loc) = self.history.get(pos).cloned() else { return Ok(()) };
        let hist = self.history.clone();
        // Focus the directory we came from when it is a child of the target.
        let focus = self.loc.parent().filter(|(p, _)| *p == loc).map(|(_, n)| n);
        self.go(loc, focus.as_deref(), opts, zips)?;
        self.history = hist;
        self.hist_pos = pos;
        Ok(())
    }

    pub fn caption(&self) -> String {
        self.loc.caption()
    }
}

fn key_of(e: &FileEntry) -> String {
    match &e.full_path {
        Some(p) => p.to_string_lossy().into_owned(),
        None => e.name.clone(),
    }
}

pub struct Panel {
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// Last visited directory per drive letter (for drive buttons).
    pub drive_dirs: HashMap<char, PathBuf>,
}

impl Panel {
    pub fn new(tab: Tab) -> Panel {
        Panel { tabs: vec![tab], active: 0, drive_dirs: HashMap::new() }
    }

    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    /// The last local directory this tab showed (to return to after a
    /// server is disconnected).
    pub fn last_local(tab: &Tab) -> Option<Location> {
        tab.history.iter().rev().find(|l| matches!(l, Location::Fs(_))).cloned()
    }

    pub fn remember_drive(&mut self) {
        let loc = &self.tabs[self.active].loc;
        if let (Some(letter), Some(dir)) = (loc.drive_letter(), loc.host_dir()) {
            self.drive_dirs.insert(letter, dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab_with_files() -> (tempfile::TempDir, Tab, ArchiveCache) {
        let dir = tempfile::tempdir().unwrap();
        for n in ["b.txt", "a.rs", "c.TXT", "d.md"] {
            std::fs::write(dir.path().join(n), n.as_bytes()).unwrap();
        }
        std::fs::create_dir(dir.path().join("zdir")).unwrap();
        let mut zips = ArchiveCache::default();
        let mut tab = Tab::new(Location::fs(dir.path()), SortSpec::default(), ViewMode::Full);
        tab.go(Location::fs(dir.path()), None, ListOptions::default(), &mut zips).unwrap();
        (dir, tab, zips)
    }

    #[test]
    fn selection_helpers() {
        let (_d, mut tab, _) = tab_with_files();
        let names: Vec<_> = tab.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["..", "zdir", "a.rs", "b.txt", "c.TXT", "d.md"]);
        assert_eq!(tab.select_mask("*.txt", true, false), 2);
        assert_eq!(tab.stats().sel_files, 2);
        tab.invert(false);
        let sel: Vec<_> = tab.entries.iter().filter(|e| e.selected).map(|e| e.name.as_str()).collect();
        assert_eq!(sel, ["a.rs", "d.md"]);
        tab.save_selection();
        tab.select_all(false, true);
        assert!(!tab.has_selection());
        tab.restore_selection();
        assert_eq!(tab.stats().sel_files, 2);
        tab.select_all(true, true);
        assert_eq!(tab.stats().sel_dirs, 1);
        tab.select_all(false, true);
        tab.set_cursor(3);
        tab.select_same_ext(true);
        assert_eq!(tab.targets().len(), 2);
    }

    #[test]
    fn navigation_history_and_refresh() {
        let (d, mut tab, mut zips) = tab_with_files();
        let opts = ListOptions::default();
        let sub = tab.loc.child_dir("zdir");
        tab.go(sub, None, opts, &mut zips).unwrap();
        assert_eq!(tab.entries.len(), 1);
        assert!(tab.can_go_back());
        tab.history_jump(0, opts, &mut zips).unwrap();
        assert_eq!(tab.cursor_entry().unwrap().name, "zdir");
        assert!(tab.can_go_forward());

        // Refresh keeps selection and cursor by name.
        tab.set_cursor(tab.index_of("d.md").unwrap());
        tab.toggle(tab.index_of("b.txt").unwrap());
        std::fs::write(d.path().join("0first.txt"), b"x").unwrap();
        tab.refresh(opts, &mut zips);
        assert_eq!(tab.cursor_entry().unwrap().name, "d.md");
        assert!(tab.entries[tab.index_of("b.txt").unwrap()].selected);
    }

    #[test]
    fn filters() {
        let (_d, mut tab, mut zips) = tab_with_files();
        tab.filter = FileFilter::Custom("*.md".into());
        tab.refresh(ListOptions::default(), &mut zips);
        let names: Vec<_> = tab.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["..", "zdir", "d.md"]);
        tab.filter = FileFilter::All;
        tab.quick_filter = Some("txt".into());
        tab.refresh(ListOptions::default(), &mut zips);
        let names: Vec<_> = tab.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["..", "b.txt", "c.TXT"]);
    }
}
