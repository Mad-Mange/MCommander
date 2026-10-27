//! Separate directory trees beside the file lists (Total Commander's
//! "Separate tree"): one tree that follows the active panel, or one for each
//! panel. Folders are read when they are expanded (network and removable
//! drives on a worker thread), and a tree opens itself down to its panel's
//! directory whenever that changes.

use crate::app::{App, DragState};
use crate::paint_icons::{self, Icon};
use crate::panel::Side;
use crate::vfs::Location;
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

/// Indentation per tree level.
const INDENT: f32 = 16.0;
const SCROLL_W: f32 = 16.0;
/// Keyboard cursor rest time before the panel follows it.
pub const OPEN_DELAY: Duration = Duration::from_millis(400);
/// Hovering a closed folder this long during a drag opens it.
const DRAG_EXPAND: Duration = Duration::from_millis(800);

/// A subfolder as read from disk.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub path: PathBuf,
    pub hidden: bool,
}

/// A top-level entry: a drive, or the share of a UNC path shown in a panel.
#[derive(Clone, Debug, PartialEq)]
pub struct Root {
    pub path: PathBuf,
    pub label: String,
    pub drive: Option<char>,
    /// Read on a worker thread (network, removable and optical drives).
    pub slow: bool,
}

/// A visible line of the tree.
#[derive(Clone, Debug)]
pub struct Row {
    pub path: PathBuf,
    pub depth: usize,
    pub label: String,
    pub hidden: bool,
    pub drive: Option<char>,
}

/// What the keyboard (or a click) asks of the focused tree.
#[derive(Clone, Debug, PartialEq)]
pub enum TreeAction {
    Cursor(isize),
    Page(isize),
    First,
    Last,
    /// Left arrow: close the folder, or go to its parent.
    Left,
    /// Right arrow: open the folder, or go to its first subfolder.
    Right,
    Expand,
    Collapse,
    Toggle,
    /// Show the folder under the cursor in the panel now.
    Open,
    Reload,
    QuickText(String),
    QuickBack,
    QuickStep(isize),
    QuickClose,
    Delete { permanent: bool },
    Transfer { mv: bool },
    Rename,
    MkDir,
    Properties,
    /// Explorer context menu at a position (`None`: at the cursor row).
    ContextMenu(Option<Pos2>),
    ClipCopy,
    ClipCut,
    ClipPaste,
}

pub struct DirTree {
    /// Subfolders by folder; `None` while a worker reads them.
    kids: HashMap<PathBuf, Option<Vec<Node>>>,
    /// Folders being read on a worker thread.
    loading: HashSet<PathBuf>,
    /// Whether a folder has subfolders (for the expander), checked when its
    /// row first becomes visible.
    has_kids: HashMap<PathBuf, bool>,
    expanded: HashSet<PathBuf>,
    /// Folders on the way to the panel's directory: listed even when hidden.
    forced: HashSet<PathBuf>,
    roots: Vec<Root>,
    rows: Vec<Row>,
    dirty: bool,
    pub show_hidden: bool,
    /// Row with the keyboard cursor.
    pub cursor: Option<PathBuf>,
    /// The panel's directory (drawn in red).
    pub current: Option<PathBuf>,
    /// The panel directory this tree last followed.
    pub synced: Option<PathBuf>,
    /// Directory to reveal once a slow folder on its way has been read.
    pending_reveal: Option<PathBuf>,
    pub top: usize,
    pub hscroll: f32,
    /// Bring the cursor into view (`center`: in the middle).
    pub follow: bool,
    pub center: bool,
    pub page_rows: usize,
    /// Widest row drawn last frame (for horizontal scrolling).
    pub content_w: f32,
    /// When the panel follows the keyboard cursor.
    pub open_at: Option<Instant>,
    pub quick: Option<(String, Instant)>,
    /// Row under a drag and since when (opens after a moment).
    pub drag_hover: Option<(PathBuf, Instant)>,
    /// Geometry of the last frame, for drops and keyboard menus.
    pub list_rect: Rect,
    pub row_h: f32,
    ctx: Option<egui::Context>,
    tx: Sender<(PathBuf, Vec<Node>)>,
    rx: Receiver<(PathBuf, Vec<Node>)>,
}

