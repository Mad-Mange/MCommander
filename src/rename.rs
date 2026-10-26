//! Multi-Rename Tool (Ctrl+M): rename masks with placeholders, search and
//! replace (optionally regex), case conversion and a counter, with preview.

use crate::theme::Palette;
use crate::vfs::FileEntry;
use egui::{RichText, Ui};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaseMode {
    Unchanged,
    Lower,
    Upper,
    FirstUpper,
    WordsUpper,
}

impl CaseMode {
    const ALL: [CaseMode; 5] = [Self::Unchanged, Self::Lower, Self::Upper, Self::FirstUpper, Self::WordsUpper];
    fn label(self) -> &'static str {
        match self {
            Self::Unchanged => "Unchanged",
            Self::Lower => "lowercase",
            Self::Upper => "UPPERCASE",
            Self::FirstUpper => "First letter uppercase",
            Self::WordsUpper => "First Of Each Word Uppercase",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Rules {
    pub name_mask: String,
    pub ext_mask: String,
    pub search: String,
    pub replace: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub case: CaseMode,
    pub counter_start: i64,
    pub counter_step: i64,
    pub counter_digits: usize,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            name_mask: "[N]".into(),
            ext_mask: "[E]".into(),
            search: String::new(),
            replace: String::new(),
            regex: false,
            case_sensitive: false,
            case: CaseMode::Unchanged,
            counter_start: 1,
            counter_step: 1,
            counter_digits: 1,
        }
    }
}

pub enum RenameResult {
    Open,
    Close,
    Done(String),
    Error(String),
}

pub struct RenameTool {
    items: Vec<(PathBuf, FileEntry)>,
    rules: Rules,
    /// Focus the name mask on the first frames (modals start with a sizing pass).
    focus: u8,
}

/// Characters `from..to` (1-based, inclusive; negative counts from the end).
fn substr(s: &str, spec: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len() as i64;
    let idx = |v: i64| -> i64 { if v < 0 { n + v + 1 } else { v } };
    let parse = |t: &str| t.trim().parse::<i64>().ok();
    let (from, to) = if let Some((a, len)) = spec.split_once(',') {
        let a = idx(parse(a).unwrap_or(1));
        (a, a + parse(len).unwrap_or(n) - 1)
    } else if let Some(rest) = spec.strip_prefix('-') {
        // "[N-3-]" style: negative start.
        match rest.split_once('-') {
            Some((a, b)) => (idx(-parse(a).unwrap_or(1)), if b.is_empty() { n } else { idx(parse(b).unwrap_or(n)) }),
            None => (idx(-parse(rest).unwrap_or(1)), idx(-parse(rest).unwrap_or(1))),
        }
    } else if let Some((a, b)) = spec.split_once('-') {
        (idx(parse(a).unwrap_or(1)), if b.is_empty() { n } else { idx(parse(b).unwrap_or(n)) })
    } else {
        let a = idx(parse(spec).unwrap_or(1));
        (a, a)
    };
    let from = from.max(1);
    let to = to.min(n);
    if from > to {
        return String::new();
    }
    chars[(from - 1) as usize..to as usize].iter().collect()
}

fn two(v: u32) -> String {
    format!("{v:02}")
}

/// Expands placeholders: [N] [N2-5] [N2,3] [E] [C] [C10+5:3] [P] [G] [Y] [M] [D] [h] [m] [s] [[.
pub fn expand(mask: &str, path: &Path, e: &FileEntry, index: usize, rules: &Rules) -> String {
    use chrono::{Datelike, Timelike};
    let (stem, ext) = crate::format::split_ext(&e.name, e.is_dir);
    let dt: Option<chrono::DateTime<chrono::Local>> = e.modified.map(Into::into);
    let mut out = String::new();
    let mut rest = mask;
    while let Some(start) = rest.find('[') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(stripped) = after.strip_prefix('[') {
            out.push('[');
            rest = stripped;
            continue;
        }
        let Some(end) = after.find(']') else {
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let tag = &after[..end];
        rest = &after[end + 1..];
        let mut chars = tag.chars();
        let head = chars.next().unwrap_or(' ');
        let spec = chars.as_str();
        let value = match head {
            'N' if spec.is_empty() => stem.to_string(),
            'N' => substr(stem, spec),
            'E' if spec.is_empty() => ext.to_string(),
            'E' => substr(ext, spec),
            'C' => {
                let (mut s, mut step, mut digits) = (rules.counter_start, rules.counter_step, rules.counter_digits);
                if !spec.is_empty() {
                    let (nums, d) = spec.split_once(':').unwrap_or((spec, ""));
                    let (a, b) = nums.split_once('+').unwrap_or((nums, ""));
                    if let Ok(v) = a.parse() {
                        s = v;
                    }
                    if let Ok(v) = b.parse() {
                        step = v;
                    }
                    if let Ok(v) = d.parse() {
                        digits = v;
                    }
                }
                format!("{:0width$}", s + step * index as i64, width = digits)
            }
            'P' => path.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            'G' => path
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            'Y' => dt.map(|d| d.year().to_string()).unwrap_or_default(),
            'M' => dt.map(|d| two(d.month())).unwrap_or_default(),
            'D' => dt.map(|d| two(d.day())).unwrap_or_default(),
            'h' => dt.map(|d| two(d.hour())).unwrap_or_default(),
            'm' => dt.map(|d| two(d.minute())).unwrap_or_default(),
            's' => dt.map(|d| two(d.second())).unwrap_or_default(),
            _ => format!("[{tag}]"),
        };
        out.push_str(&value);
    }
    out.push_str(rest);
    out
}

