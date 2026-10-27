//! Application state, input routing and command execution.

use crate::commands::{Cmd, KeyCtx, map_key};
use crate::config::{Config, QuickSearchMode, ThemeMode, ViewMode};
use crate::dialogs::{ConfirmAction, Dialog, InputKind};
use crate::icons::IconCache;
use crate::ops::{Dest, Job, OpKind, OpRequest, Source};
use crate::panel::{FileFilter, Panel, Side, Tab, ArchiveCache};
use crate::theme::Palette;
use crate::vfs::{self, FileEntry, ListOptions, Location, SortColumn, SortSpec};
use crate::watch::DirWatcher;
use crate::win;
use egui::{Event, Key, Modifiers, Pos2};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

pub struct QuickSearch {
    pub text: String,
    pub last: Instant,
    /// Stays open until closed explicitly (dialog modes).
    pub sticky: bool,
}

pub struct InlineRename {
    pub side: Side,
    pub index: usize,
    pub original: String,
    pub text: String,
    pub init: bool,
}

/// The path line above a file list while it is edited (click beside the path).
pub struct PathEdit {
    pub side: Side,
    pub text: String,
    /// Focus the field and select its text on the next frame.
    pub init: bool,
}

pub struct DragState {
    pub side: Side,
    pub names: Vec<String>,
    pub start: Pos2,
    pub active: bool,
}

/// Right mouse button held on a file list (Total Commander's selection mode):
/// a short click toggles the entry, dragging paints the same state over more
/// entries and holding still opens the context menu.
pub struct RightSelect {
    pub side: Side,
    /// Entry under the press; `None` on ".." or the empty area.
    pub start: Option<usize>,
    /// Last entry the selection state was applied to.
    pub last: Option<usize>,
    pub state: bool,
    pub was_selected: bool,
    pub at: Instant,
    pub pos: Pos2,
    /// The pointer reached another entry: no context menu any more.
    pub moved: bool,
}

pub struct PopupItem {
    pub label: String,
    pub detail: String,
    pub cmd: Option<Cmd>,
}

pub struct PopupMenu {
    pub pos: Pos2,
    pub items: Vec<PopupItem>,
    pub sel: usize,
    pub opened: Instant,
    /// Set when the popup shows a main menu (Left/Right switch menus).
    pub menu: Option<usize>,
}

pub struct Toast {
    pub text: String,
    pub at: Instant,
    pub error: bool,
}

#[derive(Clone, Default)]
pub struct DriveExtra {
    pub label: Option<String>,
    pub space: Option<(u64, u64)>,
}

pub enum BgMsg {
    DirSize { side: Side, loc: Location, name: String, size: u64 },
    DriveInfo { letter: char, label: Option<String>, space: Option<(u64, u64)> },
    Occupied { bytes: u64, files: u64, dirs: u64 },
    Message { title: String, text: String, error: bool },
    Refresh,
    /// A background directory listing finished (slow drives, servers).
    Listed { tab_id: u64, loc: Location, listing: crate::panel::Listing, focus: Option<String>, refresh: bool },
    /// A connection attempt finished (session and login directory).
    Connected {
        attempt: u64,
        req: Box<crate::remote_ui::ConnectRequest>,
        result: Result<(Box<dyn crate::remote::RemoteFs>, String), crate::remote::ConnectError>,
    },
    /// A quick server command (rename, make directory) finished.
    RemoteDone { side: Side, focus: Option<String>, error: Option<String> },
}

pub enum Preview {
    Text(String),
    Image(egui::TextureHandle, [usize; 2]),
    Info(String),
}

/// Scripted input for automated UI tests (`--keys`, `--screenshot`).
pub enum Step {
    Key(Key, Modifiers),
    Text(String),
    Raw(win::RawKey),
    Wait(u64),
    Shot(PathBuf),
    ShotChild(PathBuf),
    ShotSearch(PathBuf),
    /// Mouse button press or release at a position in points; `click`
    /// releases the button again right after the press.
    Mouse { pos: Pos2, button: egui::PointerButton, pressed: bool, click: bool },
    /// Pointer movement (for drags).
    Move(Pos2),
    Exit,
}

pub struct App {
    pub cfg: Config,
    pub pal: Palette,
    pub panels: [Panel; 2],
    pub active: Side,
    pub zips: ArchiveCache,
    pub icons: IconCache,
    pub thumbs: crate::thumbs::ThumbCache,
    pub cmdline: String,
    /// The command line has the keyboard (letter quick-search modes).
    pub cmd_active: bool,
    pub pending: Vec<Cmd>,
    pub dialog: Option<Dialog>,
    pub popup: Option<PopupMenu>,
    pub jobs: Vec<Job>,
    next_job: u64,
    pub viewers: Vec<crate::viewer::ViewerWindow>,
    next_viewer: u64,
    pub search: Option<crate::search::SearchHandle>,
    pub quick: Option<QuickSearch>,
    pub rename: Option<InlineRename>,
    pub path_edit: Option<PathEdit>,
    pub drag: Option<DragState>,
    pub right_sel: Option<RightSelect>,
    /// The bar under the menu and the vertical bar.
    pub bars: [crate::buttonbar::ButtonBar; 2],
    /// Where each button's picture comes from (by icon and command).
    pub bar_icons: HashMap<(String, String), crate::buttonbar::IconSource>,
    /// Button rectangles drawn this frame, for drops and menus.
    pub bar_hits: Vec<(crate::buttonbar::BarKind, usize, egui::Rect)>,
    pub bar_areas: [egui::Rect; 2],
    /// Below the last clicked button: where a sub-bar menu opens.
    pub bar_anchor: Option<Pos2>,
    /// Commands waiting for a `cm_wait` in a button.
    pub delayed: Vec<(Instant, Cmd)>,
    pub drives: Vec<win::DriveInfo>,
    drives_at: Instant,
    pub drive_extra: HashMap<char, DriveExtra>,
    drive_requested: HashMap<char, Instant>,
    pub bg_tx: Sender<BgMsg>,
    bg_rx: Receiver<BgMsg>,
    watcher: DirWatcher,
    pub hwnd: Option<windows::Win32::Foundation::HWND>,
    pub toast: Option<Toast>,
    pub quick_view: bool,
    pub preview: Option<(PathBuf, Preview)>,
    pub panel_rects: [egui::Rect; 2],
    pub list_rects: [egui::Rect; 2],
    pub dark: bool,
    applied_dark: Option<bool>,
    pub steps: VecDeque<Step>,
    step_at: Instant,
    pub size_cancel: Arc<AtomicBool>,
    pub window_rect: Option<egui::Rect>,
    pub maximized: bool,
    pub cmd_history_popup: bool,
    pub frame_no: u64,
    /// Where a pending context menu should open (pointer position, points).
    pub context_at: Option<Pos2>,
    /// The pending context menu is for the folder itself (a click on the
    /// empty area or on ".."): Explorer's background menu with "New".
    pub context_background: bool,
    /// Screen rectangles of the main menu titles (for keyboard menus).
    pub menu_rects: Vec<egui::Rect>,
    /// A modal loop (OLE drag, shell context menu) swallowed this button's
    /// mouse-up: synthesize it next frame.
    pub release_pointer: Option<egui::PointerButton>,
    pub ppp: f32,
    /// Id of the connection attempt in progress (results of others are dropped).
    pub connecting: Option<u64>,
    pub connect_attempt: u64,
    /// Connections made with Ctrl+N (offered for saving on disconnect).
    pub adhoc_conns: HashSet<u64>,
    /// Server files being edited locally (uploaded back when saved).
    pub remote_edits: Vec<crate::remote_ui::RemoteEdit>,
    pub edits_checked: Instant,
    /// What to do when a job finishes (open a downloaded file, ...).
    pub job_after: HashMap<u64, crate::remote_ui::After>,
    /// Entry to put the cursor on after the next re-read of a panel.
    pub refresh_focus: Option<(Side, String)>,
}