impl Default for DirTree {
    fn default() -> Self {
        let (tx, rx) = channel();
        DirTree {
            kids: HashMap::new(),
            loading: HashSet::new(),
            has_kids: HashMap::new(),
            expanded: HashSet::new(),
            forced: HashSet::new(),
            roots: Vec::new(),
            rows: Vec::new(),
            dirty: true,
            show_hidden: false,
            cursor: None,
            current: None,
            synced: None,
            pending_reveal: None,
            top: 0,
            hscroll: 0.0,
            follow: false,
            center: false,
            page_rows: 20,
            content_w: 0.0,
            open_at: None,
            quick: None,
            drag_hover: None,
            list_rect: Rect::NOTHING,
            row_h: 17.0,
            ctx: None,
            tx,
            rx,
        }
    }
}

/// Paths compared the way Windows does: without regard to case.
pub fn same(a: &Path, b: &Path) -> bool {
    a.as_os_str().eq_ignore_ascii_case(b.as_os_str()) || a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

fn same_name(a: &OsStr, b: &OsStr) -> bool {
    a.eq_ignore_ascii_case(b) || a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

/// `inner` is `outer` or lies inside it (ignoring case).
pub fn within(inner: &Path, outer: &Path) -> bool {
    let (i, o) = (crate::vfs::normalize_dir(inner), crate::vfs::normalize_dir(outer));
    let (i, o) = (i.to_string_lossy().to_lowercase(), o.to_string_lossy().to_lowercase());
    let o = o.trim_end_matches('\\');
    i == o || i.strip_prefix(o).is_some_and(|rest| rest.starts_with('\\'))
}

fn is_hidden_attr(attrs: u32) -> bool {
    attrs & (crate::format::ATTR_HIDDEN | crate::format::ATTR_SYSTEM) != 0
}

/// Subfolders of `dir` (junctions and folder links included), sorted by name.
pub fn read_subdirs(dir: &Path, show_hidden: bool) -> Vec<Node> {
    use std::os::windows::fs::MetadataExt;
    let mut out: Vec<Node> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let a = e.metadata().ok()?.file_attributes();
                    let hidden = is_hidden_attr(a);
                    (a & crate::format::ATTR_DIRECTORY != 0 && (show_hidden || !hidden)).then(|| Node { path: e.path(), hidden })
                })
                .collect()
        })
        .unwrap_or_default();
    sort_nodes(&mut out);
    out
}

fn sort_nodes(nodes: &mut [Node]) {
    let name = |n: &Node| n.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    nodes.sort_by(|a, b| crate::format::natural_cmp(&name(a), &name(b)));
}

/// True when `dir` has at least one (shown) subfolder; stops at the first.
fn has_subdirs(dir: &Path, show_hidden: bool) -> bool {
    use std::os::windows::fs::MetadataExt;
    std::fs::read_dir(dir).is_ok_and(|rd| {
        rd.flatten().any(|e| {
            e.metadata().is_ok_and(|m| {
                let a = m.file_attributes();
                a & crate::format::ATTR_DIRECTORY != 0 && (show_hidden || !is_hidden_attr(a))
            })
        })
    })
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned())
}

impl DirTree {
    pub fn set_show_hidden(&mut self, show: bool) {
        if show != self.show_hidden {
            self.show_hidden = show;
            self.kids.clear();
            self.has_kids.clear();
            self.dirty = true;
        }
    }

    pub fn set_roots(&mut self, roots: &[Root]) {
        if self.roots != roots {
            self.roots = roots.to_vec();
            self.dirty = true;
        }
    }

    fn is_slow(&self, p: &Path) -> bool {
        p.to_string_lossy().starts_with("\\\\") || self.roots.iter().any(|r| r.slow && within(p, &r.path))
    }

    /// Stores a folder's subfolders, adding the hidden ones on the way to
    /// the panel's directory.
    fn store_kids(&mut self, dir: &Path, mut list: Vec<Node>) {
        let missing: Vec<PathBuf> = self
            .forced
            .iter()
            .filter(|f| f.parent() == Some(dir) && !list.iter().any(|n| same(&n.path, f)))
            .cloned()
            .collect();
        if !missing.is_empty() {
            list.extend(missing.into_iter().map(|path| Node { path, hidden: true }));
            sort_nodes(&mut list);
        }
        self.loading.remove(dir);
        self.has_kids.insert(dir.to_path_buf(), !list.is_empty());
        self.kids.insert(dir.to_path_buf(), Some(list));
        self.dirty = true;
    }

