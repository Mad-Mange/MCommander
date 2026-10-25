//! Find files (Alt+F7): a separate window that searches on a background thread.

use crate::mask::MaskSet;
use crate::theme::Palette;
use egui::{Key, RichText, ViewportBuilder, ViewportId};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{Receiver, channel};

pub enum SearchAction {
    None,
    Close,
    GoTo(PathBuf),
    Feed(Vec<PathBuf>),
    View(PathBuf),
}

#[derive(Clone, Debug)]
pub struct Query {
    pub mask: MaskSet,
    pub text: Option<String>,
    pub case_sensitive: bool,
    pub regex: bool,
    pub subdirs: bool,
    pub include_dirs: bool,
    pub include_hidden: bool,
}

struct Running {
    rx: Receiver<Vec<PathBuf>>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    scanned: Arc<AtomicU64>,
}

/// The Find Files window; its state is shared with a deferred viewport.
pub struct SearchHandle {
    id: ViewportId,
    state: Arc<Mutex<SearchWindow>>,
}

impl SearchHandle {
    pub fn new(start: PathBuf, history: Vec<String>, pal: &Palette) -> SearchHandle {
        let w = SearchWindow::new(start, history, pal.clone());
        SearchHandle { id: w.id, state: Arc::new(Mutex::new(w)) }
    }

    pub fn viewport_id(&self) -> ViewportId {
        self.id
    }

    pub fn history(&self) -> Vec<String> {
        self.state.lock().unwrap().history.clone()
    }

    /// Keeps the window alive and returns the actions the user triggered.
    pub fn show(&self, ctx: &egui::Context, pal: &Palette) -> Vec<SearchAction> {
        let actions = {
            let mut w = self.state.lock().unwrap();
            w.pal = pal.clone();
            std::mem::take(&mut w.actions)
        };
        if actions.iter().any(|a| matches!(a, SearchAction::Close)) {
            return actions;
        }
        let builder = ViewportBuilder::default().with_title("Find Files").with_inner_size([780.0, 620.0]).with_min_inner_size([480.0, 360.0]);
        let state = self.state.clone();
        ctx.show_viewport_deferred(self.id, builder, move |ui, _| {
            state.lock().unwrap().ui(ui);
        });
        actions
    }
}

pub struct SearchWindow {
    id: ViewportId,
    pal: Palette,
    actions: Vec<SearchAction>,
    pattern: String,
    start: String,
    text: String,
    use_text: bool,
    case_sensitive: bool,
    regex: bool,
    subdirs: bool,
    include_dirs: bool,
    include_hidden: bool,
    running: Option<Running>,
    results: Vec<PathBuf>,
    selected: usize,
    status: String,
    history: Vec<String>,
    focus_pattern: bool,
}

impl SearchWindow {
    fn new(start: PathBuf, history: Vec<String>, pal: Palette) -> SearchWindow {
        SearchWindow {
            id: ViewportId::from_hash_of("find-files"),
            pal,
            actions: Vec::new(),
            pattern: history.first().cloned().unwrap_or_else(|| "*.*".into()),
            start: start.to_string_lossy().into_owned(),
            text: String::new(),
            use_text: false,
            case_sensitive: false,
            regex: false,
            subdirs: true,
            include_dirs: true,
            include_hidden: false,
            running: None,
            results: Vec::new(),
            selected: 0,
            status: String::new(),
            history,
            focus_pattern: true,
        }
    }