pub struct Options {
    pub config: PathBuf,
    pub left: Option<String>,
    pub right: Option<String>,
    pub steps: VecDeque<Step>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, cfg: Config, opts: Options) -> App {
        let ctx = cc.egui_ctx.clone();
        setup_fonts(&ctx, cfg.font_size);
        let hwnd = win::hwnd_of(cc);
        if let Some(h) = hwnd {
            win::install_key_hook(h, &ctx);
        }
        let (bg_tx, bg_rx) = channel();
        let make_panel = |pc: &crate::config::PanelConfig, override_path: Option<&String>| {
            let mut tabs: Vec<Tab> = pc
                .tabs
                .iter()
                .map(|p| Tab::new(Location::fs(p), pc.sort, pc.view))
                .collect();
            if let Some(p) = override_path {
                let i = pc.active_tab.min(tabs.len() - 1);
                tabs[i] = Tab::new(Location::fs(p), pc.sort, pc.view);
            }
            let mut panel = Panel::new(tabs.remove(0));
            panel.tabs.extend(tabs);
            panel.active = pc.active_tab.min(panel.tabs.len() - 1);
            panel
        };
        let panels = [make_panel(&cfg.left, opts.left.as_ref()), make_panel(&cfg.right, opts.right.as_ref())];
        let mut app = App {
            pal: Palette::light(),
            panels,
            active: if cfg.active_right { Side::Right } else { Side::Left },
            zips: ArchiveCache::default(),
            icons: IconCache::new(ctx.clone()),
            thumbs: crate::thumbs::ThumbCache::new(ctx.clone()),
            cmdline: String::new(),
            cmd_active: false,
            pending: Vec::new(),
            dialog: None,
            popup: None,
            jobs: Vec::new(),
            next_job: 1,
            viewers: Vec::new(),
            next_viewer: 1,
            search: None,
            quick: None,
            rename: None,
            path_edit: None,
            drag: None,
            right_sel: None,
            bars: [crate::buttonbar::ButtonBar::default_main(), crate::buttonbar::ButtonBar::default_vertical()],
            bar_icons: HashMap::new(),
            bar_hits: Vec::new(),
            bar_areas: [egui::Rect::NOTHING; 2],
            bar_anchor: None,
            delayed: Vec::new(),
            drives: win::drives(),
            drives_at: Instant::now(),
            drive_extra: HashMap::new(),
            drive_requested: HashMap::new(),
            bg_tx,
            bg_rx,
            watcher: DirWatcher::new(ctx.clone()),
            hwnd,
            toast: None,
            quick_view: false,
            preview: None,
            panel_rects: [egui::Rect::NOTHING; 2],
            list_rects: [egui::Rect::NOTHING; 2],
            dark: false,
            applied_dark: None,
            steps: opts.steps,
            step_at: Instant::now() + Duration::from_millis(400),
            size_cancel: Arc::new(AtomicBool::new(false)),
            window_rect: None,
            maximized: cfg.maximized,
            cmd_history_popup: false,
            frame_no: 0,
            context_at: None,
            context_background: false,
            menu_rects: Vec::new(),
            release_pointer: None,
            ppp: 1.0,
            connecting: None,
            connect_attempt: 0,
            adhoc_conns: HashSet::new(),
            remote_edits: Vec::new(),
            edits_checked: Instant::now(),
            job_after: HashMap::new(),
            refresh_focus: None,
            cfg,
        };
        let lo = app.list_opts();
        for side in [Side::Left, Side::Right] {
            for i in 0..app.panels[side.index()].tabs.len() {
                let loc = app.panels[side.index()].tabs[i].loc.clone();
                if app.is_slow(&loc) {
                    // Network or removable drive: don't block startup.
                    app.spawn_listing(side, i, loc, None, false);
                    continue;
                }
                let tab = &mut app.panels[side.index()].tabs[i];
                if tab.go(loc.clone(), None, lo, &mut app.zips).is_err() {
                    // Saved path no longer exists: fall back to the nearest existing parent.
                    let mut p = loc.fs_path().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("C:\\"));
                    while !p.is_dir() && p.pop() {}
                    if !p.is_dir() {
                        p = PathBuf::from("C:\\");
                    }
                    let _ = tab.go(Location::fs(p), None, lo, &mut app.zips);
                }
            }
            app.panels[side.index()].remember_drive();
        }
        app.update_watches();
        app.load_bars();
        if let Some(src) = app.cfg.imported_from.clone().filter(|_| !app.cfg.file.exists()) {
            // First start: Total Commander's button bars come along.
            if let Ok(ini) = crate::ini::Ini::load(Path::new(&src)) {
                app.import_tc_bars(Path::new(&src), &ini);
            }
            app.toast(format!("Imported settings from {src}"));
            let _ = app.cfg.save();
        }
        app
    }

    // ---- small accessors --------------------------------------------------

    pub fn list_opts(&self) -> ListOptions {
        ListOptions { show_hidden: self.cfg.show_hidden, show_parent_in_root: false }
    }

    pub fn tab(&self, side: Side) -> &Tab {
        self.panels[side.index()].tab()
    }

    pub fn tab_mut(&mut self, side: Side) -> &mut Tab {
        self.panels[side.index()].tab_mut()
    }

    pub fn atab(&self) -> &Tab {
        self.tab(self.active)
    }

    pub fn atab_mut(&mut self) -> &mut Tab {
        let s = self.active;
        self.tab_mut(s)
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast { text: text.into(), at: Instant::now(), error: false });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.dialog = Some(Dialog::Message { title: "MCommander".into(), text: text.into(), error: true });
    }

    pub fn panel_keys_active(&self) -> bool {
        self.dialog.is_none() && self.popup.is_none() && self.rename.is_none() && self.path_edit.is_none() && !self.jobs.iter().any(|j| !j.background)
    }

    // ---- navigation ---------------------------------------------------------

    /// Locations whose listing may take long (network shares, removable and
    /// optical drives); these are read on a worker thread.
    pub fn is_slow(&self, loc: &Location) -> bool {
        if let Location::Remote { .. } = loc {
            return true;
        }
        let Location::Fs(p) = loc else { return false };
        if p.to_string_lossy().starts_with("\\\\") {
            return true;
        }
        loc.drive_letter().is_some_and(|l| {
            self.drives.iter().any(|d| {
                d.letter == l && matches!(d.kind, win::DriveKind::Network | win::DriveKind::Removable | win::DriveKind::Cdrom | win::DriveKind::Unknown)
            })
        })
    }

    /// Reads `loc` for tab `index` of `side` in the background.
    pub fn spawn_listing(&mut self, side: Side, index: usize, loc: Location, focus: Option<String>, refresh: bool) {
        if !matches!(loc, Location::Fs(_) | Location::Remote { .. }) {
            return;
        }
        let opts = self.list_opts();
        let tab = &mut self.panels[side.index()].tabs[index];
        let branch = tab.branch && loc == tab.loc;
        tab.loading = Some(loc.clone());
        let tab_id = tab.id;
        let tx = self.bg_tx.clone();
        let ctx = self.ctx_clone();
        std::thread::spawn(move || {
            let listing = match &loc {
                Location::Remote { conn, path } => crate::panel::read_remote(conn, path, opts),
                Location::Fs(path) => crate::panel::read_fs(path, branch, opts),
                _ => return,
            };
            let _ = tx.send(BgMsg::Listed { tab_id, loc, listing, focus, refresh });
            ctx.request_repaint();
        });
    }

    fn finish_listing(&mut self, tab_id: u64, loc: Location, listing: crate::panel::Listing, focus: Option<String>, refresh: bool) {
        let lo = self.list_opts();
        let mut failed = None;
        for side in [Side::Left, Side::Right] {
            let Some(tab) = self.panels[side.index()].tabs.iter_mut().find(|t| t.id == tab_id) else { continue };
            if tab.loading.as_ref() != Some(&loc) {
                return; // Cancelled or superseded by another navigation.
            }
            tab.loading = None;
            if refresh {
                tab.finish_refresh(listing, lo, &mut self.zips);
            } else {
                let history = tab.pending_history.take();
                match tab.finish_go(loc, listing, focus.as_deref()) {
                    Ok(()) => {
                        if let Some((h, pos)) = history {
                            tab.history = h;
                            tab.hist_pos = pos;
                        }
                    }
                    Err(e) => {
                        if tab.entries.is_empty() {
                            // Nothing shown yet (startup): fall back to the system drive.
                            let _ = tab.go(Location::fs("C:\\"), None, lo, &mut self.zips);
                        }
                        failed = Some(e);
                    }
                }
            }
            self.panels[side.index()].remember_drive();
            break;
        }
        self.update_watches();
        if let Some(e) = failed {
            self.error(e);
        }
    }

    pub fn go(&mut self, side: Side, loc: Location, focus: Option<&str>) {
        let lo = self.list_opts();
        if self.is_slow(&loc) && !self.tab(side).locked {
            if self.rename.as_ref().is_some_and(|r| r.side == side) {
                self.rename = None;
            }
            let index = self.panels[side.index()].active;
            self.spawn_listing(side, index, loc, focus.map(str::to_string), false);
            return;
        }
        let tab = self.panels[side.index()].tab_mut();
        if tab.locked && loc != tab.loc {
            // Locked tabs open a new tab instead of navigating away.
            let mut t = Tab::new(loc.clone(), tab.sort, tab.view);
            match t.go(loc, focus, lo, &mut self.zips) {
                Ok(()) => {
                    let p = &mut self.panels[side.index()];
                    p.tabs.insert(p.active + 1, t);
                    p.active += 1;
                }
                Err(e) => self.error(e),
            }
        } else if let Err(e) = tab.go(loc, focus, lo, &mut self.zips) {
            self.error(e);
            return;
        }
        if self.rename.as_ref().is_some_and(|r| r.side == side) {
            self.rename = None;
        }
        self.panels[side.index()].remember_drive();
        self.update_watches();
    }

    pub fn refresh(&mut self, side: Side) {
        let lo = self.list_opts();
        let loc = self.tab(side).loc.clone();
        if self.is_slow(&loc) {
            if self.tab(side).loading.is_none() {
                let index = self.panels[side.index()].active;
                self.spawn_listing(side, index, loc, None, true);
            }
            return;
        }
        self.panels[side.index()].tab_mut().refresh(lo, &mut self.zips);
        self.update_watches();
    }

    pub fn refresh_all(&mut self) {
        self.refresh(Side::Left);
        self.refresh(Side::Right);
    }

    fn update_watches(&mut self) {
        for side in [Side::Left, Side::Right] {
            let dir = match &self.tab(side).loc {
                Location::Fs(p) if !self.tab(side).branch => Some(p.clone()),
                _ => None,
            };
            self.watcher.watch(side.index(), dir.as_deref());
        }
    }

    pub fn goto_drive(&mut self, side: Side, letter: char) {
        let dir = self.panels[side.index()]
            .drive_dirs
            .get(&letter)
            .filter(|d| d.is_dir())
            .cloned()
            .unwrap_or_else(|| PathBuf::from(format!("{letter}:\\")));
        let target = if dir.is_dir() { dir } else { PathBuf::from(format!("{letter}:\\")) };
        self.go(side, Location::fs(target), None);
        self.active = side;
    }

    /// Opens the entry at `index` (Enter / double click).
    pub fn open_entry(&mut self, side: Side, index: usize, force_archive: bool) {
        let tab = self.tab(side);
        let Some(entry) = tab.entries.get(index).cloned() else { return };
        let loc = tab.loc.clone();
        if entry.is_parent {
            self.go_parent(side);
            return;
        }
        if let Some(full) = &entry.full_path {
            // Search results / branch view.
            if entry.is_dir {
                self.go(side, Location::fs(full), None);
            } else if force_archive || crate::archive::is_archive_name(&entry.name) {
                self.go(side, Location::Archive { archive: full.clone(), inner: String::new() }, None);
            } else {
                self.shell_open(full);
            }
            return;
        }
        if entry.is_dir {
            self.go(side, loc.child_dir(&entry.name), None);
            return;
        }
        match &loc {
            Location::Fs(dir) => {
                let path = dir.join(&entry.name);
                if force_archive || crate::archive::is_archive_name(&entry.name) {
                    self.go(side, Location::Archive { archive: path, inner: String::new() }, None);
                } else {
                    self.shell_open(&path);
                }
            }
            Location::Archive { .. } => {
                if let Some(p) = self.materialize(side, index) {
                    self.shell_open(&p);
                }
            }
            Location::Remote { .. } => self.fetch_then(side, index, "open"),
            Location::Results { .. } => {}
        }
    }

    pub fn open_viewer(&mut self, path: PathBuf) {
        let id = self.next_viewer;
        self.next_viewer += 1;
        self.viewers.push(crate::viewer::ViewerWindow::open(id, path, &self.pal));
    }

    pub fn shell_open(&mut self, path: &Path) {
        if let Err(e) = win::shell_open(path, path.parent()) {
            self.error(format!("{}\n\n{e}", path.display()));
        }
    }

    pub fn go_parent(&mut self, side: Side) {
        if self.tab(side).branch {
            self.tab_mut(side).branch = false;
            self.refresh(side);
            return;
        }
        if let Some((up, name)) = self.tab(side).loc.parent() {
            self.go(side, up, Some(&name));
        } else if let Location::Results { .. } = self.tab(side).loc {
            self.history_step(side, -1);
        }
    }

    fn history_step(&mut self, side: Side, delta: isize) {
        let lo = self.list_opts();
        let target = {
            let tab = self.panels[side.index()].tab();
            let pos = tab.hist_pos as isize + delta;
            if pos < 0 || pos as usize >= tab.history.len() {
                return;
            }
            (pos as usize, tab.history[pos as usize].clone())
        };
        if self.is_slow(&target.1) {
            // Read in the background; the history is restored afterwards.
            let index = self.panels[side.index()].active;
            let tab = self.panels[side.index()].tab_mut();
            let focus = tab.loc.parent().filter(|(p, _)| *p == target.1).map(|(_, n)| n);
            tab.pending_history = Some((tab.history.clone(), target.0));
            self.spawn_listing(side, index, target.1, focus, false);
            return;
        }
        let tab = self.panels[side.index()].tab_mut();
        let pos = tab.hist_pos as isize + delta;
        if let Err(e) = tab.history_jump(pos as usize, lo, &mut self.zips) {
            self.error(e);
        }
        self.update_watches();
    }

    /// Full path of an entry on disk; archive members are extracted to a
    /// temporary folder first (for viewing, editing and opening).
    pub fn materialize(&mut self, side: Side, index: usize) -> Option<PathBuf> {
        let tab = self.tab(side);
        let entry = tab.entries.get(index)?.clone();
        if let Some(p) = tab.loc.child_path(&entry) {
            return Some(p);
        }
        let Location::Archive { archive, .. } = &tab.loc else { return None };
        let inner = tab.loc.inner_path(&entry.name)?;
        let idx = tab.zip.clone()?;
        let tmp = std::env::temp_dir()
            .join("MCommander")
            .join(format!("{:x}", fxhash(&format!("{}|{}", archive.display(), inner))));
        let target = tmp.join(&entry.name);
        let result = std::fs::create_dir_all(&tmp).and_then(|_| crate::archive::extract_one(&idx, &inner, &target));
        match result {
            Ok(()) => Some(target),
            Err(e) => {
                self.error(format!("Could not extract {}: {e}", entry.name));
                None
            }
        }
    }

    // ---- sources for file operations -----------------------------------------

    pub fn sources(&self, side: Side) -> Vec<Source> {
        let tab = self.tab(side);
        tab.targets()
            .into_iter()
            .filter_map(|i| {
                let e = &tab.entries[i];
                match &tab.loc {
                    _ if e.full_path.is_some() => Some(Source::Fs(e.full_path.clone().unwrap())),
                    Location::Fs(d) => Some(Source::Fs(d.join(&e.name))),
                    Location::Archive { archive, .. } => Some(Source::Archive {
                        archive: archive.clone(),
                        inner: tab.loc.inner_path(&e.name)?,
                        is_dir: e.is_dir,
                    }),
                    Location::Remote { conn, path } => Some(Source::Remote {
                        conn: conn.clone(),
                        path: crate::remote::join(path, &e.name),
                        is_dir: e.is_dir,
                        is_link: e.is_link(),
                        size: e.size,
                        modified: e.modified,
                    }),
                    Location::Results { .. } => None,
                }
            })
            .collect()
    }

    fn describe(&self, side: Side) -> String {
        let tab = self.tab(side);
        let t = tab.targets();
        if t.len() == 1 {
            let e = &tab.entries[t[0]];
            format!("\"{}\"", e.name_shown())
        } else {
            let files = t.iter().filter(|&&i| !tab.entries[i].is_dir).count();
            let dirs = t.len() - files;
            format!("{files} file(s), {dirs} director(y/ies)")
        }
    }

    /// Starts a background file operation; returns its id.
    pub fn start_job(&mut self, req: OpRequest) -> u64 {
        for side in [Side::Left, Side::Right] {
            self.tab_mut(side).save_selection();
        }
        let id = self.next_job;
        self.next_job += 1;
        // Stop watching while a big operation floods change notifications;
        // panels are refreshed when the job finishes.
        self.jobs.push(Job::start(id, req, self.ctx_clone()));
        id
    }

    fn ctx_clone(&self) -> egui::Context {
        crate::CTX.get().cloned().unwrap_or_default()
    }

    fn poll_jobs(&mut self) {
        let mut finished = Vec::new();
        for (i, job) in self.jobs.iter().enumerate() {
            let p = job.progress();
            if p.finished {
                finished.push((i, p));
            }
        }
        for (i, p) in finished.into_iter().rev() {
            let job = self.jobs.remove(i);
            // Unselect what was processed, in whichever panel shows the sources.
            for side in [Side::Left, Side::Right] {
                self.tab_mut(side).unselect_names(&p.done_sources);
            }
            let after = self.job_after.remove(&job.id);
            if let (Some(after), true) = (after, p.errors.is_empty() && !p.cancelled) {
                // A temporary download for viewing: nothing to refresh.
                self.run_after(after);
                continue;
            }
            self.refresh_all();
            if !p.errors.is_empty() {
                let mut text = p.errors.iter().take(20).cloned().collect::<Vec<_>>().join("\n");
                if p.errors.len() > 20 {
                    text.push_str(&format!("\n... and {} more", p.errors.len() - 20));
                }
                self.dialog = Some(Dialog::Message { title: format!("{} - errors", p.title), text, error: true });
            } else if p.cancelled {
                self.toast("Operation cancelled");
            } else if job.background {
                self.toast(format!("{} finished", p.title));
            }
        }
    }

    // ---- background helpers ------------------------------------------------------

    pub fn calc_dir_sizes(&mut self, side: Side, names: Vec<String>) {
        let Location::Fs(dir) = self.tab(side).loc.clone() else { return };
        let tx = self.bg_tx.clone();
        let loc = self.tab(side).loc.clone();
        let cancel = self.size_cancel.clone();
        cancel.store(false, Ordering::Relaxed);
        let ctx = self.ctx_clone();
        std::thread::spawn(move || {
            for name in names {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let (bytes, _, _) = vfs::dir_size(&dir.join(&name), &cancel);
                let _ = tx.send(BgMsg::DirSize { side, loc: loc.clone(), name, size: bytes });
                ctx.request_repaint();
            }
        });
    }

    pub fn request_drive_info(&mut self, letter: char) {
        let due = self.drive_requested.get(&letter).is_none_or(|t| t.elapsed() > Duration::from_secs(5));
        if !due {
            return;
        }
        self.drive_requested.insert(letter, Instant::now());
        let tx = self.bg_tx.clone();
        let ctx = self.ctx_clone();
        std::thread::spawn(move || {
            let root = PathBuf::from(format!("{letter}:\\"));
            let label = win::volume_label(&root);
            let space = win::disk_space(&root);
            let _ = tx.send(BgMsg::DriveInfo { letter, label, space });
            ctx.request_repaint();
        });
    }

    fn poll_background(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.bg_rx.try_recv() {
            match msg {
                BgMsg::DirSize { side, loc, name, size } => {
                    let tab = self.tab_mut(side);
                    if tab.loc == loc {
                        if let Some(i) = tab.index_of(&name) {
                            tab.entries[i].dir_size = Some(size);
                        }
                    }
                }
                BgMsg::DriveInfo { letter, label, space } => {
                    self.drive_extra.insert(letter, DriveExtra { label, space });
                }
                BgMsg::Occupied { bytes, files, dirs } => {
                    if let Some(Dialog::Occupied { result, .. }) = &mut self.dialog {
                        *result = Some((bytes, files, dirs));
                    }
                }
                BgMsg::Message { title, text, error } => {
                    self.dialog = Some(Dialog::Message { title, text, error });
                }
                BgMsg::Refresh => self.refresh_all(),
                BgMsg::Listed { tab_id, loc, listing, focus, refresh } => self.finish_listing(tab_id, loc, listing, focus, refresh),
                BgMsg::Connected { attempt, req, result } => self.finish_connect(attempt, *req, result),
                BgMsg::RemoteDone { side, focus, error } => {
                    if let Some(e) = error {
                        self.error(e);
                    }
                    self.refresh_focus = focus.map(|f| (side, f));
                    self.refresh(side);
                }
            }
        }
        self.check_remote_edits();
        if let Some((side, name)) = self.refresh_focus.clone() {
            // Select the renamed/created entry once the re-read arrived.
            if self.tab(side).loading.is_none() {
                self.refresh_focus = None;
                if let Some(i) = self.tab(side).index_of(&name) {
                    self.tab_mut(side).set_cursor(i);
                }
            }
        }
        if self.drives_at.elapsed() > Duration::from_secs(2) {
            self.drives = win::drives();
            self.drives_at = Instant::now();
        }
        let dirty = self.watcher.poll();
        for side in [Side::Left, Side::Right] {
            if dirty[side.index()] && self.rename.as_ref().is_none_or(|r| r.side != side) {
                self.refresh(side);
            }
        }
        self.icons.poll(ctx);
        self.thumbs.poll(ctx);
        self.poll_jobs();
        if let Some(t) = &self.toast {
            if t.at.elapsed() > Duration::from_secs(4) {
                self.toast = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }
        if let Some(q) = &self.quick {
            if !q.sticky && q.last.elapsed() > Duration::from_millis(1200) {
                self.quick = None;
            } else if !q.sticky {
                ctx.request_repaint_after(Duration::from_millis(300));
            }
        }
    }

    // ---- theme -------------------------------------------------------------------

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let dark = match self.cfg.theme {
            ThemeMode::Light => false,
            ThemeMode::Dark => true,
            ThemeMode::System => ctx.system_theme() == Some(egui::Theme::Dark),
        };
        if self.applied_dark == Some(dark) {
            return;
        }
        self.applied_dark = Some(dark);
        self.dark = dark;
        self.pal = if dark { Palette::dark() } else { Palette::light() };
        let visuals = self.pal.visuals();
        ctx.set_visuals_of(egui::Theme::Dark, visuals.clone());
        ctx.set_visuals_of(egui::Theme::Light, visuals);
        ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
        if let Some(h) = self.hwnd {
            win::set_dark_title_bar(h, dark);
        }
        self.preview = None;
    }

    /// Forces the theme to be re-evaluated (after the options changed).
    pub fn reapply_theme(&mut self) {
        self.applied_dark = None;
    }

    // ---- input ---------------------------------------------------------------------

    fn handle_input(&mut self, ctx: &egui::Context) {
        let panel_mode = self.panel_keys_active();
        let file_keys = panel_mode && self.cmdline.is_empty() && !self.cmd_active;
        win::PANEL_KEYS.store(file_keys, Ordering::Relaxed);

        for rk in win::take_raw_keys() {
            if !panel_mode {
                continue;
            }
            let cmd = match rk.vk {
                win::VK_NUM_PLUS if rk.ctrl => Some(Cmd::SelectAll),
                win::VK_NUM_PLUS if rk.alt => Some(Cmd::SelectSameExt),
                win::VK_NUM_PLUS => Some(Cmd::SelectGroup),
                win::VK_NUM_MINUS if rk.ctrl => Some(Cmd::UnselectAll),
                win::VK_NUM_MINUS if rk.alt => Some(Cmd::UnselectSameExt),
                win::VK_NUM_MINUS => Some(Cmd::UnselectGroup),
                win::VK_NUM_STAR => Some(Cmd::InvertSelection),
                win::VK_NUM_SLASH => Some(Cmd::RestoreSelection),
                0x43 => Some(Cmd::ClipCopy),
                0x58 => Some(Cmd::ClipCut),
                0x56 => Some(Cmd::ClipPaste),
                _ => None,
            };
            if let Some(c) = cmd {
                self.quick = None;
                self.pending.push(c);
            }
        }
        if !panel_mode {
            return;
        }

        // Left/Right move the cursor in the column-based views.
        let brief = self.atab().view != ViewMode::Full;
        let kc = KeyCtx { cmdline_empty: self.cmdline.is_empty(), brief };
        let qs_mode = self.cfg.quick_search;
        let letters_mode = matches!(qs_mode, QuickSearchMode::Letters | QuickSearchMode::LettersDialog);
        let mut cmds = Vec::new();
        let mut eat_space = false;
        let mut eat_alt_text: Option<char> = None;
        let mut quick_chars = String::new();
        let mut quick_back = false;
        let mut quick_nav: isize = 0;
        let mut close_quick = false;
        let mut exec_cmdline: Option<bool> = None;
        let mut escape = false;
        let quick_open = self.quick.is_some();
        let cmd_active = self.cmd_active;

        ctx.input_mut(|i| {
            i.events.retain(|ev| match ev {
                Event::Key { key, pressed: true, modifiers: m, .. } => {
                    let key = *key;
                    // Quick search box keys.
                    if quick_open {
                        match key {
                            Key::Backspace => {
                                quick_back = true;
                                return false;
                            }
                            Key::ArrowUp if !m.any() => {
                                quick_nav -= 1;
                                return false;
                            }
                            Key::ArrowDown if !m.any() => {
                                quick_nav += 1;
                                return false;
                            }
                            Key::Escape => {
                                close_quick = true;
                                return false;
                            }
                            Key::Enter => {
                                close_quick = true;
                                cmds.push(Cmd::Open);
                                return false;
                            }
                            _ if letter_of(key).is_some() && (m.ctrl && m.alt || m.alt && !m.ctrl || !m.any() || m.shift_only()) => {}
                            _ => close_quick = true,
                        }
                    }
                    // Alt+letter opens the main menus (unless Alt+letters is the quick search key).
                    if m.alt && !m.ctrl && !m.shift && qs_mode != QuickSearchMode::Alt && !quick_open {
                        if let Some(i) = letter_of(key).and_then(crate::commands::menu_accelerator) {
                            cmds.push(Cmd::MenuOpen(i));
                            eat_alt_text = letter_of(key);
                            return false;
                        }
                    }
                    // Ctrl+Alt+letter / Alt+letter quick search.
                    if let Some(c) = letter_of(key) {
                        let trigger = match qs_mode {
                            QuickSearchMode::CtrlAlt => m.ctrl && m.alt && !m.shift,
                            QuickSearchMode::Alt => m.alt && !m.ctrl,
                            _ => false,
                        };
                        if trigger {
                            quick_chars.push(c);
                            return false;
                        }
                    }
                    if key == Key::Escape {
                        escape = true;
                        return false;
                    }
                    if key == Key::Enter && !kc.cmdline_empty && !m.ctrl && !m.alt {
                        exec_cmdline = Some(m.shift);
                        return false;
                    }
                    if key == Key::ArrowRight && kc.cmdline_empty && !m.any() && !brief && letters_mode && !cmd_active {
                        cmds.push(Cmd::FocusCmdLine);
                        return false;
                    }
                    if let Some(c) = map_key(key, *m, kc) {
                        if c == Cmd::FocusCmdLine && !letters_mode {
                            return true;
                        }
                        if key == Key::Space {
                            eat_space = true;
                        }
                        cmds.push(c);
                        return false;
                    }
                    true
                }
                Event::Text(t) => {
                    if eat_space && t == " " {
                        eat_space = false;
                        return false;
                    }
                    if eat_alt_text.is_some_and(|c| t.eq_ignore_ascii_case(&c.to_string())) {
                        eat_alt_text = None;
                        return false;
                    }
                    if quick_open && !t.is_empty() {
                        quick_chars.push_str(t);
                        return false;
                    }
                    let typing_into_cmdline = !kc.cmdline_empty || cmd_active;
                    if letters_mode && !typing_into_cmdline && !quick_chars.is_empty() {
                        quick_chars.push_str(t);
                        return false;
                    }
                    if letters_mode && !typing_into_cmdline && t.chars().all(|c| !c.is_control()) {
                        quick_chars.push_str(t);
                        return false;
                    }
                    // AltGr / Ctrl+Alt produce text too; don't let it reach the command line
                    // when it started a quick search.
                    if !quick_chars.is_empty() && qs_mode == QuickSearchMode::CtrlAlt {
                        return false;
                    }
                    true
                }
                _ => true,
            });
        });

        if close_quick {
            self.quick = None;
        }
        if quick_back {
            if let Some(q) = &mut self.quick {
                q.text.pop();
                q.last = Instant::now();
                if q.text.is_empty() && !q.sticky {
                    self.quick = None;
                }
            }
        }
        if !quick_chars.is_empty() {
            let sticky = !matches!(qs_mode, QuickSearchMode::Letters);
            let q = self.quick.get_or_insert(QuickSearch { text: String::new(), last: Instant::now(), sticky });
            q.text.push_str(&quick_chars);
            q.last = Instant::now();
            let text = q.text.clone();
            self.quick_search_find(&text, 0);
        }
        if quick_nav != 0 {
            if let Some(q) = &mut self.quick {
                q.last = Instant::now();
                let text = q.text.clone();
                self.quick_search_find(&text, quick_nav);
            }
        }
        if escape {
            if self.atab().loading.is_some() {
                // Stop waiting for a slow drive; the result will be ignored.
                self.atab_mut().loading = None;
            } else if self.quick.is_some() {
                self.quick = None;
            } else if !self.cmdline.is_empty() || self.cmd_active {
                self.cmdline.clear();
                self.cmd_active = false;
            } else if self.atab().quick_filter.is_some() {
                self.atab_mut().quick_filter = None;
                let s = self.active;
                self.refresh(s);
            } else if self.atab().branch {
                self.pending.push(Cmd::BranchView);
            }
        }
        if let Some(keep_open) = exec_cmdline {
            let line = std::mem::take(&mut self.cmdline);
            self.cmd_active = false;
            self.run_command_line(&line, keep_open);
        }
        if !cmds.is_empty() {
            if self.quick.as_ref().is_some_and(|q| !q.sticky) {
                self.quick = None;
            }
        }
        self.pending.extend(cmds);
    }

    /// Moves the cursor to the next entry matching the quick search text.
    /// `dir` 0 = from the current entry, +1/-1 = next/previous match.
    fn quick_search_find(&mut self, text: &str, dir: isize) {
        let tab = self.atab_mut();
        let n = tab.entries.len();
        if n == 0 || text.is_empty() {
            return;
        }
        let needle = text.to_lowercase();
        let anywhere = needle.starts_with('*');
        let needle = needle.trim_start_matches('*').to_string();
        let matches = |e: &FileEntry| {
            let name = e.name.to_lowercase();
            if anywhere { name.contains(&needle) } else { name.starts_with(&needle) }
        };
        let start = tab.cursor as isize;
        let step = if dir < 0 { -1 } else { 1 };
        let first = if dir == 0 { 0 } else { step };
        for k in 0..n as isize {
            let i = (start + first + k * step).rem_euclid(n as isize) as usize;
            if !tab.entries[i].is_parent && matches(&tab.entries[i]) {
                tab.set_cursor(i);
                return;
            }
        }
    }

    // ---- command line ------------------------------------------------------------

    pub fn run_command_line(&mut self, line: &str, keep_open: bool) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        Config::push_history(&mut self.cfg.cmd_history, line);
        if let Some((conn, dir)) = self.atab().loc.remote().map(|(c, d)| (c.clone(), d.to_string())) {
            // On a server only "cd" works, like in Total Commander.
            let lower = line.to_lowercase();
            match lower.strip_prefix("cd").filter(|r| r.is_empty() || r.starts_with([' ', '/', '.'])) {
                Some(_) => {
                    let arg = line[2..].trim();
                    let target = if arg.is_empty() { "/".to_string() } else { crate::remote::normalize(&arg.replace('\\', "/"), &dir) };
                    let side = self.active;
                    self.go(side, Location::Remote { conn, path: target }, None);
                }
                None => self.error("Only \"cd <path>\" is available on the command line while connected to a server."),
            }
            return;
        }
        let Some(cwd) = self.atab().loc.host_dir() else {
            self.error("The command line needs a real directory in the active panel.");
            return;
        };
        let lower = line.to_lowercase();
        let side = self.active;
        // Internal commands: cd, drive change, md, rd.
        let cd_arg = if lower == "cd" || lower == "chdir" {
            Some(String::new())
        } else if let Some(rest) = lower.strip_prefix("cd").filter(|r| r.starts_with(['\\', '.', ' ', '/'])) {
            Some(line[line.len() - rest.len()..].trim().trim_start_matches("/d ").trim().to_string())
        } else {
            None
        };
        if let Some(arg) = cd_arg {
            if arg.is_empty() {
                return;
            }
            let target = vfs::resolve_input_path(&cwd, &arg);
            if target.is_dir() {
                self.go(side, Location::fs(target), None);
            } else if target.is_file() && crate::archive::is_archive_name(&target.to_string_lossy()) {
                self.go(side, Location::Archive { archive: target, inner: String::new() }, None);
            } else {
                self.error(format!("Directory not found:\n{}", target.display()));
            }
            return;
        }
        if line.len() == 2 && line.ends_with(':') && line.as_bytes()[0].is_ascii_alphabetic() {
            self.goto_drive(side, line.chars().next().unwrap().to_ascii_uppercase());
            return;
        }
        for (prefix, make) in [("md ", true), ("mkdir ", true), ("rd ", false), ("rmdir ", false)] {
            if lower.starts_with(prefix) {
                let target = vfs::resolve_input_path(&cwd, &line[prefix.len()..]);
                let res = if make { std::fs::create_dir_all(&target) } else { std::fs::remove_dir(&target) };
                match res {
                    Ok(()) => {
                        self.refresh(side);
                        if make {
                            if let Some(n) = target.file_name() {
                                let n = n.to_string_lossy().into_owned();
                                if let Some(i) = self.atab().index_of(&n) {
                                    self.atab_mut().set_cursor(i);
                                }
                            }
                        }
                    }
                    Err(e) => self.error(format!("{}: {e}", target.display())),
                }
                return;
            }
        }
        let result = if keep_open {
            win::spawn_detached("cmd.exe", &format!("/K {line}"), Some(&cwd))
        } else {
            let (prog, rest) = split_program(line);
            match win::find_on_path(&prog, Some(&cwd)) {
                Some(exe) => {
                    let ext = exe.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                    if ext == "exe" || ext == "com" {
                        win::spawn_detached(&exe.to_string_lossy(), rest, Some(&cwd))
                    } else if (ext == "cmd" || ext == "bat") && win::batch_starts_gui_only(&exe) {
                        // A launcher such as VS Code's code.cmd: no console window.
                        win::spawn_hidden("cmd.exe", &format!("/S /C \"{line}\""), Some(&cwd))
                    } else {
                        win::spawn_detached("cmd.exe", &format!("/S /C \"{line}\""), Some(&cwd))
                    }
                }
                None => {
                    let doc = vfs::resolve_input_path(&cwd, &prog);
                    if doc.exists() {
                        win::shell_open(&doc, Some(&cwd)).map_err(std::io::Error::other)
                    } else if win::shell_execute("open", &prog, rest, Some(&cwd)).is_ok() {
                        Ok(())
                    } else {
                        win::spawn_detached("cmd.exe", &format!("/S /C \"{line}\""), Some(&cwd))
                    }
                }
            }
        };
        if let Err(e) = result {
            self.error(format!("Could not run \"{line}\":\n{e}"));
        }
    }

    /// Opens a path typed or pasted into the path line: a folder, a folder in
    /// an archive, a file (the cursor goes to it) or a server URL. Returns
    /// false when it can't be opened, so the field stays open like in TC.
    pub fn goto_typed_path(&mut self, side: Side, input: &str) -> bool {
        let text = vfs::strip_path_mask(input);
        if text.is_empty() {
            return true;
        }
        self.active = side;
        if let Some((conn, dir)) = self.tab(side).loc.remote().map(|(c, d)| (c.clone(), d.to_string())) {
            // A path on the same server, with or without its URL prefix.
            let prefix = conn.url_prefix();
            let rest = match text.get(..prefix.len()) {
                Some(p) if p.eq_ignore_ascii_case(&prefix) => Some(&text[prefix.len()..]),
                _ if !text.contains(':') && !text.starts_with("\\\\") => Some(text),
                _ => None,
            };
            if let Some(rest) = rest {
                let path = crate::remote::normalize(&rest.replace('\\', "/"), &dir);
                self.go(side, Location::Remote { conn, path }, None);
                return true;
            }
        }
        if text.contains("://") {
            return match self.connect_url(text) {
                Ok(()) => true,
                Err(e) => {
                    self.error(e);
                    false
                }
            };
        }
        let base = self.tab(side).loc.host_dir().unwrap_or_else(|| PathBuf::from("C:\\"));
        let target = vfs::resolve_input_path(&base, text);
        if target.is_dir() {
            self.go(side, Location::fs(target), None);
        } else if let Some((archive, inner)) = vfs::split_archive_path(&target) {
            self.go(side, Location::Archive { archive, inner }, None);
        } else if let (true, Some(dir), Some(name)) = (target.is_file(), target.parent(), target.file_name()) {
            let name = name.to_string_lossy().into_owned();
            self.go(side, Location::fs(dir.to_path_buf()), Some(&name));
        } else {
            self.error(format!("Directory not found:\n{}", target.display()));
            return false;
        }
        true
    }

    // ---- scripted input (tests) ------------------------------------------------------

    pub fn inject_steps(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if self.steps.is_empty() || Instant::now() < self.step_at {
            if !self.steps.is_empty() {
                ctx.request_repaint_after(Duration::from_millis(30));
            }
            return;
        }
        let step = self.steps.pop_front().unwrap();
        let step_kind = matches!(step, Step::ShotSearch(_)) as u8;
        let mut delay = 120;
        match step {
            Step::Key(key, modifiers) => {
                for pressed in [true, false] {
                    raw.events.push(Event::Key { key, physical_key: Some(key), pressed, repeat: false, modifiers });
                }
                if key == Key::Space && modifiers.is_none() {
                    raw.events.push(Event::Text(" ".into()));
                }
            }
            Step::Text(t) => raw.events.push(Event::Text(t)),
            Step::Mouse { pos, button, pressed, click } => {
                raw.events.push(Event::PointerMoved(pos));
                raw.events.push(Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE });
                if pressed && click {
                    self.steps.push_front(Step::Mouse { pos, button, pressed: false, click: false });
                    delay = 60;
                }
            }
            Step::Move(pos) => raw.events.push(Event::PointerMoved(pos)),
            Step::Raw(k) => win::push_raw_key(k),
            Step::Wait(ms) => delay = ms,
            Step::Shot(path) => {
                ctx.request_screenshot(move |img| {
                    let _ = save_png(&path, &img);
                });
                delay = 500;
            }
            Step::ShotChild(path) | Step::ShotSearch(path) => {
                let id = if matches!(step_kind, 1) {
                    self.search.as_ref().map(|s| s.viewport_id())
                } else {
                    self.viewers.last().map(|v| v.viewport_id())
                };
                if let Some(id) = id {
                    let cb = egui::ScreenshotCallback::new(move |img: Arc<egui::ColorImage>| {
                        let _ = save_png(&path, &img);
                    });
                    ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Screenshot(cb));
                    ctx.request_repaint_of(id);
                }
                delay = 600;
            }
            Step::Exit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
        self.step_at = Instant::now() + Duration::from_millis(delay);
        ctx.request_repaint_after(Duration::from_millis(delay));
    }

    pub fn save_config(&mut self) {
        let cfg = &mut self.cfg;
        for (side, pc) in [(Side::Left, &mut cfg.left), (Side::Right, &mut cfg.right)] {
            let panel = &self.panels[side.index()];
            pc.tabs = panel
                .tabs
                .iter()
                .map(|t| match &t.loc {
                    Location::Fs(p) => p.to_string_lossy().into_owned(),
                    // Server connections aren't restored; keep the last local folder.
                    Location::Remote { .. } => Panel::last_local(t)
                        .and_then(|l| l.fs_path().map(|p| p.to_string_lossy().into_owned()))
                        .unwrap_or_else(|| "C:\\".into()),
                    other => other.host_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "C:\\".into()),
                })
                .collect();
            pc.active_tab = panel.active;
            pc.sort = panel.tab().sort;
            pc.view = panel.tab().view;
        }
        cfg.active_right = self.active == Side::Right;
        if let Some(r) = self.window_rect {
            if !self.maximized {
                cfg.window = Some([r.min.x, r.min.y, r.width(), r.height()]);
            }
        }
        cfg.maximized = self.maximized;
        if let Err(e) = cfg.save() {
            self.toast = Some(Toast { text: format!("Could not save settings: {e}"), at: Instant::now(), error: true });
        }
    }

    // ---- command execution -----------------------------------------------------------

    pub fn exec(&mut self, ctx: &egui::Context, cmd: Cmd) {
        use Cmd::*;
        let side = self.active;
        let other = side.other();
        let lo = self.list_opts();
        match cmd {
            Up | Down => {
                // In the thumbnails grid Up/Down move by a whole row.
                let tab = self.atab_mut();
                let step = if tab.view == ViewMode::Thumbs { tab.grid_cols.max(1) as isize } else { 1 };
                tab.move_cursor(if cmd == Up { -step } else { step });
            }
            PageUp => {
                let n = self.page_step();
                self.atab_mut().move_cursor(-n);
            }
            PageDown => {
                let n = self.page_step();
                self.atab_mut().move_cursor(n);
            }
            Home => self.atab_mut().set_cursor(0),
            End => {
                let n = self.atab().entries.len();
                self.atab_mut().set_cursor(n.saturating_sub(1));
            }
            Left | Right => {
                // Brief view: a column of entries; thumbnails: one entry.
                let tab = self.atab_mut();
                let step = if tab.view == ViewMode::Thumbs { 1 } else { tab.page_rows.max(1) as isize };
                tab.move_cursor(if cmd == Left { -step } else { step });
            }
            SelectUp | SelectDown => {
                let tab = self.atab_mut();
                let c = tab.cursor;
                tab.toggle(c);
                tab.move_cursor(if cmd == SelectUp { -1 } else { 1 });
            }
            SwitchPanel => {
                self.active = other;
                self.quick = None;
            }
            Open => {
                let c = self.atab().cursor;
                self.open_entry(side, c, false);
            }
            OpenShell => {
                let c = self.atab().cursor;
                if self.atab().loc.remote().is_some() {
                    self.fetch_then(side, c, "open");
                    return;
                }
                if let Some(p) = self.materialize(side, c) {
                    let is_dir = self.atab().entries.get(c).is_some_and(|e| e.is_dir);
                    if is_dir || crate::archive::is_archive_name(&p.to_string_lossy()) {
                        self.shell_open(&p);
                    } else {
                        let _ = win::spawn_detached("cmd.exe", &format!("/K \"{}\"", p.display()), p.parent());
                    }
                }
            }
            Parent => self.go_parent(side),
            Root => {
                let root = self.atab().loc.root();
                self.go(side, root, None);
            }
            Reread => {
                self.drives = win::drives();
                self.drive_requested.clear();
                self.refresh(side);
            }
            OpenInSide(target) => {
                // Ctrl+Left/Right: show the directory under the cursor (or the
                // current one) in the given panel.
                let tab = self.atab();
                let loc = match tab.cursor_entry() {
                    Some(e) if e.is_dir && !e.is_parent && target != side => match &e.full_path {
                        Some(p) => Location::fs(p),
                        None => tab.loc.child_dir(&e.name),
                    },
                    Some(e) if e.is_parent && target != side => tab.loc.parent().map(|(l, _)| l).unwrap_or(tab.loc.clone()),
                    _ => tab.loc.clone(),
                };
                if target == side {
                    let o = self.tab(other).loc.clone();
                    self.go(side, o, None);
                } else {
                    self.go(target, loc, None);
                }
            }
            TargetEqualsSource => {
                let loc = self.atab().loc.clone();
                self.go(other, loc, None);
            }
            Exchange => {
                let l = self.tab(Side::Left).loc.clone();
                let r = self.tab(Side::Right).loc.clone();
                self.go(Side::Left, r, None);
                self.go(Side::Right, l, None);
            }
            HistoryBack => self.history_step(side, -1),
            HistoryForward => self.history_step(side, 1),
            HistoryMenu => self.open_history_menu(side),
            DriveMenu(s) => self.open_drive_menu(s),
            GotoDrive(s, letter) => self.goto_drive(s, letter),
            GotoPath(p) => {
                self.goto_typed_path(side, &vfs::expand_env(&p));
            }
            NetworkDrives => {
                let items = self
                    .drives
                    .iter()
                    .filter(|d| d.kind == win::DriveKind::Network)
                    .map(|d| PopupItem { label: format!("{}:", d.letter), detail: String::new(), cmd: Some(GotoDrive(side, d.letter)) })
                    .collect::<Vec<_>>();
                if items.is_empty() {
                    self.toast("No mapped network drives");
                } else {
                    self.open_popup(items);
                }
            }
            MapNetworkDrive | DisconnectNetworkDrive => {
                let _ = win::spawn_detached(
                    "rundll32.exe",
                    if cmd == MapNetworkDrive {
                        "shell32.dll,SHHelpShortcuts_RunDLL Connect"
                    } else {
                        "shell32.dll,SHHelpShortcuts_RunDLL Disconnect"
                    },
                    None,
                );
            }
            Brief | Full | Thumbnails => {
                self.atab_mut().view = match cmd {
                    Brief => ViewMode::Brief,
                    Thumbnails => ViewMode::Thumbs,
                    _ => ViewMode::Full,
                };
                self.atab_mut().follow_cursor = true;
            }
            SortBy(col) => {
                let tab = self.atab_mut();
                tab.sort = if tab.sort.column == col { SortSpec { column: col, reverse: !tab.sort.reverse } } else { SortSpec { column: col, reverse: false } };
                if col == SortColumn::Unsorted {
                    self.refresh(side);
                } else {
                    self.atab_mut().resort();
                }
            }
            ReverseSort => {
                self.atab_mut().sort.reverse ^= true;
                self.atab_mut().resort();
            }
            ShowAll => {
                self.atab_mut().filter = FileFilter::All;
                self.refresh(side);
            }
            ShowPrograms => {
                self.atab_mut().filter = FileFilter::Programs;
                self.refresh(side);
            }
            ShowCustom => {
                let current = match &self.atab().filter {
                    FileFilter::Custom(m) => m.clone(),
                    _ => "*.*".into(),
                };
                self.dialog = Some(Dialog::input(InputKind::CustomFilter, "Show only files matching:", current, self.cfg.select_history.clone()));
            }
            ToggleHidden => {
                self.cfg.show_hidden = !self.cfg.show_hidden;
                self.refresh_all();
                self.toast(if self.cfg.show_hidden { "Showing hidden and system files" } else { "Hiding hidden and system files" });
            }
            QuickView => {
                self.quick_view = !self.quick_view;
                self.preview = None;
            }
            QuickFilter => {
                let current = self.atab().quick_filter.clone().unwrap_or_default();
                self.dialog = Some(Dialog::input(InputKind::QuickFilter, "Quick filter (shows only matching names):", current, Vec::new()));
            }
            BranchView => {
                let tab = self.atab_mut();
                if matches!(tab.loc, Location::Fs(_)) {
                    tab.branch = !tab.branch;
                    self.refresh(side);
                    self.atab_mut().set_cursor(0);
                }
            }
            NewTab => {
                let t = self.atab();
                let mut nt = Tab::new(t.loc.clone(), t.sort, t.view);
                if self.is_slow(&t.loc) {
                    // Duplicate what is shown instead of re-reading a slow location.
                    nt.entries = t.entries.iter().cloned().map(|mut e| {
                        e.selected = false;
                        e
                    }).collect();
                } else {
                    let _ = nt.go(t.loc.clone(), None, lo, &mut self.zips);
                }
                let p = &mut self.panels[side.index()];
                p.tabs.insert(p.active + 1, nt);
                p.active += 1;
            }
            OpenDirInNewTab => {
                let t = self.atab();
                let loc = match t.cursor_entry() {
                    Some(e) if e.is_dir && !e.is_parent => t.loc.child_dir(&e.name),
                    _ => t.loc.clone(),
                };
                let mut nt = Tab::new(loc.clone(), t.sort, t.view);
                if self.is_slow(&loc) {
                    let p = &mut self.panels[side.index()];
                    p.tabs.insert(p.active + 1, nt);
                    p.active += 1;
                    let index = p.active;
                    self.spawn_listing(side, index, loc, None, true);
                } else if nt.go(loc, None, lo, &mut self.zips).is_ok() {
                    let p = &mut self.panels[side.index()];
                    p.tabs.insert(p.active + 1, nt);
                    p.active += 1;
                }
            }
            CloseTab => {
                let p = &mut self.panels[side.index()];
                if p.tabs.len() > 1 {
                    p.tabs.remove(p.active);
                    p.active = p.active.min(p.tabs.len() - 1);
                    self.update_watches();
                }
            }
            CloseAllTabs => {
                let p = &mut self.panels[side.index()];
                let keep = p.tabs.remove(p.active);
                p.tabs = vec![keep];
                p.active = 0;
            }
            NextTab | PrevTab => {
                let p = &mut self.panels[side.index()];
                let n = p.tabs.len();
                p.active = if cmd == NextTab { (p.active + 1) % n } else { (p.active + n - 1) % n };
                self.update_watches();
            }
            Select => {
                let tab = self.atab_mut();
                let c = tab.cursor;
                tab.toggle(c);
                tab.move_cursor(1);
            }
            SelectSpace => {
                let tab = self.atab_mut();
                let c = tab.cursor;
                tab.toggle(c);
                let calc = tab.entries.get(c).filter(|e| e.is_dir && !e.is_parent && e.selected && e.dir_size.is_none()).map(|e| e.name.clone());
                if let (true, Some(name)) = (self.cfg.space_calculates_dirs, calc) {
                    self.calc_dir_sizes(side, vec![name]);
                }
            }
            SelectGroup | UnselectGroup => {
                let select = cmd == SelectGroup;
                let default = self.cfg.select_history.first().cloned().unwrap_or_else(|| "*.*".into());
                self.dialog = Some(Dialog::input(
                    InputKind::SelectGroup(select),
                    if select { "Select files matching:" } else { "Unselect files matching:" },
                    default,
                    self.cfg.select_history.clone(),
                ));
            }
            SelectAll => {
                let dirs = true;
                self.atab_mut().select_all(true, dirs);
            }
            UnselectAll => self.atab_mut().select_all(false, true),
            InvertSelection => {
                let dirs = self.cfg.num_plus_selects_dirs;
                self.atab_mut().invert(dirs);
            }
            SelectSameExt => self.atab_mut().select_same_ext(true),
            UnselectSameExt => self.atab_mut().select_same_ext(false),
            SaveSelection => self.atab_mut().save_selection(),
            RestoreSelection => self.atab_mut().restore_selection(),
            CopyNames | CopyFullNames | CopyDetails => {
                let tab = self.atab();
                let lines: Vec<String> = tab
                    .targets()
                    .into_iter()
                    .map(|i| {
                        let e = &tab.entries[i];
                        let full = tab.loc.child_path(e).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| format!("{}{}", tab.loc.display(), e.name));
                        let name = if cmd == CopyNames { e.name_shown().to_string() } else { full };
                        if cmd == CopyDetails {
                            let size = if e.is_dir { "<DIR>".into() } else { crate::format::thousands(e.size) };
                            let date = e.modified.map(crate::format::datetime).unwrap_or_default();
                            format!("{name}\t{size}\t{date}\t{}", e.attr_text())
                        } else {
                            name
                        }
                    })
                    .collect();
                if !lines.is_empty() {
                    ctx.copy_text(lines.join("\r\n"));
                    self.toast(format!("Copied {} name(s) to the clipboard", lines.len()));
                }
            }
            CompareDirs => self.compare_dirs(),
            SyncDirs => match (self.tab(Side::Left).loc.fs_path(), self.tab(Side::Right).loc.fs_path()) {
                (Some(l), Some(r)) => self.dialog = Some(Dialog::Sync(Box::new(crate::sync::SyncTool::new(l, r)))),
                _ => self.error("Synchronize needs a normal directory in both panels."),
            },
            CdTree => {
                if let Some(dir) = self.atab().loc.host_dir() {
                    self.dialog = Some(Dialog::Tree(Box::new(crate::tree::TreeDialog::new(&dir, self.cfg.show_hidden))));
                }
            }
            View => self.view_current(),
            Edit => {
                let c = self.atab().cursor;
                if self.atab().loc.remote().is_some() {
                    self.fetch_then(side, c, "edit");
                    return;
                }
                if self.atab().entries.get(c).is_some_and(|e| !e.is_dir) {
                    if let Some(p) = self.materialize(side, c) {
                        let editor = self.cfg.editor.clone();
                        if let Err(e) = win::spawn_detached(&editor, &format!("\"{}\"", p.display()), p.parent()) {
                            self.error(format!("Could not start editor \"{editor}\": {e}"));
                        }
                    }
                }
            }
            EditNew => {
                if self.atab().loc.fs_path().is_some() {
                    let def = self.atab().cursor_entry().filter(|e| !e.is_dir).map(|e| e.name.clone()).unwrap_or_else(|| "new.txt".into());
                    self.dialog = Some(Dialog::input(InputKind::NewFile, "Edit file (created if it does not exist):", def, Vec::new()));
                }
            }
            Copy | Move | CopySame => self.transfer_dialog(cmd == Move, cmd == CopySame),
            Rename => self.start_rename(),
            MkDir => {
                if matches!(self.atab().loc, Location::Results { .. }) {
                    return;
                }
                self.dialog = Some(Dialog::input(InputKind::MkDir, "New folder (directory):", String::new(), Vec::new()));
            }
            Delete | DeleteNow => self.delete_dialog(cmd == DeleteNow),
            Pack => self.pack_dialog(),
            Unpack => self.unpack_dialog(),
            TestArchive => self.test_archives(),
            Properties => {
                let tab = self.atab();
                let path = match tab.cursor_entry() {
                    Some(e) if !e.is_parent => tab.loc.child_path(e),
                    _ => tab.loc.fs_path().map(Path::to_path_buf),
                };
                if let Some(p) = path {
                    win::show_properties(self.hwnd, &p);
                }
            }
            Attributes => {
                let paths: Vec<PathBuf> = self.sources(side).into_iter().filter_map(|s| if let Source::Fs(p) = s { Some(p) } else { None }).collect();
                if !paths.is_empty() {
                    self.dialog = Some(Dialog::attributes(paths));
                }
            }
            OccupiedSpace => self.occupied_space(),
            AllDirSizes => {
                let names: Vec<String> = self.atab().entries.iter().filter(|e| e.is_dir && !e.is_parent).map(|e| e.name.clone()).collect();
                self.calc_dir_sizes(side, names);
            }
            MultiRename => {
                let tab = self.atab();
                if let Location::Fs(dir) = &tab.loc {
                    let mut items: Vec<(PathBuf, FileEntry)> = Vec::new();
                    for i in tab.targets() {
                        let e = tab.entries[i].clone();
                        let p = e.full_path.clone().unwrap_or_else(|| dir.join(&e.name));
                        items.push((p, e));
                    }
                    if !items.is_empty() {
                        self.dialog = Some(Dialog::MultiRename(Box::new(crate::rename::RenameTool::new(items))));
                    }
                }
            }
            Search => {
                if self.search.is_none() {
                    let start = self.atab().loc.host_dir().unwrap_or_else(|| PathBuf::from("C:\\"));
                    self.search = Some(crate::search::SearchHandle::new(start, self.cfg.search_history.clone(), &self.pal));
                }
            }
            CompareContent => self.compare_content(),
            Checksum => self.checksum(),
            Hotlist => self.open_hotlist(),
            AddToHotlist => {
                if let Some(dir) = self.atab().loc.fs_path() {
                    let name = self.atab().loc.caption();
                    let path = dir.to_string_lossy().into_owned();
                    self.cfg.hotlist.push(crate::config::HotlistEntry { name, path });
                    self.save_config();
                    self.toast("Added to directory hotlist");
                }
            }
            ConfigureHotlist => self.dialog = Some(Dialog::Hotlist { entries: self.cfg.hotlist.clone(), sel: 0 }),
            ClipCopy | ClipCut => {
                let paths: Vec<PathBuf> = self.sources(side).into_iter().filter_map(|s| if let Source::Fs(p) = s { Some(p) } else { None }).collect();
                if !paths.is_empty() {
                    match win::set_clipboard_files(&paths, cmd == ClipCut) {
                        Ok(()) => self.toast(format!("{} {} item(s) to the clipboard", if cmd == ClipCut { "Cut" } else { "Copied" }, paths.len())),
                        Err(e) => self.error(format!("Clipboard error: {e}")),
                    }
                }
            }
            ClipPaste => {
                let dest = match &self.atab().loc {
                    Location::Fs(d) => Dest::Dir(d.clone()),
                    Location::Archive { archive, inner } => Dest::Archive { archive: archive.clone(), inner: inner.clone() },
                    Location::Remote { conn, path } => Dest::Remote { conn: conn.clone(), dir: path.clone() },
                    _ => return,
                };
                match win::get_clipboard_files() {
                    Some((files, cut)) if !files.is_empty() => {
                        self.start_job(OpRequest {
                            kind: if cut { OpKind::Move } else { OpKind::Copy },
                            sources: files.into_iter().map(Source::Fs).collect(),
                            dest: Some(dest),
                            mask: "*.*".into(),
                        });
                    }
                    _ => self.toast("No files on the clipboard"),
                }
            }
            ContextMenu => {
                let at = self.context_at.take();
                self.open_context_menu(at);
            }
            OpenTerminal => {
                if let Some(dir) = self.atab().loc.host_dir() {
                    let term = self.cfg.terminal.clone();
                    if let Err(e) = win::spawn_detached(&term, "", Some(&dir)) {
                        self.error(format!("Could not start {term}: {e}"));
                    }
                }
            }
            OpenExplorer => {
                if let Some(dir) = self.atab().loc.host_dir() {
                    let _ = win::spawn_detached("explorer.exe", &format!("\"{}\"", dir.display()), None);
                }
            }
            OpenDesktop => {
                if let Some(home) = std::env::var_os("USERPROFILE") {
                    let candidates = [PathBuf::from(&home).join("Desktop"), PathBuf::from(&home).join("OneDrive").join("Desktop"), PathBuf::from(&home).join("OneDrive").join("Skrivbord")];
                    if let Some(d) = candidates.into_iter().find(|p| p.is_dir()) {
                        self.go(side, Location::fs(d), None);
                    }
                }
            }
            CmdHistory => self.open_cmd_history(),
            CmdAddName | CmdAddPath => {
                let tab = self.atab();
                if let Some(e) = tab.cursor_entry().filter(|e| !e.is_parent) {
                    let text = if cmd == CmdAddPath {
                        tab.loc.child_path(e).map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| e.name.clone())
                    } else {
                        e.name_shown().to_string()
                    };
                    let quoted = if text.contains(' ') { format!("\"{text}\"") } else { text };
                    if !self.cmdline.is_empty() && !self.cmdline.ends_with(' ') {
                        self.cmdline.push(' ');
                    }
                    self.cmdline.push_str(&quoted);
                    self.cmdline.push(' ');
                    self.cmd_active = true;
                }
            }
            CmdCopyPath => {
                let p = self.atab().loc.display();
                self.cmdline.push_str(&p);
                self.cmd_active = true;
            }
            FocusCmdLine => self.cmd_active = true,
            Options => self.dialog = Some(Dialog::Options(Box::new(self.cfg.clone()))),
            ToggleTheme => {
                self.cfg.theme = if self.dark { ThemeMode::Light } else { ThemeMode::Dark };
                self.apply_theme(ctx);
                self.save_config();
            }
            ImportTc => match crate::config::find_tc_ini() {
                Some(path) => match crate::ini::Ini::load(&path) {
                    Ok(ini) => {
                        self.cfg.import_tc(&ini);
                        let sites = self.import_tc_sites();
                        self.cfg.imported_from = Some(path.to_string_lossy().into_owned());
                        self.applied_dark = None;
                        let (l, r) = (self.cfg.left.clone(), self.cfg.right.clone());
                        for (s, pc) in [(Side::Left, l), (Side::Right, r)] {
                            if let Some(p) = pc.tabs.get(pc.active_tab) {
                                self.tab_mut(s).sort = pc.sort;
                                self.go(s, Location::fs(p), None);
                            }
                        }
                        let bars = self.import_tc_bars(&path, &ini);
                        let bars = if bars.is_empty() { String::new() } else { format!("\n- the {}", bars.join(" and the ")) };
                        self.save_config();
                        self.dialog = Some(Dialog::Message {
                            title: "Import".into(),
                            text: format!(
                                "Imported from {}:\n\n- {} hotlist entries\n- {} command line history entries\n- {sites} new FTP connection(s){bars}\n- panel paths, sort order, quick search mode, mouse mode, theme and hidden-file setting",
                                path.display(),
                                self.cfg.hotlist.len(),
                                self.cfg.cmd_history.len()
                            ),
                            error: false,
                        });
                    }
                    Err(e) => self.error(format!("{}: {e}", path.display())),
                },
                None => self.error("No Total Commander configuration (wincmd.ini) was found."),
            },
            SaveSettings => {
                self.save_config();
                self.toast(format!("Settings saved to {}", self.cfg.file.display()));
            }
            About => self.dialog = Some(Dialog::About),
            KeyboardHelp => self.dialog = Some(Dialog::Keyboard),
            Exit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            Run(line) => self.run_command_line(&line, false),
            RunButton { button, dropped } => self.run_button(ctx, *button, dropped),
            ConfigureBar(kind) => {
                let bar = self.bars[kind.index()].clone();
                self.dialog = Some(Dialog::BarEditor(Box::new(crate::bar_ui::BarEditor::new(kind, bar, None))));
            }
            EditBarButton(kind, i) => {
                let bar = self.bars[kind.index()].clone();
                self.dialog = Some(Dialog::BarEditor(Box::new(crate::bar_ui::BarEditor::new(kind, bar, Some(i)))));
            }
            DeleteBarButton(kind, i) => {
                let mut bar = self.bars[kind.index()].clone();
                if i < bar.buttons.len() {
                    bar.buttons.remove(i);
                    self.set_bar(kind, bar);
                }
            }
            ToggleBar(kind) => {
                let flag = match kind {
                    crate::buttonbar::BarKind::Main => &mut self.cfg.show_toolbar,
                    crate::buttonbar::BarKind::Vertical => &mut self.cfg.show_middle_bar,
                };
                *flag = !*flag;
                self.save_config();
            }
            MenuOpen(i) => self.open_main_menu(i % crate::commands::MENU_COUNT),
            FtpConnect => self.dialog = Some(Dialog::Sites { sel: 0 }),
            FtpNew => self.dialog = Some(Dialog::QuickConnect { text: self.cfg.ftp_history.first().cloned().unwrap_or_default(), error: None, init: true }),
            FtpDisconnect => self.disconnect(side),
            HistoryGoto(pos) => {
                let delta = pos as isize - self.atab().hist_pos as isize;
                if delta != 0 {
                    self.history_step(side, delta);
                }
            }
        }
    }

    fn page_step(&self) -> isize {
        let tab = self.atab();
        let rows = tab.page_rows.max(1) as isize;
        match tab.view {
            ViewMode::Brief => rows * 2,
            ViewMode::Thumbs => rows * tab.grid_cols.max(1) as isize,
            ViewMode::Full => (rows - 1).max(1),
        }
    }

    // ---- dialogs that prepare operations ------------------------------------------------

    fn transfer_dialog(&mut self, mv: bool, same_dir: bool) {
        let side = self.active;
        let sources = self.sources(side);
        if sources.is_empty() {
            return;
        }
        let single = (sources.len() == 1).then(|| sources[0].name());
        let src_dir = self.atab().loc.host_dir();
        let target_base = if same_dir {
            self.atab().loc.display()
        } else {
            match &self.tab(side.other()).loc {
                Location::Results { .. } => self.atab().loc.display(),
                other => other.display(),
            }
        };
        let is_dir_single = sources.len() == 1
            && match &sources[0] {
                Source::Fs(p) => p.is_dir(),
                Source::Archive { is_dir, .. } | Source::Remote { is_dir, .. } => *is_dir,
            };
        let target = match &single {
            Some(name) if same_dir || !is_dir_single => format!("{target_base}{name}"),
            _ => format!("{target_base}*.*"),
        };
        let what = self.describe(side);
        let verb = if mv { "Rename/move" } else { "Copy" };
        self.dialog = Some(Dialog::Transfer {
            mv,
            sources,
            label: format!("{verb} {what} to:"),
            target,
            src_dir,
            init: true,
        });
    }

    pub fn start_transfer(&mut self, mv: bool, sources: Vec<Source>, target: &str, src_dir: Option<PathBuf>) {
        let parsed = match self.parse_remote_target(target, sources.len()) {
            Some(r) => r,
            None => parse_target(target, src_dir.as_deref(), sources.len()),
        };
        match parsed {
            Ok((dest, mask)) => {
                self.start_job(OpRequest { kind: if mv { OpKind::Move } else { OpKind::Copy }, sources, dest: Some(dest), mask });
            }
            Err(e) => self.error(e),
        }
    }

    /// A target like `sftp://me@host/dir/` names a directory on a server that
    /// is open in a panel. `None` when the text isn't a URL.
    fn parse_remote_target(&self, text: &str, n_sources: usize) -> Option<Result<(Dest, String), String>> {
        let t = text.trim().trim_matches('"');
        let (scheme, _) = t.split_once("://")?;
        if scheme.contains(['\\', '/']) {
            return None;
        }
        let conns = self.panels.iter().flat_map(|p| p.tabs.iter()).filter_map(|tab| tab.loc.remote().map(|(c, _)| c.clone()));
        let mut best: Option<(Arc<crate::remote::Connection>, usize)> = None;
        for c in conns {
            let prefix = c.url_prefix();
            let matches = t.len() >= prefix.len() && t[..prefix.len()].eq_ignore_ascii_case(&prefix) && t[prefix.len()..].chars().next().is_none_or(|ch| ch == '/');
            if matches && best.as_ref().is_none_or(|(_, n)| prefix.len() > *n) {
                best = Some((c, prefix.len()));
            }
        }
        let Some((conn, n)) = best else {
            return Some(Err(format!("Not connected to {t}.\n\nConnect first (Ctrl+F or Ctrl+N), then copy into that panel.")));
        };
        let rest = &t[n..];
        let path = crate::remote::normalize(rest, "/");
        if rest.is_empty() || rest.ends_with('/') {
            return Some(Ok((Dest::Remote { conn, dir: path }, "*.*".into())));
        }
        let (dir, last) = crate::remote::split(&path).unwrap_or(("/".into(), String::new()));
        if crate::mask::has_wildcards(&last) || n_sources == 1 {
            Some(Ok((Dest::Remote { conn, dir }, last)))
        } else {
            Some(Ok((Dest::Remote { conn, dir: path }, "*.*".into())))
        }
    }

    fn start_rename(&mut self) {
        let side = self.active;
        let tab = self.atab();
        if tab.has_selection() && tab.targets().len() > 1 {
            self.transfer_dialog(true, true);
            return;
        }
        if !matches!(tab.loc, Location::Fs(_) | Location::Remote { .. }) {
            self.transfer_dialog(true, true);
            return;
        }
        let Some(e) = tab.cursor_entry().filter(|e| !e.is_parent && e.full_path.is_none()) else { return };
        self.rename = Some(InlineRename { side, index: tab.cursor, original: e.name.clone(), text: e.name.clone(), init: true });
    }

    pub fn finish_rename(&mut self, accept: bool) {
        let Some(r) = self.rename.take() else { return };
        if !accept || r.text == r.original || r.text.trim().is_empty() {
            return;
        }
        if let Some((_, dir)) = self.tab(r.side).loc.remote().map(|(c, d)| (c.clone(), d.to_string())) {
            let new = r.text.trim().to_string();
            if new.contains('/') {
                self.error("A name on the server cannot contain \"/\".");
                return;
            }
            if self.tab(r.side).entries.iter().any(|e| e.name == new) {
                self.error(format!("\"{new}\" already exists."));
                return;
            }
            let (from, to) = (crate::remote::join(&dir, &r.original), crate::remote::join(&dir, &new));
            self.remote_op(r.side, Some(new), move |fs| fs.rename(&from, &to));
            return;
        }
        let Location::Fs(dir) = self.tab(r.side).loc.clone() else { return };
        let from = dir.join(&r.original);
        let to = dir.join(r.text.trim());
        if to.exists() && !r.original.eq_ignore_ascii_case(r.text.trim()) {
            self.error(format!("\"{}\" already exists.", r.text.trim()));
            return;
        }
        match std::fs::rename(&from, &to) {
            Ok(()) => {
                self.refresh(r.side);
                let name = r.text.trim().to_string();
                if let Some(i) = self.tab(r.side).index_of(&name) {
                    self.tab_mut(r.side).set_cursor(i);
                }
            }
            Err(e) => self.error(format!("Could not rename \"{}\":\n{e}", r.original)),
        }
    }

    fn delete_dialog(&mut self, permanent: bool) {
        let side = self.active;
        let sources = self.sources(side);
        if sources.is_empty() {
            return;
        }
        let in_archive = sources.iter().any(|s| matches!(s, Source::Archive { .. }));
        if sources.iter().any(|s| matches!(s, Source::Remote { .. })) {
            // Servers have no recycle bin.
            let req = OpRequest { kind: OpKind::Delete { recycle: false }, sources, dest: None, mask: String::new() };
            if !self.cfg.confirm_delete {
                self.start_job(req);
                return;
            }
            let what = self.describe(side);
            let server = self.atab().loc.remote().map(|(c, _)| c.url_prefix()).unwrap_or_default();
            self.dialog = Some(Dialog::Confirm {
                title: "Delete".into(),
                text: format!("Do you really want to delete {what} from {server}?\n\nFiles on a server cannot be restored."),
                action: ConfirmAction::Delete(req.sources, false),
            });
            return;
        }
        // Network shares have no recycle bin: Windows would delete permanently.
        let network = sources.iter().any(|s| match s {
            Source::Fs(p) => {
                let text = p.to_string_lossy();
                let letter = text.chars().next().filter(|_| text.as_bytes().get(1) == Some(&b':')).map(|c| c.to_ascii_uppercase());
                text.starts_with("\\\\") || letter.is_some_and(|l| self.drives.iter().any(|d| d.letter == l && d.kind == win::DriveKind::Network))
            }
            Source::Archive { .. } | Source::Remote { .. } => false,
        });
        let recycle = self.cfg.use_recycle_bin && !permanent && !in_archive && !network;
        if !self.cfg.confirm_delete && !(network && self.cfg.use_recycle_bin && !permanent) {
            self.start_job(OpRequest { kind: OpKind::Delete { recycle }, sources, dest: None, mask: String::new() });
            return;
        }
        let what = self.describe(side);
        let text = if recycle {
            format!("Do you really want to move {what} to the recycle bin?")
        } else if network && !permanent && self.cfg.use_recycle_bin {
            format!("Network drives have no recycle bin.\n\nDo you really want to PERMANENTLY delete {what}?")
        } else {
            format!("Do you really want to PERMANENTLY delete {what}?")
        };
        self.dialog = Some(Dialog::Confirm { title: "Delete".into(), text, action: ConfirmAction::Delete(sources, recycle) });
    }

    fn pack_dialog(&mut self) {
        let side = self.active;
        let sources: Vec<Source> = self.sources(side).into_iter().filter(|s| matches!(s, Source::Fs(_))).collect();
        if sources.is_empty() {
            return;
        }
        let base = match self.tab(side.other()).loc.fs_path() {
            Some(p) => p.to_path_buf(),
            None => self.atab().loc.host_dir().unwrap_or_default(),
        };
        let name = if sources.len() == 1 {
            let n = sources[0].name();
            let stem = if matches!(&sources[0], Source::Fs(p) if p.is_dir()) { n } else { crate::format::split_ext(&n, false).0.to_string() };
            format!("{stem}.zip")
        } else {
            format!("{}.zip", self.atab().loc.caption())
        };
        self.dialog = Some(Dialog::Pack {
            label: format!("Pack {} to archive:", self.describe(side)),
            sources,
            target: base.join(name).to_string_lossy().into_owned(),
            move_files: false,
            init: true,
        });
    }

    fn unpack_dialog(&mut self) {
        let side = self.active;
        let tab = self.atab();
        let archives: Vec<PathBuf> = match &tab.loc {
            Location::Archive { archive, .. } => vec![archive.clone()],
            _ => self
                .sources(side)
                .into_iter()
                .filter_map(|s| if let Source::Fs(p) = s { Some(p) } else { None })
                .filter(|p| p.is_file())
                .collect(),
        };
        if archives.is_empty() {
            return;
        }
        let target = match self.tab(side.other()).loc.fs_path() {
            Some(p) => p.to_path_buf(),
            None => tab.loc.host_dir().unwrap_or_default(),
        };
        self.dialog = Some(Dialog::Unpack { archives, target: vfs::Location::fs(target).display(), separate: false, init: true });
    }

    pub fn start_unpack(&mut self, archives: Vec<PathBuf>, target: &str, separate: bool) {
        let base = PathBuf::from(vfs::expand_env(target.trim()));
        for a in archives {
            let dest = if separate {
                base.join(a.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "archive".into()))
            } else {
                base.clone()
            };
            self.start_job(OpRequest {
                kind: OpKind::Copy,
                sources: vec![Source::Archive { archive: a, inner: String::new(), is_dir: true }],
                dest: Some(Dest::Dir(dest)),
                mask: "*.*".into(),
            });
        }
    }

    fn test_archives(&mut self) {
        let side = self.active;
        let archives: Vec<PathBuf> = match &self.atab().loc {
            Location::Archive { archive, .. } => vec![archive.clone()],
            _ => self.sources(side).into_iter().filter_map(|s| if let Source::Fs(p) = s { Some(p) } else { None }).filter(|p| p.is_file()).collect(),
        };
        if archives.is_empty() {
            return;
        }
        let tx = self.bg_tx.clone();
        let ctx = self.ctx_clone();
        self.toast("Testing archive(s)...");
        std::thread::spawn(move || {
            let mut report = Vec::new();
            for a in archives {
                let res = crate::archive::test_archive(&a).map_err(|e| e.to_string());
                let name = a.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                report.push(match res {
                    Ok(n) => format!("OK     {name} ({n} entries)"),
                    Err(e) => format!("ERROR  {name}: {e}"),
                });
            }
            let error = report.iter().any(|l| l.starts_with("ERROR"));
            let _ = tx.send(BgMsg::Message { title: "Test archives".into(), text: report.join("\n"), error });
            ctx.request_repaint();
        });
    }

    fn occupied_space(&mut self) {
        let side = self.active;
        let tab = self.atab();
        let mut paths: Vec<PathBuf> = self.sources(side).into_iter().filter_map(|s| if let Source::Fs(p) = s { Some(p) } else { None }).collect();
        if !tab.has_selection() {
            if let Some(d) = tab.loc.fs_path() {
                paths = vec![d.to_path_buf()];
            }
        }
        if paths.is_empty() {
            return;
        }
        let what = if paths.len() == 1 { paths[0].display().to_string() } else { format!("{} selected items", paths.len()) };
        self.dialog = Some(Dialog::Occupied { what, result: None });
        let tx = self.bg_tx.clone();
        let ctx = self.ctx_clone();
        let cancel = self.size_cancel.clone();
        cancel.store(false, Ordering::Relaxed);
        std::thread::spawn(move || {
            let (mut b, mut f, mut d) = (0, 0, 0);
            for p in paths {
                if p.is_dir() {
                    let (bb, ff, dd) = vfs::dir_size(&p, &cancel);
                    b += bb;
                    f += ff;
                    d += dd + 1;
                } else if let Ok(m) = std::fs::metadata(&p) {
                    b += m.len();
                    f += 1;
                }
            }
            let _ = tx.send(BgMsg::Occupied { bytes: b, files: f, dirs: d });
            ctx.request_repaint();
        });
    }

    fn compare_dirs(&mut self) {
        // Select files that are newer or missing on the other side (both panels).
        use std::collections::HashMap as Map;
        let map = |tab: &Tab| -> Map<String, Option<std::time::SystemTime>> {
            tab.entries.iter().filter(|e| !e.is_dir).map(|e| (e.name.to_lowercase(), e.modified)).collect()
        };
        let l = map(self.tab(Side::Left));
        let r = map(self.tab(Side::Right));
        let mut counts = [0usize; 2];
        for (side, other) in [(Side::Left, &r), (Side::Right, &l)] {
            let tab = self.tab_mut(side);
            for e in tab.entries.iter_mut().filter(|e| !e.is_dir) {
                e.selected = match other.get(&e.name.to_lowercase()) {
                    None => true,
                    Some(t) => match (e.modified, *t) {
                        (Some(a), Some(b)) => a.duration_since(b).map(|d| d.as_secs() >= 2).unwrap_or(false),
                        _ => false,
                    },
                };
                if e.selected {
                    counts[side.index()] += 1;
                }
            }
        }
        self.toast(format!("Compare: {} newer/unique file(s) left, {} right", counts[0], counts[1]));
    }

    fn compare_content(&mut self) {
        let a = self.atab().cursor_entry().filter(|e| !e.is_dir).and_then(|e| self.atab().loc.child_path(e));
        let b = self.tab(self.active.other()).cursor_entry().filter(|e| !e.is_dir).and_then(|e| self.tab(self.active.other()).loc.child_path(e));
        let (Some(a), Some(b)) = (a, b) else {
            self.error("Place the cursor on a file in each panel to compare them.");
            return;
        };
        let tx = self.bg_tx.clone();
        let ctx = self.ctx_clone();
        std::thread::spawn(move || {
            let text = match compare_files(&a, &b) {
                Ok(None) => format!("The files are identical.\n\n{}\n{}", a.display(), b.display()),
                Ok(Some(off)) => format!("The files differ starting at offset {off} (0x{off:X}).\n\n{}\n{}", a.display(), b.display()),
                Err(e) => format!("Comparison failed: {e}"),
            };
            let _ = tx.send(BgMsg::Message { title: "Compare by content".into(), text, error: false });
            ctx.request_repaint();
        });
    }

    fn checksum(&mut self) {
        let side = self.active;
        let Some(dir) = self.atab().loc.fs_path().map(Path::to_path_buf) else { return };
        let files: Vec<PathBuf> = self.sources(side).into_iter().filter_map(|s| if let Source::Fs(p) = s { Some(p) } else { None }).filter(|p| p.is_file()).collect();
        if files.is_empty() {
            return;
        }
        let tx = self.bg_tx.clone();
        let ctx = self.ctx_clone();
        self.toast("Calculating SHA-256...");
        std::thread::spawn(move || {
            let mut lines = Vec::new();
            let mut errors = Vec::new();
            for f in &files {
                match sha256_file(f) {
                    Ok(h) => lines.push(format!("{h} *{}", f.file_name().unwrap_or_default().to_string_lossy())),
                    Err(e) => errors.push(format!("{}: {e}", f.display())),
                }
            }
            let out = if files.len() == 1 {
                dir.join(format!("{}.sha256", files[0].file_name().unwrap_or_default().to_string_lossy()))
            } else {
                dir.join(format!("{}.sha256", dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "checksums".into())))
            };
            let write = std::fs::write(&out, lines.join("\r\n") + "\r\n");
            let mut text = format!("{}\n\nWritten to {}", lines.join("\n"), out.display());
            if let Err(e) = write {
                errors.push(format!("{}: {e}", out.display()));
            }
            if !errors.is_empty() {
                text.push_str(&format!("\n\nErrors:\n{}", errors.join("\n")));
            }
            let _ = tx.send(BgMsg::Message { title: "Checksum (SHA-256)".into(), text, error: !errors.is_empty() });
            let _ = tx.send(BgMsg::Refresh);
            ctx.request_repaint();
        });
    }

    fn view_current(&mut self) {
        let side = self.active;
        let c = self.atab().cursor;
        let Some(entry) = self.atab().entries.get(c).cloned() else { return };
        if entry.is_dir {
            if !entry.is_parent {
                self.calc_dir_sizes(side, vec![entry.name.clone()]);
            }
            return;
        }
        if self.atab().loc.remote().is_some() {
            self.fetch_then(side, c, "view");
            return;
        }
        if let Some(p) = self.materialize(side, c) {
            let id = self.next_viewer;
            self.next_viewer += 1;
            self.viewers.push(crate::viewer::ViewerWindow::open(id, p, &self.pal));
        }
    }

    // ---- popup menus -------------------------------------------------------------------

    pub fn open_popup(&mut self, items: Vec<PopupItem>) {
        let pos = self.list_rects[self.active.index()].left_top() + egui::vec2(20.0, 4.0);
        self.open_popup_at(items, pos);
    }

    pub fn open_popup_at(&mut self, items: Vec<PopupItem>, pos: Pos2) {
        if items.is_empty() {
            return;
        }
        self.quick = None;
        self.popup = Some(PopupMenu { pos, items, sel: 0, opened: Instant::now(), menu: None });
    }

    /// Shows a main menu as a keyboard-navigable popup under its title.
    fn open_main_menu(&mut self, index: usize) {
        let mut menus = crate::commands::main_menu();
        let items = if index < menus.len() { menus.swap_remove(index).1 } else { crate::commands::help_menu() };
        let items: Vec<PopupItem> = items
            .into_iter()
            .map(|it| match it.cmd {
                None => PopupItem { label: String::new(), detail: String::new(), cmd: None },
                Some(cmd) => {
                    let mark = if crate::ui::checked(self, &cmd) == Some(true) { "\u{2714} " } else { "    " };
                    PopupItem { label: format!("{mark}{}", it.label), detail: it.shortcut.to_string(), cmd: Some(cmd) }
                }
            })
            .collect();
        let pos = self.menu_rects.get(index).map(|r| r.left_bottom()).unwrap_or(egui::pos2(4.0, 22.0));
        self.open_popup_at(items, pos);
        if let Some(p) = &mut self.popup {
            p.menu = Some(index);
        }
    }

    fn open_hotlist(&mut self) {
        let side = self.active;
        let mut items: Vec<PopupItem> = self
            .cfg
            .hotlist
            .iter()
            .map(|h| {
                if h.path.is_empty() {
                    PopupItem { label: String::new(), detail: String::new(), cmd: None }
                } else {
                    PopupItem { label: h.name.clone(), detail: h.path.clone(), cmd: Some(Cmd::GotoPath(h.path.clone())) }
                }
            })
            .collect();
        if !items.is_empty() {
            items.push(PopupItem { label: String::new(), detail: String::new(), cmd: None });
        }
        if self.atab().loc.fs_path().is_some() {
            items.push(PopupItem { label: format!("Add current directory ({})", self.atab().loc.caption()), detail: String::new(), cmd: Some(Cmd::AddToHotlist) });
        }
        items.push(PopupItem { label: "Configure...".into(), detail: String::new(), cmd: Some(Cmd::ConfigureHotlist) });
        let _ = side;
        self.open_popup(items);
    }

    fn open_history_menu(&mut self, side: Side) {
        let tab = self.tab(side);
        let items: Vec<PopupItem> = tab
            .history
            .iter()
            .enumerate()
            .rev()
            .map(|(i, l)| PopupItem {
                label: if i == tab.hist_pos { format!("> {}", l.display()) } else { l.display() },
                detail: String::new(),
                cmd: match l {
                    Location::Results { .. } => None,
                    _ => Some(Cmd::HistoryGoto(i)),
                },
            })
            .collect();
        self.open_popup(items);
    }

    fn open_drive_menu(&mut self, side: Side) {
        let items: Vec<PopupItem> = self
            .drives
            .clone()
            .iter()
            .map(|d| {
                let extra = self.drive_extra.get(&d.letter).cloned().unwrap_or_default();
                PopupItem {
                    label: format!("{}:  {}", d.letter.to_ascii_lowercase(), extra.label.unwrap_or_default()),
                    detail: extra.space.map(|(f, _)| format!("{} free", crate::format::kilo(f))).unwrap_or_default(),
                    cmd: Some(Cmd::GotoDrive(side, d.letter)),
                }
            })
            .collect();
        for d in self.drives.clone() {
            self.request_drive_info(d.letter);
        }
        self.active = side;
        let pos = self.panel_rects[side.index()].left_top() + egui::vec2(4.0, 40.0);
        self.open_popup_at(items, pos);
    }

    fn open_cmd_history(&mut self) {
        let items: Vec<PopupItem> = self
            .cfg
            .cmd_history
            .iter()
            .map(|c| PopupItem { label: c.clone(), detail: String::new(), cmd: Some(Cmd::Run(c.clone())) })
            .collect();
        if items.is_empty() {
            self.toast("Command line history is empty");
            return;
        }
        let pos = self.list_rects[self.active.index()].left_bottom() - egui::vec2(-20.0, 20.0 * items.len().min(15) as f32 + 30.0);
        self.cmd_history_popup = true;
        self.open_popup_at(items, pos);
    }

    /// Explorer's context menu for the selection (or the item under the
    /// cursor, or the current directory on `..`); an internal menu inside
    /// archives.
    /// After "New > Text Document" and the like: put the cursor on the one
    /// entry that appeared and rename it in place, as Explorer does.
    fn rename_new_entry(&mut self, side: Side, before: &HashSet<String>) {
        let tab = self.tab(side);
        if tab.loading.is_some() {
            return;
        }
        let new: Vec<usize> = tab.entries.iter().enumerate().filter(|(_, e)| !e.is_parent && !before.contains(&e.name)).map(|(i, _)| i).collect();
        if let [i] = new[..] {
            self.active = side;
            self.tab_mut(side).set_cursor(i);
            self.start_rename();
        }
    }

    pub fn open_context_menu(&mut self, at: Option<Pos2>) {
        use Cmd::*;
        let side = self.active;
        let background = std::mem::take(&mut self.context_background);
        let tab = self.atab();
        if let (Some(hwnd), true) = (self.hwnd, matches!(tab.loc, Location::Fs(_) | Location::Results { .. })) {
            let mut paths: Vec<PathBuf> = if background {
                Vec::new()
            } else {
                self.sources(side).into_iter().filter_map(|s| if let Source::Fs(p) = s { Some(p) } else { None }).collect()
            };
            // The shell menu needs items from a single folder.
            if let Some(parent) = paths.first().and_then(|p| p.parent()).map(Path::to_path_buf) {
                paths.retain(|p| p.parent() == Some(parent.as_path()));
            }
            let rh = crate::ui::row_height(self);
            let point = at.unwrap_or_else(|| {
                let r = self.list_rects[side.index()];
                let row = tab.cursor.saturating_sub(tab.top) as f32;
                egui::pos2(r.left() + 60.0, r.top() + (row + 1.0) * rh)
            });
            let client = ((point.x * self.ppp) as i32, (point.y * self.ppp) as i32);
            let extended = win::shift_down();
            if paths.is_empty() {
                // Nothing selected, or a click beside the files: the folder's
                // own menu, as on the empty area in Explorer (New, Properties...).
                let Some(dir) = tab.loc.fs_path().map(Path::to_path_buf) else { return };
                let before: HashSet<String> = tab.entries.iter().map(|e| e.name.clone()).collect();
                match win::shell_background_menu(hwnd, &dir, Some(client), extended) {
                    Ok(win::MenuOutcome::Invoked) => {
                        self.refresh_all();
                        self.rename_new_entry(side, &before);
                    }
                    Ok(_) => {}
                    Err(e) => self.toast(format!("Context menu: {e}")),
                }
                return;
            }
            match win::shell_context_menu(hwnd, &paths, Some(client), extended) {
                Ok(win::MenuOutcome::Rename) => self.start_rename(),
                Ok(win::MenuOutcome::Invoked) => self.refresh_all(),
                Ok(win::MenuOutcome::Cancelled) => {}
                Err(e) => self.toast(format!("Context menu: {e}")),
            }
            return;
        }
        let on_file = tab.cursor_entry().is_some_and(|e| !e.is_parent);
        let fs = matches!(tab.loc, Location::Fs(_));
        let mut items = Vec::new();
        let mut add = |label: &str, detail: &str, cmd: Cmd| items.push(PopupItem { label: label.into(), detail: detail.into(), cmd: Some(cmd) });
        if on_file {
            add("Open", "Enter", Open);
            add("View", "F3", View);
            add("Edit", "F4", Edit);
        }
        add("Copy", "Ctrl+C", ClipCopy);
        add("Cut", "Ctrl+X", ClipCut);
        add("Paste", "Ctrl+V", ClipPaste);
        if on_file {
            add("Rename", "Shift+F6", Rename);
            add("Delete", "Del", Delete);
            add("Pack...", "Alt+F5", Pack);
        }
        if fs {
            add("Open Explorer here", "", OpenExplorer);
            add("Open command prompt here", "", OpenTerminal);
        }
        add("Properties", "Alt+Enter", Properties);
        let pos = at.unwrap_or_else(|| {
            let r = self.list_rects[self.active.index()];
            r.left_top() + egui::vec2(40.0, 20.0)
        });
        self.open_popup_at(items, pos);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frame_no += 1;
        self.ppp = ctx.pixels_per_point();
        self.apply_theme(&ctx);
        ctx.input(|i| {
            let vp = i.viewport();
            if let (Some(outer), Some(inner)) = (vp.outer_rect, vp.inner_rect) {
                if vp.maximized != Some(true) && vp.minimized != Some(true) {
                    // Position of the window frame, size of the client area
                    // (matches ViewportBuilder::with_position / with_inner_size).
                    self.window_rect = Some(egui::Rect::from_min_size(outer.min, inner.size()));
                }
            }
            if let Some(m) = vp.maximized {
                self.maximized = m;
            }
        });
        self.poll_background(&ctx);
        self.handle_input(&ctx);
        self.bar_hits.clear();
        crate::ui::draw(self, ui);
        crate::dialogs::draw(self, &ctx);
        crate::ui::draw_popup(self, &ctx);
        // Viewer windows.
        let pal = self.pal.clone();
        self.viewers.retain(|v| v.show(&ctx, &pal));
        // Search window.
        if let Some(s) = self.search.take() {
            let mut keep = true;
            for action in s.show(&ctx, &pal) {
                use crate::search::SearchAction as A;
                match action {
                    A::None => {}
                    A::Close => keep = false,
                    A::GoTo(p) => {
                        if let (Some(dir), Some(name)) = (p.parent(), p.file_name()) {
                            let name = name.to_string_lossy().into_owned();
                            self.go(self.active, Location::fs(dir), Some(&name));
                        }
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                    A::Feed(items) => {
                        let n = items.len();
                        self.go(self.active, Location::Results { title: format!("Search results ({n})"), items: Arc::new(items) }, None);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                        keep = false;
                    }
                    A::View(p) => {
                        let id = self.next_viewer;
                        self.next_viewer += 1;
                        self.viewers.push(crate::viewer::ViewerWindow::open(id, p, &pal));
                    }
                }
            }
            self.cfg.search_history = s.history();
            if keep {
                self.search = Some(s);
            }
        }
        let now = Instant::now();
        if !self.delayed.is_empty() {
            let (due, wait): (Vec<_>, Vec<_>) = std::mem::take(&mut self.delayed).into_iter().partition(|(at, _)| *at <= now);
            self.delayed = wait;
            self.pending.extend(due.into_iter().map(|(_, c)| c));
            if let Some(next) = self.delayed.iter().map(|(at, _)| *at).min() {
                ctx.request_repaint_after(next.saturating_duration_since(now));
            }
        }
        let pending = std::mem::take(&mut self.pending);
        for cmd in pending {
            self.exec(&ctx, cmd);
        }
        if !self.steps.is_empty() {
            // Keep frames coming while a test script runs.
            ctx.request_repaint_after(self.step_at.saturating_duration_since(Instant::now()).max(Duration::from_millis(15)));
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            self.size_cancel.store(true, Ordering::Relaxed);
            self.save_config();
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if let Some(button) = self.release_pointer.take() {
            let pos = ctx.input(|i| i.pointer.latest_pos()).unwrap_or(Pos2::ZERO);
            raw_input.events.push(Event::PointerButton { pos, button, pressed: false, modifiers: Modifiers::NONE });
            raw_input.events.push(Event::PointerGone);
        }
        self.inject_steps(ctx, raw_input);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.pal.face.to_normalized_gamma_f32()
    }
}

// ---- free helpers ------------------------------------------------------------------------

fn letter_of(key: Key) -> Option<char> {
    let name = key.name();
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) if c.is_ascii_alphanumeric() => Some(c.to_ascii_lowercase()),
        _ => None,
    }
}