    /// Reads a folder (again): local folders right away, slow ones on a
    /// worker thread, keeping what is known meanwhile.
    fn load(&mut self, dir: &Path) {
        if !self.is_slow(dir) {
            let list = read_subdirs(dir, self.show_hidden);
            self.store_kids(dir, list);
            return;
        }
        if !self.loading.insert(dir.to_path_buf()) {
            return;
        }
        self.kids.entry(dir.to_path_buf()).or_insert(None);
        let (tx, path, hidden, ctx) = (self.tx.clone(), dir.to_path_buf(), self.show_hidden, self.ctx.clone());
        std::thread::spawn(move || {
            let list = read_subdirs(&path, hidden);
            let _ = tx.send((path, list));
            if let Some(c) = ctx {
                c.request_repaint();
            }
        });
    }

    fn kids(&mut self, dir: &Path) -> Option<&[Node]> {
        if !self.kids.contains_key(dir) {
            self.load(dir);
        }
        self.kids.get(dir)?.as_deref()
    }

    /// Takes folders read in the background; call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        if self.ctx.is_none() {
            self.ctx = Some(ctx.clone());
        }
        let mut got = false;
        while let Ok((dir, list)) = self.rx.try_recv() {
            self.store_kids(&dir, list);
            got = true;
        }
        if got {
            if let Some(d) = self.pending_reveal.take() {
                self.reveal(&d);
            }
        }
    }

    /// Forgets what is known about `dir` (its folders changed).
    pub fn invalidate(&mut self, dir: &Path) {
        let Some(key) = self.kids.keys().find(|k| same(k, dir)).cloned() else {
            self.has_kids.retain(|k, _| !same(k, dir));
            return;
        };
        self.has_kids.remove(&key);
        if self.is_slow(&key) {
            self.load(&key);
        } else {
            self.kids.remove(&key);
            self.dirty = true;
        }
    }

    /// Reads every open folder again (F2, after file operations).
    pub fn reload(&mut self) {
        self.has_kids.clear();
        let keys: Vec<PathBuf> = self.kids.keys().cloned().collect();
        for k in keys {
            if self.is_slow(&k) {
                self.load(&k);
            } else {
                self.kids.remove(&k);
            }
        }
        self.dirty = true;
    }

    /// The visible rows (rebuilt when something changed).
    pub fn rows(&mut self) -> &[Row] {
        if self.dirty {
            self.dirty = false;
            let roots = self.roots.clone();
            let mut out = Vec::new();
            for r in &roots {
                out.push(Row { path: r.path.clone(), depth: 0, label: r.label.clone(), hidden: false, drive: r.drive });
                if self.expanded.contains(&r.path) {
                    self.push_kids(&r.path, 1, &mut out);
                }
            }
            self.rows = out;
        }
        &self.rows
    }

    fn push_kids(&mut self, dir: &Path, depth: usize, out: &mut Vec<Row>) {
        let Some(kids) = self.kids(dir).map(<[Node]>::to_vec) else { return };
        for k in kids {
            out.push(Row { label: name_of(&k.path), path: k.path.clone(), depth, hidden: k.hidden, drive: None });
            if self.expanded.contains(&k.path) {
                self.push_kids(&k.path, depth + 1, out);
            }
        }
    }

    /// Opens the tree down to `dir` and puts the cursor on it. Names are
    /// matched without regard to case, so the tree's own paths are used.
    pub fn reveal(&mut self, dir: &Path) {
        let dir = crate::vfs::normalize_dir(dir);
        let root = self.roots.iter().filter(|r| within(&dir, &r.path)).max_by_key(|r| r.path.components().count());
        let Some(mut at) = root.map(|r| r.path.clone()) else {
            self.current = None;
            return;
        };
        let skip = at.components().count();
        let names: Vec<&OsStr> = dir.components().skip(skip).filter_map(|c| if let Component::Normal(n) = c { Some(n) } else { None }).collect();
        // Remember the way down first, so hidden folders on it get listed.
        self.forced.clear();
        let mut p = at.clone();
        for n in &names {
            p = p.join(n);
            self.forced.insert(p.clone());
        }
        for name in names {
            self.expanded.insert(at.clone());
            self.dirty = true;
            if self.kids(&at).is_none() {
                // A slow folder is being read: continue when it arrives.
                self.pending_reveal = Some(dir.clone());
                self.cursor = Some(at);
                self.current = None;
                return;
            }
            let kids = self.kids.get(&at).cloned().flatten().unwrap_or_default();
            at = match kids.iter().find(|k| k.path.file_name().is_some_and(|f| same_name(f, name))) {
                Some(k) => k.path.clone(),
                None => {
                    // Created since the folder was read: read it again.
                    self.load(&at);
                    let kids = self.kids.get(&at).cloned().flatten().unwrap_or_default();
                    match kids.iter().find(|k| k.path.file_name().is_some_and(|f| same_name(f, name))) {
                        Some(k) => k.path.clone(),
                        None => at.join(name),
                    }
                }
            };
        }
        self.pending_reveal = None;
        self.current = Some(at.clone());
        self.cursor = Some(at);
        self.follow = true;
        self.center = true;
    }

    /// Whether a folder has subfolders; local folders are checked on demand.
    pub fn has_children(&mut self, dir: &Path) -> bool {
        if let Some(Some(k)) = self.kids.get(dir) {
            return !k.is_empty();
        }
        if let Some(&b) = self.has_kids.get(dir) {
            return b;
        }
        if self.is_slow(dir) {
            return true;
        }
        let b = has_subdirs(dir, self.show_hidden);
        self.has_kids.insert(dir.to_path_buf(), b);
        b
    }

    pub fn is_expanded(&self, dir: &Path) -> bool {
        self.expanded.contains(dir)
    }

    pub fn expand(&mut self, dir: &Path) {
        if self.expanded.insert(dir.to_path_buf()) {
            self.dirty = true;
        }
    }

    pub fn collapse(&mut self, dir: &Path) {
        if self.expanded.remove(dir) {
            self.dirty = true;
            // Keep the cursor on a visible row.
            if self.cursor.as_ref().is_some_and(|c| c != dir && c.starts_with(dir)) {
                self.cursor = Some(dir.to_path_buf());
            }
        }
    }

    pub fn toggle(&mut self, dir: &Path) {
        if self.is_expanded(dir) {
            self.collapse(dir);
        } else {
            self.expand(dir);
        }
    }

    pub fn cursor_index(&mut self) -> Option<usize> {
        let c = self.cursor.clone()?;
        self.rows().iter().position(|r| r.path == c)
    }

    pub fn set_cursor_index(&mut self, i: usize) {
        if let Some(r) = self.rows().get(i) {
            self.cursor = Some(r.path.clone());
            self.follow = true;
        }
    }

    /// Moves the cursor to the next row whose name starts with `text`
    /// (`step` 0: from the cursor row on, +1/-1: the next/previous match).
    pub fn quick_find(&mut self, text: &str, step: isize) -> bool {
        let needle = text.to_lowercase();
        let start = self.cursor_index().unwrap_or(0) as isize;
        let rows = self.rows();
        let n = rows.len() as isize;
        if n == 0 || needle.is_empty() {
            return false;
        }
        let dir = if step < 0 { -1 } else { 1 };
        let first = if step == 0 { 0 } else { dir };
        let hit = (0..n).map(|k| (start + first + k * dir).rem_euclid(n) as usize).find(|&i| rows[i].label.to_lowercase().starts_with(&needle));
        match hit {
            Some(i) => {
                self.set_cursor_index(i);
                true
            }
            None => false,
        }
    }

    /// The folder of the row at a screen position (last frame's layout).
    pub fn path_at(&self, p: Pos2) -> Option<PathBuf> {
        if !self.list_rect.contains(p) {
            return None;
        }
        let i = self.top + ((p.y - self.list_rect.top()) / self.row_h).floor() as usize;
        self.rows.get(i).map(|r| r.path.clone())
    }

    /// Where the cursor row is on screen (for menus opened by keyboard).
    pub fn cursor_pos(&mut self) -> Option<Pos2> {
        let i = self.cursor_index()?;
        let rel = i.checked_sub(self.top)?;
        Some(pos2(self.list_rect.left() + 40.0, self.list_rect.top() + (rel as f32 + 1.0) * self.row_h))
    }
}

