//! Parameter placeholders for buttons and menu commands, as in Total
//! Commander: `%P%N` (file under the cursor with path), `%T` (target
//! directory), `%S` (selected names), `%L` (list file of the selection)...

use std::path::PathBuf;

/// What a panel shows, as far as the placeholders need it.
#[derive(Clone, Debug, Default)]
pub struct PanelInfo {
    /// Directory with a trailing backslash.
    pub dir: String,
    /// Name under the cursor; `None` on "..".
    pub cursor: Option<String>,
    /// Selected names, in list order.
    pub selected: Vec<String>,
}

impl PanelInfo {
    /// The selection, or the entry under the cursor when nothing is selected.
    fn targets(&self, keep_empty: bool) -> Vec<String> {
        if !self.selected.is_empty() || keep_empty {
            self.selected.clone()
        } else {
            self.cursor.iter().cloned().collect()
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Context {
    pub source: PanelInfo,
    pub target: PanelInfo,
    /// The active (source) panel is the left one; `%X` switches to left/right.
    pub source_is_left: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct Expanded {
    pub args: String,
    /// The parameters started with `?`: let the user edit them first.
    pub ask: bool,
    /// Temporary list file (`%L`, `%F`...), to delete when the program ends.
    pub list_file: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq)]
enum ListKind {
    Ansi,
    Utf8,
    Utf16,
}

/// Expands the placeholders of a parameter string.
pub fn expand(template: &str, ctx: &Context) -> Expanded {
    let mut t = template.trim_start();
    let ask = t.starts_with('?');
    if ask {
        t = t[1..].trim_start();
    }
    let mut st = State {
        ctx,
        no_quotes: t.contains("%Q"),
        keep_empty: t.contains("%Y"),
        skip_empty_list: t.contains("%y"),
        left_right: false,
        list_file: None,
    };
    let mut out = String::new();
    let mut word = String::new();
    let mut quoted = false;
    for c in t.chars() {
        if c == '"' {
            quoted = !quoted;
        }
        if c.is_whitespace() && !quoted {
            out.push_str(&st.word(&std::mem::take(&mut word)));
            out.push(c);
        } else {
            word.push(c);
        }
    }
    out.push_str(&st.word(&word));
    Expanded { args: out.trim().to_string(), ask, list_file: st.list_file }
}

struct State<'a> {
    ctx: &'a Context,
    no_quotes: bool,
    keep_empty: bool,
    skip_empty_list: bool,
    left_right: bool,
    list_file: Option<PathBuf>,
}

/// One expanded piece of a word.
enum Piece {
    /// A single value (a path or name): the word gets quotes if it has spaces.
    Single(String),
    /// Several names, already quoted one by one.
    Multi(String),
    Text(String),
}

impl State<'_> {
    fn panels(&self) -> (&PanelInfo, &PanelInfo) {
        let c = self.ctx;
        if self.left_right && !c.source_is_left { (&c.target, &c.source) } else { (&c.source, &c.target) }
    }

    /// Expands one whitespace-separated word, quoting it when a single
    /// value with spaces went into it (unless it has quotes of its own).
    fn word(&mut self, w: &str) -> String {
        if w.is_empty() {
            return String::new();
        }
        let chars: Vec<char> = w.chars().collect();
        let mut i = 0;
        let mut out = String::new();
        let mut single = false;
        while i < chars.len() {
            if chars[i] != '%' || i + 1 >= chars.len() {
                out.push(chars[i]);
                i += 1;
                continue;
            }
            let (piece, used) = self.placeholder(&chars[i + 1..]);
            match piece {
                Piece::Single(s) => {
                    single = true;
                    out.push_str(&s);
                }
                Piece::Multi(s) | Piece::Text(s) => out.push_str(&s),
            }
            i += 1 + used;
        }
        if single && !self.no_quotes && !w.contains('"') && out.contains(' ') {
            format!("\"{out}\"")
        } else {
            out
        }
    }

    /// Expands the placeholder after a `%`; returns it and the number of
    /// characters it used.
    fn placeholder(&mut self, s: &[char]) -> (Piece, usize) {
        let (src, trg) = self.panels();
        let (src, trg) = (src.clone(), trg.clone());
        let next = s.get(1).copied();
        match s[0] {
            '%' => (Piece::Text("%".into()), 1),
            '|' => match s[1..].iter().position(|&c| c == '|') {
                Some(end) => {
                    let name: String = s[1..1 + end].iter().collect();
                    (Piece::Text(std::env::var(&name).unwrap_or_default()), end + 2)
                }
                None => (Piece::Text("%|".into()), 1),
            },
            'X' => {
                self.left_right = true;
                (Piece::Text(String::new()), 1)
            }
            'x' => {
                self.left_right = false;
                (Piece::Text(String::new()), 1)
            }
            'Q' | 'q' | 'Y' | 'y' | 'Z' | 'z' => (Piece::Text(String::new()), 1),
            // %P%S / %T%R: every selected name with its path.
            'P' | 'p' if matches!(next, Some('%')) && matches!(s.get(2), Some('S' | 's')) => {
                let (names, used) = self.names(&src, &s[3..]);
                (Piece::Multi(join_quoted(names.iter().map(|n| format!("{}{n}", src.dir)))), 3 + used)
            }
            'T' | 't' if matches!(next, Some('%')) && matches!(s.get(2), Some('R' | 'r')) => {
                let (names, used) = self.names(&trg, &s[3..]);
                (Piece::Multi(join_quoted(names.iter().map(|n| format!("{}{n}", trg.dir)))), 3 + used)
            }
            'P' | 'p' => single(src.dir.clone(), &s[1..], 1),
            'T' | 't' => single(trg.dir.clone(), &s[1..], 1),
            'N' | 'n' | 'v' => single(src.cursor.clone().unwrap_or_default(), &s[1..], 1),
            'V' => single(format!("{}{}", src.dir, src.cursor.clone().unwrap_or_default()), &s[1..], 1),
            'M' | 'm' => single(trg.cursor.clone().unwrap_or_default(), &s[1..], 1),
            'O' | 'o' => single(crate::format::split_ext(src.cursor.as_deref().unwrap_or(""), false).0.to_string(), &s[1..], 1),
            'E' | 'e' => {
                let name = src.cursor.clone().unwrap_or_default();
                let ext = crate::format::split_ext(&name, false).1.trim_start_matches('.').to_string();
                single(ext, &s[1..], 1)
            }
            'S' | 's' => {
                let (names, used) = self.names(&src, &s[1..]);
                (Piece::Multi(join_quoted(names.into_iter())), 1 + used)
            }
            'R' | 'r' => {
                let (names, used) = self.names(&trg, &s[1..]);
                (Piece::Multi(join_quoted(names.into_iter())), 1 + used)
            }
            'C' | 'c' if next.is_some_and(|d| d.is_ascii_digit() && d != '0') => {
                let n = next.unwrap().to_digit(10).unwrap() as usize;
                (Piece::Single(self.compare_file(&src, &trg, n)), 2)
            }
            'B' | 'b' => {
                let (target, mut used) = if next == Some('T') { (true, 2) } else { (false, 1) };
                let level = s.get(used).and_then(|c| c.to_digit(10)).map(|d| {
                    used += 1;
                    d as usize
                });
                let dir = if target { &trg.dir } else { &src.dir };
                let parts: Vec<&str> = dir.trim_end_matches('\\').split('\\').filter(|p| !p.is_empty()).collect();
                let part = parts.len().checked_sub(1 + level.unwrap_or(0)).and_then(|i| parts.get(i)).copied().unwrap_or("");
                single(part.to_string(), &s[used..], used)
            }
            'U' | 'W' if matches!(next, Some('L' | 'F')) => {
                let kind = if s[0] == 'U' { ListKind::Utf8 } else { ListKind::Utf16 };
                (self.list(&src, next == Some('L'), kind), 2)
            }
            'L' | 'l' | 'D' => (self.list(&src, true, ListKind::Ansi), 1),
            'F' | 'f' | 'd' => (self.list(&src, false, ListKind::Ansi), 1),
            'A' | 'a' => {
                // Alias parameters (%A, %A1, %A2-4): not used here.
                let used = 1 + s[1..].iter().take_while(|c| c.is_ascii_digit() || matches!(c, '-' | '+')).count();
                (Piece::Text(String::new()), used)
            }
            other => (Piece::Text(format!("%{other}")), 1),
        }
    }

    /// Selected names for %S/%R with an optional count suffix:
    /// `%S3` first three, `%S+2` second, `%S+2-4`, `%S-1` last, `%S-3-1`.
    fn names(&self, p: &PanelInfo, rest: &[char]) -> (Vec<String>, usize) {
        let all = p.targets(self.keep_empty);
        let read = |from: usize| -> (Option<usize>, usize) {
            let n = rest[from..].iter().take_while(|c| c.is_ascii_digit()).count();
            (rest[from..from + n].iter().collect::<String>().parse().ok(), from + n)
        };
        match rest.first() {
            Some(c) if c.is_ascii_digit() => {
                let (n, used) = read(0);
                (all.into_iter().take(n.unwrap_or(0)).collect(), used)
            }
            Some(sign @ ('+' | '-')) if rest.get(1).is_some_and(|c| c.is_ascii_digit()) => {
                let (a, mut used) = read(1);
                let a = a.unwrap_or(1).max(1);
                let mut b = Some(a);
                if rest.get(used) == Some(&'-') {
                    let (m, u) = read(used + 1);
                    b = m;
                    used = u;
                }
                let len = all.len();
                // Convert to 0-based indexes from the start.
                let idx = |k: usize| if *sign == '+' { k.checked_sub(1) } else { len.checked_sub(k) };
                let (from, to) = match (idx(a), b.and_then(idx)) {
                    (Some(x), Some(y)) => (x.min(y), x.max(y)),
                    (Some(x), None) => if *sign == '+' { (x, len.saturating_sub(1)) } else { (x, len.saturating_sub(1)) },
                    _ => return (Vec::new(), used),
                };
                (all.into_iter().skip(from).take(to + 1 - from).collect(), used)
            }
            _ => (all, 0),
        }
    }

    /// %C1/%C2 like "Compare by content": the first and second file.
    fn compare_file(&self, src: &PanelInfo, trg: &PanelInfo, n: usize) -> String {
        let sel = src.targets(false);
        let path = |dir: &str, name: &str| format!("{dir}{name}");
        match n {
            1 => sel.first().map(|f| path(&src.dir, f)).unwrap_or_default(),
            2 => match sel.get(1) {
                Some(f) => path(&src.dir, f),
                None => trg.selected.first().or(trg.cursor.as_ref()).map(|f| path(&trg.dir, f)).unwrap_or_default(),
            },
            k => sel.get(k - 1).map(|f| path(&src.dir, f)).unwrap_or_default(),
        }
    }

    /// Writes the list file for %L/%F (one per command) and returns its name.
    fn list(&mut self, p: &PanelInfo, with_path: bool, kind: ListKind) -> Piece {
        let names = p.targets(self.keep_empty);
        if names.is_empty() && self.skip_empty_list {
            return Piece::Text(String::new());
        }
        if let Some(f) = &self.list_file {
            return Piece::Single(f.to_string_lossy().into_owned());
        }
        let text: String = names.iter().map(|n| if with_path { format!("{}{n}\r\n", p.dir) } else { format!("{n}\r\n") }).collect();
        let bytes = match kind {
            ListKind::Ansi => encoding_rs::WINDOWS_1252.encode(&text).0.into_owned(),
            ListKind::Utf8 => [&[0xEF, 0xBB, 0xBF][..], text.as_bytes()].concat(),
            ListKind::Utf16 => [0xFF, 0xFE].into_iter().chain(text.encode_utf16().flat_map(u16::to_le_bytes)).collect(),
        };
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let path = std::env::temp_dir().join(format!("CMD{:X}.tmp", (stamp as u64) & 0xFFFF_FFFF ^ std::process::id() as u64));
        if std::fs::write(&path, bytes).is_err() {
            return Piece::Text(String::new());
        }
        self.list_file = Some(path.clone());
        Piece::Single(path.to_string_lossy().into_owned())
    }
}

/// A single value (`len` placeholder characters long), with an optional
/// `:~start,length` substring after it in `rest`.
fn single(value: String, rest: &[char], len: usize) -> (Piece, usize) {
    if rest.first() != Some(&':') || rest.get(1) != Some(&'~') {
        return (Piece::Single(value), len);
    }
    let num = |from: usize| -> (Option<i64>, usize) {
        let mut end = from;
        if rest.get(end) == Some(&'-') {
            end += 1;
        }
        while rest.get(end).is_some_and(|c| c.is_ascii_digit()) {
            end += 1;
        }
        let text: String = rest[from..end].iter().collect();
        (text.parse().ok(), end)
    };
    let (start, mut end) = num(2);
    let Some(start) = start else { return (Piece::Single(value), len) };
    let mut field_len = None;
    if rest.get(end) == Some(&',') {
        let (l, e) = num(end + 1);
        field_len = l;
        end = e;
    }
    if rest.get(end) == Some(&'~') {
        // "%N:~2,5~2": a ~ ends the field so digits can follow.
        end += 1;
    }
    (Piece::Single(substring(&value, start, field_len)), len + end)
}


/// Names separated by spaces, each quoted if it contains one.
fn join_quoted(names: impl Iterator<Item = String>) -> String {
    names.map(|n| if n.contains(' ') { format!("\"{n}\"") } else { n }).collect::<Vec<_>>().join(" ")
}

/// TC's `:~start,length`: negative numbers count from the end.
fn substring(s: &str, start: i64, len: Option<i64>) -> String {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len() as i64;
    let from = if start < 0 { (n + start).max(0) } else { start.min(n) };
    let to = match len {
        None => n,
        Some(l) if l < 0 => (n + l).max(from),
        Some(l) => (from + l).min(n),
    };
    chars[from as usize..to as usize].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context {
        Context {
            source: PanelInfo {
                dir: r"C:\My Files\".into(),
                cursor: Some("read me.txt".into()),
                selected: vec!["a.txt".into(), "b c.txt".into(), "d.txt".into()],
            },
            target: PanelInfo { dir: r"D:\Backup\".into(), cursor: Some("old.txt".into()), selected: vec![] },
            source_is_left: false,
        }
    }

    fn x(t: &str) -> String {
        expand(t, &ctx()).args
    }

    #[test]
    fn single_values_and_quoting() {
        assert_eq!(x("%P%N"), r#""C:\My Files\read me.txt""#);
        assert_eq!(x("%Q%P%N"), r"C:\My Files\read me.txt");
        assert_eq!(x(r#""%P%N""#), r#""C:\My Files\read me.txt""#);
        assert_eq!(x("%T%M"), r"D:\Backup\old.txt");
        assert_eq!(x("/L=%P /R=%T"), r#""/L=C:\My Files\" /R=D:\Backup\"#);
        assert_eq!(x("%O.%E 100%%"), r#""read me.txt" 100%"#);
        assert_eq!(x("%N:~0,4 %N:~-3"), "read txt");
        assert_eq!(x("%P:~0,-1"), r#""C:\My Files""#);
        assert_eq!(x("%B %BT %B1"), r#""My Files" Backup C:"#);
    }

    #[test]
    fn selections() {
        assert_eq!(x("%S"), r#"a.txt "b c.txt" d.txt"#);
        assert_eq!(x("%P%S"), r#""C:\My Files\a.txt" "C:\My Files\b c.txt" "C:\My Files\d.txt""#);
        assert_eq!(x("%S2"), r#"a.txt "b c.txt""#);
        assert_eq!(x("%S+3"), "d.txt");
        assert_eq!(x("%S-1"), "d.txt");
        assert_eq!(x("%S+2-3"), r#""b c.txt" d.txt"#);
        // Nothing selected in the target: the file under its cursor.
        assert_eq!(x("%R"), "old.txt");
        assert_eq!(x("%Y%R"), "");
        assert_eq!(x("%C1 %C2"), r#""C:\My Files\a.txt" "C:\My Files\b c.txt""#);
    }

    #[test]
    fn left_right_and_prompt() {
        // The right panel is the source here; %X makes %P the left one.
        assert_eq!(x("%X%P %T %x%P"), r#"D:\Backup\ "C:\My Files\" "C:\My Files\""#);
        let e = expand("? -v %N", &ctx());
        assert!(e.ask);
        assert_eq!(e.args, r#"-v "read me.txt""#);
        assert_eq!(x("%|OS|"), std::env::var("OS").unwrap_or_default());
    }

    #[test]
    fn list_files() {
        let e = expand("-@%L", &ctx());
        let list = e.list_file.clone().expect("list file");
        assert_eq!(e.args.replace('"', ""), format!("-@{}", list.display()));
        let text = std::fs::read_to_string(&list).unwrap();
        assert_eq!(text, "C:\\My Files\\a.txt\r\nC:\\My Files\\b c.txt\r\nC:\\My Files\\d.txt\r\n");
        std::fs::remove_file(&list).unwrap();
        let e = expand("%WF", &ctx());
        let bytes = std::fs::read(e.list_file.as_ref().unwrap()).unwrap();
        assert_eq!(&bytes[..4], &[0xFF, 0xFE, b'a', 0]);
        std::fs::remove_file(e.list_file.unwrap()).unwrap();
        let mut none = ctx();
        none.source.selected.clear();
        none.source.cursor = None;
        assert_eq!(expand("x %y%L", &none), Expanded { args: "x".into(), ask: false, list_file: None });
    }
}