/// Splits a command line into the program and the raw argument string.
pub fn split_program(line: &str) -> (String, &str) {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return (rest[..end].to_string(), rest[end + 1..].trim_start());
        }
    }
    match line.find(' ') {
        Some(i) => (line[..i].to_string(), line[i + 1..].trim_start()),
        None => (line.to_string(), ""),
    }
}

/// Interprets the target text of the copy/move dialog.
pub fn parse_target(text: &str, src_dir: Option<&Path>, n_sources: usize) -> Result<(Dest, String), String> {
    let t = vfs::expand_env(text.trim().trim_matches('"'));
    if t.is_empty() {
        return Err("No target specified.".into());
    }
    let absolute = t.starts_with("\\\\") || (t.len() >= 2 && t.as_bytes()[1] == b':');
    let full = if absolute {
        PathBuf::from(&t)
    } else {
        match src_dir {
            Some(d) => vfs::resolve_input_path(d, &t),
            None => return Err("A relative target needs a source directory.".into()),
        }
    };
    let ends_sep = t.ends_with('\\') || t.ends_with('/');
    // A path that runs through an existing archive file targets the archive.
    let mut acc = PathBuf::new();
    let comps: Vec<_> = full.components().collect();
    for (i, c) in comps.iter().enumerate() {
        acc.push(c.as_os_str());
        if i + 1 < comps.len() && acc.is_file() && crate::archive::is_archive_name(&acc.to_string_lossy()) {
            let mut rest: Vec<String> = comps[i + 1..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
            let mut mask = "*.*".to_string();
            if rest.last().is_some_and(|l| crate::mask::has_wildcards(l)) {
                mask = rest.pop().unwrap();
            }
            return Ok((Dest::Archive { archive: acc, inner: rest.join("/") }, mask));
        }
    }
    if ends_sep || full.is_dir() {
        return Ok((Dest::Dir(vfs::normalize_dir(&full)), "*.*".into()));
    }
    let last = full.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let parent = full.parent().map(Path::to_path_buf).unwrap_or_else(|| full.clone());
    if crate::mask::has_wildcards(&last) || n_sources == 1 {
        Ok((Dest::Dir(parent), last))
    } else {
        Ok((Dest::Dir(full), "*.*".into()))
    }
}

fn compare_files(a: &Path, b: &Path) -> std::io::Result<Option<u64>> {
    use std::io::Read;
    let mut fa = std::io::BufReader::new(std::fs::File::open(a)?);
    let mut fb = std::io::BufReader::new(std::fs::File::open(b)?);
    let (mut ba, mut bb) = (vec![0u8; 65536], vec![0u8; 65536]);
    let mut offset = 0u64;
    loop {
        let na = read_full(&mut fa, &mut ba)?;
        let nb = read_full(&mut fb, &mut bb)?;
        let n = na.min(nb);
        if let Some(i) = (0..n).find(|&i| ba[i] != bb[i]) {
            return Ok(Some(offset + i as u64));
        }
        if na != nb {
            return Ok(Some(offset + n as u64));
        }
        if na == 0 {
            return Ok(None);
        }
        offset += na as u64;
    }
    fn read_full(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut n = 0;
        while n < buf.len() {
            let k = r.read(&mut buf[n..])?;
            if k == 0 {
                break;
            }
            n += k;
        }
        Ok(n)
    }
}

fn sha256_file(p: &Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub fn fxhash(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

fn save_png(path: &Path, img: &egui::ColorImage) -> image::ImageResult<()> {
    let [w, h] = img.size;
    let mut buf = Vec::with_capacity(w * h * 4);
    for c in &img.pixels {
        buf.extend_from_slice(&c.to_srgba_unmultiplied());
    }
    image::save_buffer(path, &buf, w as u32, h as u32, image::ColorType::Rgba8)
}

/// Fonts: Segoe UI (as Total Commander on Windows 11) with egui's defaults as
/// fallback, Consolas for the viewer.
pub fn setup_fonts(ctx: &egui::Context, size: f32) {
    use egui::{FontData, FontDefinitions, FontFamily, FontId, TextStyle};
    let mut fonts = FontDefinitions::default();
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    let load = |file: &str| std::fs::read(Path::new(&windir).join("Fonts").join(file)).ok();
    if let Some(bytes) = load("segoeui.ttf") {
        fonts.font_data.insert("segoe".into(), Arc::new(FontData::from_owned(bytes)));
        fonts.families.entry(FontFamily::Proportional).or_default().insert(0, "segoe".into());
    }
    if let Some(bytes) = load("seguisym.ttf") {
        fonts.font_data.insert("segoe-symbol".into(), Arc::new(FontData::from_owned(bytes)));
        fonts.families.entry(FontFamily::Proportional).or_default().push("segoe-symbol".into());
    }
    if let Some(bytes) = load("consola.ttf") {
        fonts.font_data.insert("consolas".into(), Arc::new(FontData::from_owned(bytes)));
        fonts.families.entry(FontFamily::Monospace).or_default().insert(0, "consolas".into());
    }
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(size - 2.0)),
            (TextStyle::Body, FontId::proportional(size)),
            (TextStyle::Button, FontId::proportional(size)),
            (TextStyle::Heading, FontId::proportional(size + 4.0)),
            (TextStyle::Monospace, FontId::monospace(size)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(8.0, 2.0);
        style.spacing.interact_size.y = 20.0;
        style.spacing.menu_margin = egui::Margin::same(3);
        style.interaction.tooltip_delay = 0.6;
        style.spacing.scroll = egui::style::ScrollStyle::solid();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn program_splitting() {
        assert_eq!(split_program("code ."), ("code".to_string(), "."));
        assert_eq!(split_program("\"C:\\Program Files\\x.exe\" -a b"), ("C:\\Program Files\\x.exe".to_string(), "-a b"));
        assert_eq!(split_program("notepad"), ("notepad".to_string(), ""));
    }

    #[test]
    fn copy_targets() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir(d.join("sub")).unwrap();
        let (dest, mask) = parse_target(&format!("{}\\sub\\*.*", d.display()), None, 3).unwrap();
        assert_eq!(dest, Dest::Dir(d.join("sub")));
        assert_eq!(mask, "*.*");
        let (dest, mask) = parse_target(&format!("{}\\sub\\renamed.txt", d.display()), None, 1).unwrap();
        assert_eq!(dest, Dest::Dir(d.join("sub")));
        assert_eq!(mask, "renamed.txt");
        let (dest, _) = parse_target(&format!("{}\\newdir", d.display()), None, 2).unwrap();
        assert_eq!(dest, Dest::Dir(d.join("newdir")));
        let (dest, mask) = parse_target("sub\\*.bak", Some(d), 2).unwrap();
        assert_eq!(dest, Dest::Dir(d.join("sub")));
        assert_eq!(mask, "*.bak");
        // Into an archive.
        let zip = d.join("a.zip");
        crate::archive::rebuild(&zip, &|_| true, &[], &mut |_, _| true, &AtomicBool::new(false)).unwrap();
        let (dest, mask) = parse_target(&format!("{}\\inner\\*.*", zip.display()), None, 2).unwrap();
        assert_eq!(dest, Dest::Archive { archive: zip.clone(), inner: "inner".into() });
        assert_eq!(mask, "*.*");
    }

    #[test]
    fn compare_detects_first_difference() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        std::fs::write(&a, b"hello world").unwrap();
        std::fs::write(&b, b"hello there").unwrap();
        assert_eq!(compare_files(&a, &b).unwrap(), Some(6));
        std::fs::write(&b, b"hello world").unwrap();
        assert_eq!(compare_files(&a, &b).unwrap(), None);
        std::fs::write(&b, b"hello world!").unwrap();
        assert_eq!(compare_files(&a, &b).unwrap(), Some(11));
    }

    #[test]
    fn sha256_known_value() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x");
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(sha256_file(&p).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