    fn start_search(&mut self, ctx: &egui::Context) {
        crate::config::Config::push_history(&mut self.history, &self.pattern);
        self.results.clear();
        self.selected = 0;
        let query = Query {
            mask: MaskSet::parse_loose(&self.pattern),
            text: (self.use_text && !self.text.is_empty()).then(|| self.text.clone()),
            case_sensitive: self.case_sensitive,
            regex: self.regex,
            subdirs: self.subdirs,
            include_dirs: self.include_dirs && !(self.use_text && !self.text.is_empty()),
            include_hidden: self.include_hidden,
        };
        let roots: Vec<PathBuf> = self.start.split(';').map(|s| PathBuf::from(s.trim())).filter(|p| p.is_dir()).collect();
        if roots.is_empty() {
            self.status = "The start directory does not exist.".into();
            return;
        }
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let scanned = Arc::new(AtomicU64::new(0));
        let (c2, d2, s2, ctx2, id) = (cancel.clone(), done.clone(), scanned.clone(), ctx.clone(), self.id);
        std::thread::spawn(move || {
            run_search(&roots, &query, &c2, &s2, |batch| {
                let _ = tx.send(batch);
                ctx2.request_repaint_of(id);
            });
            d2.store(true, Ordering::Relaxed);
            ctx2.request_repaint_of(id);
        });
        self.running = Some(Running { rx, cancel, done, scanned });
        self.status = "Searching...".into();
    }

    fn poll(&mut self) {
        let Some(r) = &self.running else { return };
        while let Ok(batch) = r.rx.try_recv() {
            self.results.extend(batch);
        }
        let scanned = r.scanned.load(Ordering::Relaxed);
        if r.done.load(Ordering::Relaxed) {
            let cancelled = r.cancel.load(Ordering::Relaxed);
            self.status = format!(
                "{}: {} found, {} items checked",
                if cancelled { "Cancelled" } else { "Done" },
                self.results.len(),
                scanned
            );
            self.running = None;
        } else {
            self.status = format!("Searching... {} found, {} items checked", self.results.len(), scanned);
        }
    }