fn apply_case(s: &str, mode: CaseMode) -> String {
    match mode {
        CaseMode::Unchanged => s.to_string(),
        CaseMode::Lower => s.to_lowercase(),
        CaseMode::Upper => s.to_uppercase(),
        CaseMode::FirstUpper => {
            let lower = s.to_lowercase();
            let mut c = lower.chars();
            match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect(),
                None => String::new(),
            }
        }
        CaseMode::WordsUpper => {
            let mut out = String::new();
            let mut start = true;
            for ch in s.to_lowercase().chars() {
                if start && ch.is_alphanumeric() {
                    out.extend(ch.to_uppercase());
                    start = false;
                } else {
                    out.push(ch);
                }
                if ch.is_whitespace() || ch == '_' || ch == '-' || ch == '.' {
                    start = true;
                }
            }
            out
        }
    }
}

/// `$2_$1` -> `${2}_${1}`: the regex crate would otherwise read `$2_` as a
/// group named "2_".
fn braced_groups(replacement: &str) -> String {
    let mut out = String::new();
    let mut chars = replacement.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' && chars.peek().is_some_and(char::is_ascii_digit) {
            out.push_str("${");
            while let Some(d) = chars.peek().copied().filter(char::is_ascii_digit) {
                out.push(d);
                chars.next();
            }
            out.push('}');
        } else {
            out.push(c);
        }
    }
    out
}

/// Computes the new name for one entry.
pub fn new_name(path: &Path, e: &FileEntry, index: usize, rules: &Rules) -> Result<String, String> {
    let name = expand(&rules.name_mask, path, e, index, rules);
    let ext = expand(&rules.ext_mask, path, e, index, rules);
    let mut full = if ext.is_empty() { name } else { format!("{name}.{ext}") };
    if !rules.search.is_empty() {
        if rules.regex {
            let pattern = if rules.case_sensitive { rules.search.clone() } else { format!("(?i){}", rules.search) };
            let re = regex::Regex::new(&pattern).map_err(|e| format!("Invalid regular expression: {e}"))?;
            full = re.replace_all(&full, braced_groups(&rules.replace).as_str()).into_owned();
        } else if rules.case_sensitive {
            full = full.replace(&rules.search, &rules.replace);
        } else {
            let lower = full.to_lowercase();
            let needle = rules.search.to_lowercase();
            let mut out = String::new();
            let mut i = 0;
            while let Some(pos) = lower[i..].find(&needle) {
                out.push_str(&full[i..i + pos]);
                out.push_str(&rules.replace);
                i += pos + needle.len();
            }
            out.push_str(&full[i..]);
            full = out;
        }
    }
    let full = apply_case(&full, rules.case);
    if full.trim().is_empty() {
        return Err("empty name".into());
    }
    if full.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) {
        return Err("invalid character".into());
    }
    Ok(full)
}

impl RenameTool {
    pub fn new(items: Vec<(PathBuf, FileEntry)>) -> RenameTool {
        RenameTool { items, rules: Rules::default(), focus: 2 }
    }

    fn preview(&self) -> Vec<Result<String, String>> {
        let results: Vec<Result<String, String>> =
            self.items.iter().enumerate().map(|(i, (p, e))| new_name(p, e, i, &self.rules)).collect();
        // Flag duplicates (case-insensitive, as on NTFS).
        let mut seen = HashSet::new();
        let mut dup = HashSet::new();
        for (i, r) in results.iter().enumerate() {
            if let Ok(n) = r {
                let key = self.items[i].0.with_file_name(n).to_string_lossy().to_lowercase();
                if !seen.insert(key.clone()) {
                    dup.insert(key);
                }
            }
        }
        results
            .into_iter()
            .enumerate()
            .map(|(i, r)| match r {
                Ok(n) if dup.contains(&self.items[i].0.with_file_name(&n).to_string_lossy().to_lowercase()) => Err(format!("{n} (duplicate)")),
                other => other,
            })
            .collect()
    }