// ---- drawing --------------------------------------------------------------------------

/// Draws tree `t`, which follows the panel on `side`, into `rect`.
pub fn draw(app: &mut App, ui: &mut Ui, rect: Rect, t: usize, side: Side) {
    app.tree_rects[t] = rect;
    let pal = app.pal.clone();
    let rh = crate::ui::row_height(app);
    let fnt = FontId::proportional(app.cfg.font_size);
    let roots = app.tree_roots();
    let focused = app.focused_tree() == Some((t, side));
    let dir = app.tab(side).loc.host_dir().filter(|_| !matches!(app.tab(side).loc, Location::Results { .. }));
    let (show_hidden, brackets) = (app.cfg.show_hidden, app.cfg.dir_brackets);
    let dragging = app.drag.as_ref().is_some_and(|d| d.active);

    let tree = &mut app.trees[t];
    tree.set_show_hidden(show_hidden);
    tree.set_roots(&roots);
    if tree.synced != dir {
        tree.synced = dir.clone();
        match &dir {
            Some(d) => tree.reveal(d),
            None => tree.current = None,
        }
    }
    // The panel follows the keyboard cursor after a short rest.
    let mut open = None;
    if let Some(at) = tree.open_at {
        let now = Instant::now();
        if now >= at {
            tree.open_at = None;
            open = tree.cursor.clone();
        } else {
            ui.ctx().request_repaint_after(at - now);
        }
    }
    if tree.quick.as_ref().is_some_and(|(_, at)| at.elapsed() > Duration::from_millis(1500)) {
        tree.quick = None;
    }

    ui.painter().rect_filled(rect, 0.0, pal.list_bg);
    ui.painter().rect_stroke(rect, 0.0, Stroke::new(1.0, pal.border), StrokeKind::Inside);
    let inner = rect.shrink(1.0);
    let n = tree.rows().len();
    let vbar = n as f32 * rh > inner.height();
    let hbar = tree.content_w > inner.width() - if vbar { SCROLL_W } else { 0.0 };
    let list_rect = Rect::from_min_max(inner.min, pos2(inner.right() - if vbar { SCROLL_W } else { 0.0 }, inner.bottom() - if hbar { SCROLL_W } else { 0.0 }));
    let page = ((list_rect.height() / rh).floor() as usize).max(1);
    tree.page_rows = page;
    tree.list_rect = list_rect;
    tree.row_h = rh;
    let max_top = n.saturating_sub(page);
    let max_h = (tree.content_w - list_rect.width()).max(0.0);
    tree.hscroll = tree.hscroll.clamp(0.0, max_h);
    if tree.follow {
        if let Some(ci) = tree.cursor_index() {
            if tree.center && (ci < tree.top || ci >= tree.top + page) {
                tree.top = ci.saturating_sub(page / 2);
            } else if ci < tree.top {
                tree.top = ci;
            } else if ci >= tree.top + page {
                tree.top = ci + 1 - page;
            }
            // Sideways too: deep folders are beyond a narrow tree's edge.
            let row = &tree.rows()[ci];
            let shown = if row.depth > 0 && brackets { format!("[{}]", row.label) } else { row.label.clone() };
            let start = row.depth as f32 * INDENT;
            let end = start + 4.0 + 14.0 + 16.0 + 4.0 + ui.painter().layout_no_wrap(shown, fnt.clone(), pal.text).size().x + 6.0;
            let view = list_rect.width();
            if end - tree.hscroll > view {
                tree.hscroll = end - view;
            }
            if start < tree.hscroll || end - start > view {
                tree.hscroll = start;
            }
        }
        tree.follow = false;
        tree.center = false;
    }
    tree.top = tree.top.min(max_top);

    let resp = ui.interact(list_rect, egui::Id::unique(("septree", t)), Sense::click_and_drag());
    if resp.hovered() {
        let lines = crate::ui::wheel_lines(ui, rh, page);
        if lines != 0 {
            let step = lines.unsigned_abs();
            tree.top = if lines > 0 { tree.top.saturating_sub(step) } else { (tree.top + step).min(max_top) };
        }
    }
    let top = tree.top;
    let hscroll = tree.hscroll;
    let row_at = |p: Pos2| -> Option<usize> {
        if !list_rect.contains(p) {
            return None;
        }
        let i = top + ((p.y - list_rect.top()) / rh).floor() as usize;
        (i < n).then_some(i)
    };
    let x_of = |depth: usize| list_rect.left() + 4.0 + depth as f32 * INDENT - hscroll;
    let visible: Vec<Row> = tree.rows()[top..(top + page + 1).min(n)].to_vec();
    let row_of = |i: usize| -> Option<&Row> { i.checked_sub(top).and_then(|k| visible.get(k)) };

    // Mouse.
    let mut actions: Vec<TreeAction> = Vec::new();
    let mut drag = None;
    let mut focus = false;
    if resp.clicked() || resp.secondary_clicked() || resp.double_clicked() || resp.drag_started() {
        focus = true;
    }
    if let Some(p) = resp.interact_pointer_pos() {
        let hit = row_at(p).and_then(|i| row_of(i).cloned());
        if let Some(row) = &hit {
            let expander = Rect::from_min_size(pos2(x_of(row.depth), list_rect.top() + (row_at(p).unwrap() - top) as f32 * rh), vec2(13.0, rh));
            let on_expander = expander.contains(p);
            if resp.clicked() {
                if on_expander {
                    tree.toggle(&row.path);
                } else {
                    tree.cursor = Some(row.path.clone());
                    tree.open_at = None;
                    open = Some(row.path.clone());
                }
            }
            if resp.double_clicked() && !on_expander {
                tree.toggle(&row.path);
            }
            if resp.secondary_clicked() {
                tree.cursor = Some(row.path.clone());
                actions.push(TreeAction::ContextMenu(Some(p)));
            }
        }
        if resp.drag_started_by(egui::PointerButton::Primary) {
            let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
            if let Some(row) = row_at(origin).and_then(row_of).filter(|r| r.depth > 0) {
                drag = Some((row.path.clone(), p));
            }
        }
    }

    // A drag over the tree: mark the folder under the pointer, open it after a moment.
    let hover = if dragging { ui.input(|i| i.pointer.latest_pos()).and_then(row_at).and_then(|i| row_of(i).cloned()) } else { None };
    match &hover {
        Some(row) => {
            let since = match &tree.drag_hover {
                Some((p, at)) if *p == row.path => *at,
                _ => {
                    tree.drag_hover = Some((row.path.clone(), Instant::now()));
                    Instant::now()
                }
            };
            if since.elapsed() >= DRAG_EXPAND && !tree.is_expanded(&row.path) && tree.has_children(&row.path) {
                tree.expand(&row.path);
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(100));
            }
        }
        None => tree.drag_hover = None,
    }

    // Rows.
    let painter = ui.painter().with_clip_rect(list_rect);
    let cursor = tree.cursor.clone();
    let current = tree.current.clone();
    let mut content_w: f32 = 0.0;
    let mut icons = Vec::new();
    for (k, row) in visible.iter().enumerate() {
        let y = list_rect.top() + k as f32 * rh;
        if y > list_rect.bottom() {
            break;
        }
        let r = Rect::from_min_size(pos2(list_rect.left(), y), vec2(list_rect.width(), rh));
        if focused && cursor.as_ref() == Some(&row.path) {
            painter.rect_filled(r.shrink2(vec2(1.0, 0.0)), 0.0, pal.cursor_fill);
            painter.rect_stroke(r.shrink2(vec2(1.0, 0.0)), 0.0, Stroke::new(1.0, pal.cursor_border), StrokeKind::Inside);
        } else if hover.as_ref().is_some_and(|h| h.path == row.path) {
            painter.rect_stroke(r.shrink(1.0), 0.0, Stroke::new(1.0, pal.accent), StrokeKind::Inside);
        }
        let x = x_of(row.depth);
        if tree.has_children(&row.path) {
            chevron(&painter, pos2(x + 6.0, r.center().y), tree.is_expanded(&row.path), pal.text_dim);
        }
        let icon = Rect::from_min_size(pos2(x + 14.0, r.center().y - 8.0), vec2(16.0, 16.0));
        icons.push((icon, row.drive, row.hidden));
        let label = if row.depth > 0 && brackets { format!("[{}]", row.label) } else { row.label.clone() };
        let is_current = current.as_ref().is_some_and(|c| *c == row.path);
        let color = if is_current { pal.marked } else if row.hidden { pal.text_dim } else { pal.text };
        let galley = painter.layout_no_wrap(label, fnt.clone(), color);
        let tx = icon.right() + 4.0;
        content_w = content_w.max(tx + galley.size().x + 6.0 - list_rect.left() + hscroll);
        painter.galley(pos2(tx, r.center().y - galley.size().y / 2.0), galley, color);
    }
    tree.content_w = content_w;
    let quick = tree.quick.clone();
    let (vpos, hpos) = (tree.top, tree.hscroll);
    for (r, drive, hidden) in icons {
        draw_icon(app, &painter, r, drive, hidden);
    }

    // Scroll bars.
    if vbar {
        let track = Rect::from_min_max(pos2(list_rect.right(), inner.top()), pos2(inner.right(), list_rect.bottom()));
        if let Some(pos) = crate::ui::scroll_track(ui, &pal, egui::Id::unique(("septree-v", t)), track, n, page, true, vpos) {
            app.trees[t].top = pos;
        }
    }
    if hbar {
        let track = Rect::from_min_max(pos2(inner.left(), list_rect.bottom()), pos2(list_rect.right(), inner.bottom()));
        let (total, shown) = (app.trees[t].content_w.ceil() as usize, list_rect.width().floor() as usize);
        if let Some(pos) = crate::ui::scroll_track(ui, &pal, egui::Id::unique(("septree-h", t)), track, total, shown, false, hpos.round() as usize) {
            app.trees[t].hscroll = pos as f32;
        }
    }

    // Quick search box.
    if let (true, Some((text, _))) = (focused, quick) {
        let r = Rect::from_min_size(pos2(list_rect.left() + 4.0, list_rect.bottom() - rh - 6.0), vec2((list_rect.width() - 8.0).max(60.0), rh + 4.0));
        ui.painter().rect(r, 2.0, pal.input_bg, Stroke::new(1.0, pal.accent), StrokeKind::Inside);
        ui.painter().with_clip_rect(r).text(pos2(r.left() + 5.0, r.center().y), Align2::LEFT_CENTER, format!("Search: {text}"), fnt, pal.text);
    }

    if focus {
        app.active = side;
        app.tree_focus = true;
        app.quick = None;
        if app.rename.is_some() {
            app.finish_rename(true);
        }
    }
    if let Some(p) = open {
        app.tree_open(side, &p);
    }
    if let Some((path, start)) = drag {
        // Drag a folder to a panel, another tree folder or out to Explorer.
        app.drag = Some(DragState { side, names: Vec::new(), start, active: false, paths: vec![path] });
    }
    app.tree_actions.extend(actions);
}

