//! Synchronize directories: compares two trees by size and date and copies
//! the differences in the chosen direction.

use crate::mask::MaskSet;
use crate::theme::Palette;
use egui::{RichText, Ui};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::SystemTime;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Equal,
    LeftNewer,
    RightNewer,
    LeftOnly,
    RightOnly,
    /// Same time, different size.
    Differ,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    ToRight,
    ToLeft,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub rel: String,
    pub left: Option<(u64, Option<SystemTime>)>,
    pub right: Option<(u64, Option<SystemTime>)>,
    pub state: State,
    pub action: Action,
}

fn default_action(s: State) -> Action {
    match s {
        State::LeftNewer | State::LeftOnly => Action::ToRight,
        State::RightNewer | State::RightOnly => Action::ToLeft,
        State::Equal | State::Differ => Action::None,
    }
}

type FileMap = BTreeMap<String, (String, u64, Option<SystemTime>)>;

fn collect(root: &Path, subdirs: bool, mask: &MaskSet) -> FileMap {
    let mut out = BTreeMap::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(root.join(&rel)) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            let name = e.file_name().to_string_lossy().into_owned();
            let r = rel.join(&name);
            if m.is_dir() {
                if subdirs && !m.file_type().is_symlink() {
                    stack.push(r);
                }
            } else if mask.matches(&name, false) {
                let s = r.to_string_lossy().into_owned();
                out.insert(s.to_lowercase(), (s, m.len(), m.modified().ok()));
            }
        }
    }
    out
}

/// Compares two directory trees (case-insensitive paths, 2 s time tolerance).
pub fn compare(left: &Path, right: &Path, subdirs: bool, mask: &MaskSet, ignore_date: bool) -> Vec<Item> {
    let l = collect(left, subdirs, mask);
    let r = collect(right, subdirs, mask);
    let mut keys: Vec<&String> = l.keys().chain(r.keys()).collect();
    keys.sort();
    keys.dedup();
    keys.into_iter()
        .map(|k| {
            let a = l.get(k);
            let b = r.get(k);
            let state = match (a, b) {
                (Some(_), None) => State::LeftOnly,
                (None, Some(_)) => State::RightOnly,
                (Some((_, sa, ta)), Some((_, sb, tb))) => {
                    let secs = |t: &Option<SystemTime>| t.and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
                    let diff = match (secs(ta), secs(tb)) {
                        (Some(x), Some(y)) => x - y,
                        _ => 0,
                    };
                    if ignore_date || diff.abs() <= 2 {
                        if sa == sb { State::Equal } else { State::Differ }
                    } else if diff > 0 {
                        State::LeftNewer
                    } else {
                        State::RightNewer
                    }
                }
                (None, None) => State::Equal,
            };
            let rel = a.or(b).map(|(s, _, _)| s.clone()).unwrap_or_default();
            Item {
                rel,
                left: a.map(|(_, s, t)| (*s, *t)),
                right: b.map(|(_, s, t)| (*s, *t)),
                state,
                action: default_action(state),
            }
        })
        .collect()
}

pub enum SyncResult {
    Open,
    Close,
    Start(Vec<(PathBuf, PathBuf)>),
}

pub struct SyncTool {
    left: String,
    right: String,
    mask: String,
    subdirs: bool,
    ignore_date: bool,
    show: [bool; 4],
    items: Vec<Item>,
    running: Option<Receiver<Vec<Item>>>,
    status: String,
}

impl SyncTool {
    pub fn new(left: &Path, right: &Path) -> SyncTool {
        SyncTool {
            left: left.to_string_lossy().into_owned(),
            right: right.to_string_lossy().into_owned(),
            mask: "*.*".into(),
            subdirs: true,
            ignore_date: false,
            show: [false, true, true, true],
            items: Vec::new(),
            running: None,
            status: "Press Compare to start.".into(),
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        let (l, r) = (PathBuf::from(self.left.trim()), PathBuf::from(self.right.trim()));
        if !l.is_dir() || !r.is_dir() {
            self.status = "Both directories must exist.".into();
            return;
        }
        let (tx, rx) = channel();
        let (subdirs, ignore, mask, ctx) = (self.subdirs, self.ignore_date, MaskSet::parse(&self.mask), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(compare(&l, &r, subdirs, &mask, ignore));
            ctx.request_repaint();
        });
        self.running = Some(rx);
        self.items.clear();
        self.status = "Comparing...".into();
    }