    pub fn ui(&mut self, ui: &mut Ui, pal: &Palette) -> RenameResult {
        let r = &mut self.rules;
        egui::Grid::new("mrt").num_columns(4).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Rename mask: file name");
            let id = egui::Id::unique("mrt-name-mask");
            crate::dialogs::focus_select_modal(ui, id, std::mem::take(&mut self.focus) > 0, 0, r.name_mask.chars().count());
            ui.add(egui::TextEdit::singleline(&mut r.name_mask).id(id).desired_width(220.0));
            ui.label("Extension");
            ui.add(egui::TextEdit::singleline(&mut r.ext_mask).desired_width(120.0));
            ui.end_row();
            ui.label("Search for");
            ui.add(egui::TextEdit::singleline(&mut r.search).desired_width(220.0));
            ui.label("Replace with");
            ui.add(egui::TextEdit::singleline(&mut r.replace).desired_width(120.0));
            ui.end_row();
            ui.label("");
            ui.horizontal(|ui| {
                ui.checkbox(&mut r.regex, "RegEx");
                ui.checkbox(&mut r.case_sensitive, "Case sensitive");
            });
            ui.label("Upper/lowercase");
            egui::ComboBox::from_id_salt("mrt-case").selected_text(r.case.label()).show_ui(ui, |ui| {
                for m in CaseMode::ALL {
                    ui.selectable_value(&mut r.case, m, m.label());
                }
            });
            ui.end_row();
            ui.label("Counter [C]: start");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut r.counter_start));
                ui.label("step");
                ui.add(egui::DragValue::new(&mut r.counter_step));
                ui.label("digits");
                ui.add(egui::DragValue::new(&mut r.counter_digits).range(1..=10));
            });
            ui.end_row();
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Placeholders:").weak());
            for (tag, tip) in [
                ("[N]", "name"),
                ("[N1-3]", "chars 1-3 of name"),
                ("[E]", "extension"),
                ("[C]", "counter"),
                ("[P]", "parent folder"),
                ("[Y]", "year"),
                ("[M]", "month"),
                ("[D]", "day"),
                ("[h]", "hour"),
                ("[m]", "minute"),
                ("[s]", "second"),
            ] {
                if ui.small_button(tag).on_hover_text(tip).clicked() {
                    r.name_mask.push_str(tag);
                }
            }
        });
        ui.add_space(6.0);
        let preview = self.preview();
        let errors = preview.iter().filter(|p| p.is_err()).count();
        egui::Frame::new().fill(pal.list_bg).stroke(egui::Stroke::new(1.0, pal.border)).inner_margin(4.0).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
                egui::Grid::new("mrt-preview").num_columns(2).striped(true).min_col_width(360.0).show(ui, |ui| {
                    ui.label(RichText::new("Old name").strong());
                    ui.label(RichText::new("New name").strong());
                    ui.end_row();
                    for ((_, e), p) in self.items.iter().zip(&preview) {
                        ui.label(&e.name);
                        match p {
                            Ok(n) if *n != e.name => ui.label(RichText::new(n).color(pal.accent)),
                            Ok(n) => ui.label(n),
                            Err(err) => ui.label(RichText::new(format!("! {err}")).color(pal.error)),
                        };
                        ui.end_row();
                    }
                });
            });
        });
        let mut result = RenameResult::Open;
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(format!("{} file(s)", self.items.len()));
            if errors > 0 {
                ui.label(RichText::new(format!("{errors} problem(s)")).color(pal.error));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new("Close").min_size(egui::vec2(84.0, 24.0))).clicked() {
                    result = RenameResult::Close;
                }
                let start = ui.add_enabled(errors == 0, egui::Button::new("Start!").min_size(egui::vec2(84.0, 24.0)));
                if start.clicked() {
                    result = self.execute(&preview);
                }
            });
        });
        result
    }

    /// Renames in two phases (via temporary names) so chains and swaps work.
    fn execute(&self, preview: &[Result<String, String>]) -> RenameResult {
        let mut plan = Vec::new();
        for ((path, e), p) in self.items.iter().zip(preview) {
            let Ok(new) = p else { continue };
            if *new == e.name {
                continue;
            }
            let target = path.with_file_name(new);
            let is_case_only = new.eq_ignore_ascii_case(&e.name);
            let renamed_away = self.items.iter().any(|(p2, _)| *p2 == target);
            if target.exists() && !is_case_only && !renamed_away {
                return RenameResult::Error(format!("\"{new}\" already exists - nothing was renamed."));
            }
            plan.push((path.clone(), target));
        }
        if plan.is_empty() {
            return RenameResult::Close;
        }
        let mut errors = Vec::new();
        let mut staged = Vec::new();
        for (i, (from, to)) in plan.iter().enumerate() {
            let tmp = from.with_file_name(format!("~mcren{i}_{}", std::process::id()));
            match std::fs::rename(from, &tmp) {
                Ok(()) => staged.push((tmp, to.clone(), from.clone())),
                Err(e) => errors.push(format!("{}: {e}", from.display())),
            }
        }
        let mut done = 0;
        for (tmp, to, from) in staged {
            match std::fs::rename(&tmp, &to) {
                Ok(()) => done += 1,
                Err(e) => {
                    errors.push(format!("{}: {e}", to.display()));
                    let _ = std::fs::rename(&tmp, &from);
                }
            }
        }
        if errors.is_empty() {
            RenameResult::Done(format!("Renamed {done} file(s)"))
        } else {
            RenameResult::Error(format!("Renamed {done} file(s), {} error(s):\n{}", errors.len(), errors.join("\n")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> FileEntry {
        FileEntry {
            name: name.into(),
            is_dir: false,
            is_parent: false,
            size: 0,
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
    fn placeholders() {
        let rules = Rules::default();
        let p = Path::new(r"c:\photos\trip\IMG_1234.JPG");
        let e = entry("IMG_1234.JPG");
        assert_eq!(new_name(p, &e, 0, &rules).unwrap(), "IMG_1234.JPG");
        let r = Rules { name_mask: "[P]_[C]".into(), counter_digits: 3, ..Rules::default() };
        assert_eq!(new_name(p, &e, 4, &r).unwrap(), "trip_005.JPG");
        let r = Rules { name_mask: "[N5-8]".into(), ext_mask: "[E]".into(), case: CaseMode::Lower, ..Rules::default() };
        assert_eq!(new_name(p, &e, 0, &r).unwrap(), "1234.jpg");
        let r = Rules { name_mask: "[N1,3]-[C10+5:2]".into(), ..Rules::default() };
        assert_eq!(new_name(p, &e, 2, &r).unwrap(), "IMG-20.JPG");
        let r = Rules { name_mask: "[[x]] [N]".into(), ..Rules::default() };
        assert_eq!(new_name(p, &e, 0, &r).unwrap(), "[x]] IMG_1234.JPG");
        assert_eq!(substr("abcdef", "-2-"), "ef");
        assert_eq!(substr("abcdef", "2"), "b");
        assert_eq!(substr("abcdef", "3-"), "cdef");
    }

    #[test]
    fn search_replace_and_case() {
        let p = Path::new(r"c:\x\My File name.txt");
        let e = entry("My File name.txt");
        let r = Rules { search: "file".into(), replace: "Doc".into(), ..Rules::default() };
        assert_eq!(new_name(p, &e, 0, &r).unwrap(), "My Doc name.txt");
        let r = Rules { search: r"(\w+) (\w+)".into(), replace: "$2_$1".into(), regex: true, ..Rules::default() };
        assert_eq!(new_name(p, &e, 0, &r).unwrap(), "File_My name.txt");
        let r = Rules { case: CaseMode::WordsUpper, ..Rules::default() };
        assert_eq!(new_name(p, &e, 0, &r).unwrap(), "My File Name.Txt");
        let r = Rules { search: "name".into(), replace: "a/b".into(), ..Rules::default() };
        assert!(new_name(p, &e, 0, &r).is_err());
    }

    #[test]
    fn swap_names_via_two_phase_rename() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        std::fs::write(&a, b"A").unwrap();
        std::fs::write(&b, b"B").unwrap();
        let tool = RenameTool::new(vec![(a.clone(), entry("a.txt")), (b.clone(), entry("b.txt"))]);
        let preview = vec![Ok("b.txt".to_string()), Ok("a.txt".to_string())];
        assert!(matches!(tool.execute(&preview), RenameResult::Done(_)));
        assert_eq!(std::fs::read(&a).unwrap(), b"B");
        assert_eq!(std::fs::read(&b).unwrap(), b"A");
    }
}