    fn act(&mut self, ctx: &egui::Context, action: SearchAction) {
        self.actions.push(action);
        ctx.request_repaint_of(ViewportId::ROOT);
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        self.poll();
        let pal = &self.pal.clone();
        let mut action = SearchAction::None;
        {
            let ctx = ui.ctx().clone();
            if ctx.input(|i| i.viewport().close_requested()) {
                self.cancel();
                self.act(&ctx, SearchAction::Close);
                return;
            }
            if self.running.is_some() {
                ctx.request_repaint_after(std::time::Duration::from_millis(150));
            }
            egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.face).inner_margin(12.0)).show(ui, |ui| {
                egui::Grid::new("findgrid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label("Search for:");
                    let resp = ui.add(egui::TextEdit::singleline(&mut self.pattern).desired_width(f32::INFINITY).hint_text("*.txt *.md | *.bak   (no wildcards = part of name)"));
                    if self.focus_pattern {
                        resp.request_focus();
                        self.focus_pattern = false;
                    }
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && self.running.is_none() {
                        self.start_search(&ctx);
                    }
                    ui.end_row();
                    ui.label("Search in:");
                    ui.add(egui::TextEdit::singleline(&mut self.start).desired_width(f32::INFINITY));
                    ui.end_row();
                    ui.label("");
                    ui.horizontal_wrapped(|ui| {
                        ui.checkbox(&mut self.subdirs, "Search subdirectories");
                        ui.checkbox(&mut self.include_dirs, "Find directories too");
                        ui.checkbox(&mut self.include_hidden, "Include hidden/system");
                    });
                    ui.end_row();
                    ui.checkbox(&mut self.use_text, "Find text:");
                    ui.add_enabled(self.use_text, egui::TextEdit::singleline(&mut self.text).desired_width(f32::INFINITY));
                    ui.end_row();
                    ui.label("");
                    ui.add_enabled_ui(self.use_text, |ui| {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut self.case_sensitive, "Case sensitive");
                            ui.checkbox(&mut self.regex, "RegEx");
                        });
                    });
                    ui.end_row();
                });
                if !self.history.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Recent:").weak());
                        for h in self.history.clone().iter().take(8) {
                            if ui.small_button(h).clicked() {
                                self.pattern = h.clone();
                            }
                        }
                    });
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if self.running.is_none() {
                        if ui.add(egui::Button::new("Start search").min_size(egui::vec2(110.0, 26.0))).clicked() {
                            self.start_search(&ctx);
                        }
                    } else if ui.add(egui::Button::new("Cancel").min_size(egui::vec2(110.0, 26.0))).clicked() {
                        self.cancel();
                    }
                    let has = !self.results.is_empty();
                    if ui.add_enabled(has, egui::Button::new("Go to file")).clicked() {
                        action = SearchAction::GoTo(self.results[self.selected.min(self.results.len() - 1)].clone());
                    }
                    if ui.add_enabled(has, egui::Button::new("Feed to listbox")).clicked() {
                        self.cancel();
                        action = SearchAction::Feed(self.results.clone());
                    }
                    if ui.add_enabled(has, egui::Button::new("View (F3)")).clicked() {
                        action = SearchAction::View(self.results[self.selected.min(self.results.len() - 1)].clone());
                    }
                    if self.running.is_some() {
                        ui.spinner();
                    }
                });
                ui.label(RichText::new(&self.status).weak());
                ui.add_space(4.0);
                // Results.
                let row_h = 18.0;
                egui::Frame::new().fill(pal.list_bg).stroke(egui::Stroke::new(1.0, pal.border)).show(ui, |ui| {
                    ui.set_min_height(ui.available_height());
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_h, self.results.len(), |ui, range| {
                        for i in range {
                            let text = self.results[i].to_string_lossy().into_owned();
                            let resp = ui.add_sized([ui.available_width(), row_h], egui::Button::selectable(i == self.selected, text).frame_when_inactive(false));
                            if resp.clicked() {
                                self.selected = i;
                            }
                            if resp.double_clicked() {
                                action = SearchAction::GoTo(self.results[i].clone());
                            }
                        }
                    });
                });
            });
            // Keyboard in the result list.
            if !ctx.text_edit_focused() && !self.results.is_empty() {
                ctx.input(|i| {
                    if i.key_pressed(Key::ArrowDown) {
                        self.selected = (self.selected + 1).min(self.results.len() - 1);
                    }
                    if i.key_pressed(Key::ArrowUp) {
                        self.selected = self.selected.saturating_sub(1);
                    }
                    if i.key_pressed(Key::Enter) {
                        action = SearchAction::GoTo(self.results[self.selected].clone());
                    }
                    if i.key_pressed(Key::F3) {
                        action = SearchAction::View(self.results[self.selected].clone());
                    }
                });
            }
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.cancel();
                action = SearchAction::Close;
            }
            if !matches!(action, SearchAction::None) {
                self.act(&ctx, action);
            }
        }
    }

    fn cancel(&mut self) {
        if let Some(r) = &self.running {
            r.cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// Walks the roots and reports matches in batches.
pub fn run_search(roots: &[PathBuf], q: &Query, cancel: &AtomicBool, scanned: &AtomicU64, mut emit: impl FnMut(Vec<PathBuf>)) {
    use std::os::windows::fs::MetadataExt;
    let matcher = q.text.as_ref().map(|t| TextMatcher::new(t, q.case_sensitive, q.regex));
    if let Some(Err(e)) = &matcher {
        let _ = e;
        return;
    }
    let matcher = matcher.map(|m| m.unwrap());
    let mut batch = Vec::new();
    let mut last_emit = std::time::Instant::now();
    let mut stack: Vec<PathBuf> = roots.to_vec();
    while let Some(dir) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let Ok(meta) = e.metadata() else { continue };
            scanned.fetch_add(1, Ordering::Relaxed);
            let attrs = meta.file_attributes();
            if !q.include_hidden && attrs & (crate::format::ATTR_HIDDEN | crate::format::ATTR_SYSTEM) != 0 {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = meta.is_dir();
            if is_dir && q.subdirs && attrs & crate::format::ATTR_REPARSE == 0 {
                stack.push(e.path());
            }
            if is_dir && !q.include_dirs {
                continue;
            }
            if !q.mask.matches(&name, is_dir) {
                continue;
            }
            if let Some(m) = &matcher {
                if is_dir || !m.file_contains(&e.path(), cancel) {
                    continue;
                }
            }
            batch.push(e.path());
            if batch.len() >= 200 || last_emit.elapsed().as_millis() > 200 {
                emit(std::mem::take(&mut batch));
                last_emit = std::time::Instant::now();
            }
        }
    }
    if !batch.is_empty() {
        emit(batch);
    }
}

pub enum TextMatcher {
    Bytes { needles: Vec<Vec<u8>>, case_sensitive: bool },
    Regex(regex::bytes::Regex),
}

impl TextMatcher {
    pub fn new(text: &str, case_sensitive: bool, regex: bool) -> Result<TextMatcher, String> {
        if regex {
            let pat = if case_sensitive { text.to_string() } else { format!("(?i){text}") };
            return regex::bytes::Regex::new(&pat).map(TextMatcher::Regex).map_err(|e| e.to_string());
        }
        let norm = |s: &str| if case_sensitive { s.to_string() } else { s.to_lowercase() };
        let t = norm(text);
        let utf8 = t.as_bytes().to_vec();
        let utf16: Vec<u8> = t.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let (ansi, _, _) = encoding_rs::WINDOWS_1252.encode(&t);
        let mut needles = vec![utf8, utf16];
        if ansi.as_ref() != needles[0].as_slice() {
            needles.push(ansi.into_owned());
        }
        Ok(TextMatcher::Bytes { needles, case_sensitive })
    }

    pub fn file_contains(&self, path: &Path, cancel: &AtomicBool) -> bool {
        let Ok(mut f) = std::fs::File::open(path) else { return false };
        let overlap = 1024;
        let mut buf = vec![0u8; 1 << 20];
        let mut carry: Vec<u8> = Vec::new();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return false;
            }
            let n = match f.read(&mut buf) {
                Ok(0) | Err(_) => return false,
                Ok(n) => n,
            };
            let mut chunk = std::mem::take(&mut carry);
            chunk.extend_from_slice(&buf[..n]);
            if self.matches(&chunk) {
                return true;
            }
            let keep = chunk.len().min(overlap);
            carry = chunk[chunk.len() - keep..].to_vec();
        }
    }

    fn matches(&self, data: &[u8]) -> bool {
        match self {
            TextMatcher::Regex(re) => re.is_match(data),
            TextMatcher::Bytes { needles, case_sensitive } => {
                let lowered;
                let hay: &[u8] = if *case_sensitive {
                    data
                } else {
                    lowered = data.to_ascii_lowercase();
                    &lowered
                };
                needles.iter().any(|n| !n.is_empty() && hay.windows(n.len()).any(|w| w == n.as_slice()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_name_and_content() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir_all(d.join("a").join("b")).unwrap();
        std::fs::write(d.join("a").join("notes.txt"), "Hello World").unwrap();
        std::fs::write(d.join("a").join("b").join("other.txt"), "nothing here").unwrap();
        let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain("say HELLO".encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
        std::fs::write(d.join("a").join("b").join("wide.txt"), utf16).unwrap();
        std::fs::write(d.join("readme.md"), "hello").unwrap();

        let run = |q: Query| {
            let mut out = Vec::new();
            run_search(&[d.to_path_buf()], &q, &AtomicBool::new(false), &AtomicU64::new(0), |b| out.extend(b));
            let mut names: Vec<String> = out.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
            names.sort();
            names
        };
        let base = Query {
            mask: MaskSet::parse_loose("*.txt"),
            text: None,
            case_sensitive: false,
            regex: false,
            subdirs: true,
            include_dirs: false,
            include_hidden: true,
        };
        assert_eq!(run(base.clone()), ["notes.txt", "other.txt", "wide.txt"]);
        assert_eq!(run(Query { text: Some("hello".into()), ..base.clone() }), ["notes.txt", "wide.txt"]);
        assert_eq!(run(Query { text: Some("Hello".into()), case_sensitive: true, ..base.clone() }), ["notes.txt"]);
        assert_eq!(run(Query { text: Some(r"W\w+d".into()), regex: true, ..base.clone() }), ["notes.txt"]);
        assert_eq!(run(Query { subdirs: false, mask: MaskSet::parse_loose("read"), ..base.clone() }), ["readme.md"]);
        assert_eq!(run(Query { mask: MaskSet::parse_loose("b"), include_dirs: true, ..base }), ["b"]);
    }
}