    fn visible(&self, it: &Item) -> bool {
        match it.state {
            State::Equal => self.show[0],
            State::LeftNewer | State::RightNewer | State::Differ => self.show[1],
            State::LeftOnly => self.show[2],
            State::RightOnly => self.show[3],
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, pal: &Palette) -> SyncResult {
        if let Some(rx) = &self.running {
            if let Ok(items) = rx.try_recv() {
                let count = |s: State| items.iter().filter(|i| i.state == s).count();
                self.status = format!(
                    "{} files: {} equal, {} newer left, {} newer right, {} different, {} left only, {} right only",
                    items.len(),
                    count(State::Equal),
                    count(State::LeftNewer),
                    count(State::RightNewer),
                    count(State::Differ),
                    count(State::LeftOnly),
                    count(State::RightOnly)
                );
                self.items = items;
                self.running = None;
            }
        }
        egui::Grid::new("sync-grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Left:");
            ui.add(egui::TextEdit::singleline(&mut self.left).desired_width(f32::INFINITY));
            ui.end_row();
            ui.label("Right:");
            ui.add(egui::TextEdit::singleline(&mut self.right).desired_width(f32::INFINITY));
            ui.end_row();
            ui.label("Files:");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.mask).desired_width(160.0));
                ui.checkbox(&mut self.subdirs, "Subdirectories");
                ui.checkbox(&mut self.ignore_date, "Ignore date (size only)");
            });
            ui.end_row();
        });
        ui.horizontal(|ui| {
            let busy = self.running.is_some();
            if ui.add_enabled(!busy, egui::Button::new("Compare").min_size(egui::vec2(96.0, 24.0))).clicked() {
                let ctx = ui.ctx().clone();
                self.start(&ctx);
            }
            if busy {
                ui.spinner();
            }
            ui.separator();
            ui.label("Show:");
            ui.toggle_value(&mut self.show[0], "= equal");
            ui.toggle_value(&mut self.show[1], "\u{2260} different");
            ui.toggle_value(&mut self.show[2], "\u{2192} left only");
            ui.toggle_value(&mut self.show[3], "\u{2190} right only");
        });
        ui.label(RichText::new(&self.status).weak());
        let fmt = |v: &Option<(u64, Option<SystemTime>)>| match v {
            Some((s, t)) => format!("{}  {}", crate::format::thousands(*s), t.map(crate::format::datetime).unwrap_or_default()),
            None => String::new(),
        };
        let rows: Vec<usize> = (0..self.items.len()).filter(|&i| self.visible(&self.items[i])).collect();
        egui::Frame::new().fill(pal.list_bg).stroke(egui::Stroke::new(1.0, pal.border)).inner_margin(4.0).show(ui, |ui| {
            ui.set_min_height(320.0);
            let row_h = 19.0;
            egui::ScrollArea::vertical().max_height(320.0).auto_shrink([false, false]).show_rows(ui, row_h, rows.len(), |ui, range| {
                let font = egui::FontId::proportional(12.5);
                let mono = egui::FontId::monospace(12.0);
                for (n, &i) in rows[range.clone()].iter().enumerate() {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), egui::Sense::hover());
                    if (range.start + n) % 2 == 1 {
                        ui.painter().rect_filled(r, 0.0, pal.hover.gamma_multiply(0.5));
                    }
                    // Columns: name | left size+date | direction | right size+date
                    let info_w = 200.0;
                    let arrow_w = 36.0;
                    let right_x = r.right() - info_w;
                    let arrow_x = right_x - arrow_w;
                    let left_x = arrow_x - info_w;
                    let it = &mut self.items[i];
                    let painter = ui.painter();
                    painter.with_clip_rect(egui::Rect::from_min_max(r.min, egui::pos2(left_x - 6.0, r.bottom()))).text(
                        egui::pos2(r.left() + 4.0, r.center().y),
                        egui::Align2::LEFT_CENTER,
                        &it.rel,
                        font.clone(),
                        pal.text,
                    );
                    painter.text(egui::pos2(arrow_x - 6.0, r.center().y), egui::Align2::RIGHT_CENTER, fmt(&it.left), mono.clone(), pal.text);
                    painter.text(egui::pos2(r.right() - 4.0, r.center().y), egui::Align2::RIGHT_CENTER, fmt(&it.right), mono.clone(), pal.text);
                    let (sym, color) = match it.action {
                        Action::ToRight => ("\u{2192}", pal.accent),
                        Action::ToLeft => ("\u{2190}", Color::GREEN),
                        Action::None if it.state == State::Equal => ("=", pal.text_dim),
                        Action::None => ("\u{2260}", pal.error),
                    };
                    let arect = egui::Rect::from_min_size(egui::pos2(arrow_x + 4.0, r.top() + 1.0), egui::vec2(arrow_w - 8.0, row_h - 2.0));
                    let resp = ui.interact(arect, egui::Id::unique(("sync-arrow", i)), egui::Sense::click());
                    let fill = if resp.hovered() { pal.pressed } else { pal.bar };
                    ui.painter().rect(arect, 3.0, fill, egui::Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
                    ui.painter().text(arect.center(), egui::Align2::CENTER_CENTER, sym, egui::FontId::proportional(14.0), color);
                    if resp.on_hover_text("Click to change the direction").clicked() {
                        it.action = match (it.action, it.left.is_some(), it.right.is_some()) {
                            (Action::None, true, _) => Action::ToRight,
                            (Action::None, false, _) => Action::ToLeft,
                            (Action::ToRight, _, true) => Action::ToLeft,
                            (Action::ToRight, _, false) => Action::None,
                            (Action::ToLeft, _, _) => Action::None,
                        };
                    }
                }
            });
        });
        let to_right = self.items.iter().filter(|i| i.action == Action::ToRight).count();
        let to_left = self.items.iter().filter(|i| i.action == Action::ToLeft).count();
        let mut result = SyncResult::Open;
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(format!("{to_right} file(s) to copy right, {to_left} to copy left"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new("Close").min_size(egui::vec2(84.0, 24.0))).clicked() {
                    result = SyncResult::Close;
                }
                let go = ui.add_enabled(to_left + to_right > 0 && self.running.is_none(), egui::Button::new("Synchronize").min_size(egui::vec2(110.0, 24.0)));
                if go.clicked() {
                    let (l, r) = (PathBuf::from(self.left.trim()), PathBuf::from(self.right.trim()));
                    let pairs = self
                        .items
                        .iter()
                        .filter_map(|it| match it.action {
                            Action::ToRight => Some((l.join(&it.rel), r.join(&it.rel))),
                            Action::ToLeft => Some((r.join(&it.rel), l.join(&it.rel))),
                            Action::None => None,
                        })
                        .collect();
                    result = SyncResult::Start(pairs);
                }
            });
        });
        result
    }
}

