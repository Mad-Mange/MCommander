//! Directory tree dialog (Alt+F10): pick a folder of the current drive.

use crate::theme::Palette;
use egui::{Key, Sense, Ui, pos2, vec2};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub enum TreeResult {
    Open,
    Close,
    Go(PathBuf),
}

pub struct TreeDialog {
    root: PathBuf,
    children: HashMap<PathBuf, Vec<PathBuf>>,
    expanded: HashSet<PathBuf>,
    selected: PathBuf,
    /// The directory the dialog was opened from; its ancestors are always
    /// listed, even when hidden (e.g. AppData).
    current: PathBuf,
    show_hidden: bool,
    /// Frames left in which to scroll the selection into view (the first
    /// frame of a modal is an invisible sizing pass).
    scroll_to_selected: u8,
}

fn subdirs(dir: &Path, show_hidden: bool) -> Vec<PathBuf> {
    use std::os::windows::fs::MetadataExt;
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    e.metadata().is_ok_and(|m| {
                        let a = m.file_attributes();
                        m.is_dir() && (show_hidden || a & (crate::format::ATTR_HIDDEN | crate::format::ATTR_SYSTEM) == 0)
                    })
                })
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| crate::format::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
    out
}

impl TreeDialog {
    pub fn new(current: &Path, show_hidden: bool) -> TreeDialog {
        let root = crate::vfs::root_of(current);
        let mut d = TreeDialog {
            root: root.clone(),
            children: HashMap::new(),
            expanded: HashSet::new(),
            selected: current.to_path_buf(),
            current: current.to_path_buf(),
            show_hidden,
            scroll_to_selected: 3,
        };
        // Expand the path down to the current directory.
        for anc in current.ancestors() {
            d.expanded.insert(anc.to_path_buf());
        }
        d.expanded.insert(root);
        d
    }

    fn kids(&mut self, p: &Path) -> Vec<PathBuf> {
        if !self.children.contains_key(p) {
            let mut k = subdirs(p, self.show_hidden);
            if let Some(next) = self.current.ancestors().find(|a| a.parent() == Some(p)) {
                if !k.iter().any(|c| c == next) {
                    k.push(next.to_path_buf());
                    k.sort_by(|a, b| crate::format::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
                }
            }
            self.children.insert(p.to_path_buf(), k);
        }
        self.children[p].clone()
    }

    /// Visible rows as (path, depth).
    fn rows(&mut self) -> Vec<(PathBuf, usize)> {
        let mut out = Vec::new();
        let mut stack = vec![(self.root.clone(), 0usize)];
        while let Some((p, depth)) = stack.pop() {
            out.push((p.clone(), depth));
            if self.expanded.contains(&p) {
                for k in self.kids(&p).into_iter().rev() {
                    stack.push((k, depth + 1));
                }
            }
        }
        out
    }

    pub fn ui(&mut self, ui: &mut Ui, pal: &Palette) -> TreeResult {
        let rows = self.rows();
        let sel = rows.iter().position(|(p, _)| *p == self.selected).unwrap_or(0);
        let mut result = TreeResult::Open;
        let (up, down, left, right, enter) = ui.input(|i| {
            (
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::Enter),
            )
        });
        if up && sel > 0 {
            self.selected = rows[sel - 1].0.clone();
            self.scroll_to_selected = 2;
        }
        if down && sel + 1 < rows.len() {
            self.selected = rows[sel + 1].0.clone();
            self.scroll_to_selected = 2;
        }
        if right {
            self.expanded.insert(self.selected.clone());
        }
        if left {
            if self.expanded.remove(&self.selected) {
            } else if let Some(parent) = self.selected.parent() {
                self.selected = parent.to_path_buf();
                self.scroll_to_selected = 2;
            }
        }
        if enter {
            result = TreeResult::Go(self.selected.clone());
        }
        let row_h = 18.0;
        egui::Frame::new().fill(pal.list_bg).stroke(egui::Stroke::new(1.0, pal.border)).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(420.0).auto_shrink([false, false]).show(ui, |ui| {
                for (p, depth) in &rows {
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
                    if *p == self.selected {
                        ui.painter().rect_filled(r, 0.0, pal.cursor_fill);
                        if self.scroll_to_selected > 0 {
                            ui.scroll_to_rect(r, Some(egui::Align::Center));
                            self.scroll_to_selected -= 1;
                        }
                    }
                    let x = r.left() + 6.0 + *depth as f32 * 16.0;
                    let has_kids = self.children.get(p).is_none_or(|k| !k.is_empty());
                    let open = self.expanded.contains(p);
                    let tri = egui::Rect::from_min_size(pos2(x, r.top()), vec2(14.0, row_h));
                    if has_kids {
                        ui.painter().text(tri.center(), egui::Align2::CENTER_CENTER, if open { "\u{25BE}" } else { "\u{25B8}" }, egui::FontId::proportional(12.0), pal.text_dim);
                    }
                    let icon = egui::Rect::from_min_size(pos2(x + 15.0, r.center().y - 7.0), vec2(14.0, 14.0));
                    crate::paint_icons::paint(ui.painter(), icon, crate::paint_icons::Icon::Folder, pal.text);
                    let name = if p.parent().is_none() {
                        p.to_string_lossy().into_owned()
                    } else {
                        p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                    };
                    ui.painter().text(pos2(icon.right() + 5.0, r.center().y), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(12.5), pal.text);
                    if resp.clicked() {
                        if let Some(ptr) = resp.interact_pointer_pos() {
                            if tri.contains(ptr) && has_kids {
                                if open {
                                    self.expanded.remove(p);
                                } else {
                                    self.expanded.insert(p.clone());
                                }
                            }
                        }
                        self.selected = p.clone();
                    }
                    if resp.double_clicked() {
                        result = TreeResult::Go(p.clone());
                    }
                }
            });
        });
        ui.add_space(6.0);
        ui.add(egui::Label::new(egui::RichText::new(self.selected.to_string_lossy()).weak()).truncate());
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new("Cancel").min_size(egui::vec2(84.0, 24.0))).clicked() {
                    result = TreeResult::Close;
                }
                if ui.add(egui::Button::new("OK").min_size(egui::vec2(84.0, 24.0))).clicked() {
                    result = TreeResult::Go(self.selected.clone());
                }
            });
        });
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_follow_expansion() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("a").join("deep")).unwrap();
        std::fs::create_dir_all(d.path().join("b")).unwrap();
        let mut t = TreeDialog::new(&d.path().join("a"), true);
        t.root = d.path().to_path_buf();
        let rows = t.rows();
        let names: Vec<String> = rows.iter().skip(1).map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["a", "deep", "b"]);
        t.expanded.remove(&d.path().join("a"));
        assert_eq!(t.rows().len(), 3);
    }
}