/// The ">" / "v" sign in front of folders with subfolders.
fn chevron(painter: &egui::Painter, c: Pos2, open: bool, color: Color32) {
    let s = 3.0;
    let pts = if open {
        vec![pos2(c.x - s, c.y - s / 2.0), pos2(c.x, c.y + s / 2.0), pos2(c.x + s, c.y - s / 2.0)]
    } else {
        vec![pos2(c.x - s / 2.0, c.y - s), pos2(c.x + s / 2.0, c.y), pos2(c.x - s / 2.0, c.y + s)]
    };
    painter.add(egui::Shape::line(pts, Stroke::new(1.3, color)));
}

fn draw_icon(app: &mut App, painter: &egui::Painter, r: Rect, drive: Option<char>, hidden: bool) {
    let tex = match drive {
        Some(letter) => app.icons.for_drive(letter),
        None => app.icons.for_folder(),
    };
    match tex {
        Some(tex) => {
            let tint = if hidden { Color32::from_white_alpha(140) } else { Color32::WHITE };
            painter.image(tex, r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
        }
        None => paint_icons::paint(painter, r, Icon::Folder, app.pal.text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_at(root: &Path, slow: bool) -> DirTree {
        let mut t = DirTree::default();
        t.set_roots(&[Root { path: root.to_path_buf(), label: "root".into(), drive: None, slow }]);
        t
    }

    fn labels(t: &mut DirTree) -> Vec<String> {
        t.rows().iter().map(|r| format!("{}{}", " ".repeat(r.depth), r.label)).collect()
    }

    #[test]
    fn reveal_opens_the_way_and_matches_any_case() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("Alpha").join("Deep")).unwrap();
        std::fs::create_dir_all(d.path().join("beta")).unwrap();
        let mut t = tree_at(d.path(), false);
        t.reveal(&d.path().join("ALPHA").join("deep"));
        let deep = d.path().join("Alpha").join("Deep");
        assert_eq!(t.current.as_deref(), Some(deep.as_path()));
        assert_eq!(t.cursor.as_deref(), Some(deep.as_path()));
        assert_eq!(labels(&mut t), ["root", " Alpha", "  Deep", " beta"]);
        assert!(t.has_children(&d.path().join("Alpha")));
        assert!(!t.has_children(&d.path().join("beta")));
        // Closing a folder takes the cursor out of it.
        t.collapse(&d.path().join("Alpha"));
        assert_eq!(t.cursor.as_deref(), Some(d.path().join("Alpha").as_path()));
        assert_eq!(labels(&mut t), ["root", " Alpha", " beta"]);
        assert_eq!(t.cursor_index(), Some(1));
        assert!(t.quick_find("BE", 0));
        assert_eq!(t.cursor_index(), Some(2));
        assert!(t.quick_find("", 0) == false);
    }

    #[test]
    fn invalidate_and_reload_pick_up_new_folders() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("a")).unwrap();
        let mut t = tree_at(d.path(), false);
        t.reveal(d.path());
        assert_eq!(labels(&mut t), ["root"]);
        t.expand(d.path());
        assert_eq!(labels(&mut t), ["root", " a"]);
        assert!(!t.has_children(&d.path().join("a")));
        std::fs::create_dir(d.path().join("a").join("new")).unwrap();
        std::fs::create_dir(d.path().join("b")).unwrap();
        // Cached until told otherwise.
        assert!(!t.has_children(&d.path().join("a")));
        t.invalidate(&d.path().join("A"));
        assert!(t.has_children(&d.path().join("a")));
        assert_eq!(labels(&mut t), ["root", " a"]);
        t.reload();
        assert_eq!(labels(&mut t), ["root", " a", " b"]);
    }

    #[test]
    fn hidden_folders_on_the_way_are_listed() {
        let d = tempfile::tempdir().unwrap();
        let hid = d.path().join("hid");
        std::fs::create_dir_all(hid.join("inner")).unwrap();
        std::fs::create_dir_all(d.path().join("plain")).unwrap();
        let ok = std::process::Command::new("attrib").arg("+h").arg(&hid).status().is_ok_and(|s| s.success());
        assert!(ok, "attrib +h failed");
        assert_eq!(read_subdirs(d.path(), false).len(), 1);
        assert_eq!(read_subdirs(d.path(), true).len(), 2);
        let mut t = tree_at(d.path(), false);
        t.reveal(&hid.join("inner"));
        assert_eq!(labels(&mut t), ["root", " hid", "  inner", " plain"]);
        assert!(t.rows()[1].hidden);
        // Still there after the folder is read again.
        t.reload();
        assert_eq!(labels(&mut t), ["root", " hid", "  inner", " plain"]);
        t.set_show_hidden(true);
        assert_eq!(labels(&mut t), ["root", " hid", "  inner", " plain"]);
    }

    #[test]
    fn slow_folders_are_read_in_the_background() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("x").join("y")).unwrap();
        let mut t = tree_at(d.path(), true);
        let ctx = egui::Context::default();
        t.reveal(&d.path().join("x").join("y"));
        assert_eq!(t.current, None, "waits for the worker");
        let until = Instant::now() + Duration::from_secs(5);
        while t.current.is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
            t.poll(&ctx);
        }
        assert_eq!(t.current.as_deref(), Some(d.path().join("x").join("y").as_path()));
        assert_eq!(labels(&mut t), ["root", " x", "  y"]);
        // Folders not read yet count as having subfolders.
        assert!(t.has_children(&d.path().join("x").join("y")));
    }

    #[test]
    fn path_comparisons_ignore_case() {
        assert!(same(Path::new(r"C:\Users\A"), Path::new(r"c:\users\a")));
        assert!(within(Path::new(r"c:\users\a\b"), Path::new(r"C:\Users\A")));
        assert!(within(Path::new(r"c:\users"), Path::new(r"C:\")));
        assert!(within(Path::new(r"C:\Users\A"), Path::new(r"c:\users\a\")));
        assert!(!within(Path::new(r"C:\Users\Ab"), Path::new(r"C:\Users\A")));
        assert!(within(Path::new(r"\srv\share\x"), Path::new(r"\SRV\Share\")));
    }
}