struct Color;
impl Color {
    const GREEN: egui::Color32 = egui::Color32::from_rgb(0x2E, 0x9E, 0x3E);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn touch(p: &Path, data: &[u8], age_secs: u64) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, data).unwrap();
        let t = SystemTime::now() - Duration::from_secs(age_secs);
        std::fs::OpenOptions::new().write(true).open(p).unwrap().set_modified(t).unwrap();
    }

    #[test]
    fn classifies_differences() {
        let d = tempfile::tempdir().unwrap();
        let (l, r) = (d.path().join("l"), d.path().join("r"));
        touch(&l.join("same.txt"), b"x", 1000);
        touch(&r.join("same.txt"), b"x", 1000);
        touch(&l.join("newer.txt"), b"new", 10);
        touch(&r.join("NEWER.txt"), b"old", 5000);
        touch(&l.join("sub").join("only_left.txt"), b"l", 10);
        touch(&r.join("only_right.txt"), b"r", 10);
        touch(&l.join("size.txt"), b"aa", 1000);
        touch(&r.join("size.txt"), b"a", 1000);
        let items = compare(&l, &r, true, &MaskSet::parse("*.*"), false);
        let state = |name: &str| items.iter().find(|i| i.rel.to_lowercase().ends_with(name)).unwrap().state;
        assert_eq!(state("same.txt"), State::Equal);
        assert_eq!(state("newer.txt"), State::LeftNewer);
        assert_eq!(state("only_left.txt"), State::LeftOnly);
        assert_eq!(state("only_right.txt"), State::RightOnly);
        assert_eq!(state("size.txt"), State::Differ);
        assert_eq!(items.iter().find(|i| i.state == State::LeftNewer).unwrap().action, Action::ToRight);
        let flat = compare(&l, &r, false, &MaskSet::parse("*.*"), false);
        assert!(flat.iter().all(|i| !i.rel.contains('\\')));
    }
}
