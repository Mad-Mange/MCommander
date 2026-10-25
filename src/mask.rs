//! Wildcard masks (`*.txt *.doc | *.bak`) and TC-style rename masks.

/// Case-insensitive DOS-style wildcard match with `*` and `?`.
/// As in Windows, `*.*` matches every name and `name.*` also matches `name`.
pub fn wildcard(pattern: &str, name: &str) -> bool {
    if pattern == "*" || pattern == "*.*" {
        return true;
    }
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    if matches(&p, &n) {
        return true;
    }
    // "readme.*" should also match "readme" (no extension).
    if p.ends_with(&['.', '*']) && !n.contains(&'.') {
        return matches(&p[..p.len() - 2], &n);
    }
    false
}

fn matches(p: &[char], n: &[char]) -> bool {
    // Iterative glob matching with backtracking on the last '*'.
    let (mut pi, mut ni) = (0, 0);
    let (mut star, mut mark) = (None, 0);
    while ni < n.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == n[ni]) {
            pi += 1;
            ni += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ni;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ni = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// A set of include and exclude patterns, e.g. `*.rs;*.toml | target\`.
/// Patterns ending in a backslash only apply to directories.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MaskSet {
    include: Vec<String>,
    exclude: Vec<String>,
}

impl MaskSet {
    pub fn parse(spec: &str) -> MaskSet {
        let (inc, exc) = spec.split_once('|').unwrap_or((spec, ""));
        let split = |s: &str| -> Vec<String> {
            split_patterns(s).into_iter().filter(|p| !p.is_empty()).collect()
        };
        let mut include = split(inc);
        if include.is_empty() {
            include.push("*".to_string());
        }
        MaskSet { include, exclude: split(exc) }
    }

    /// Like [`MaskSet::parse`] but patterns without wildcards match anywhere
    /// in the name (quick filter / search behaviour).
    pub fn parse_loose(spec: &str) -> MaskSet {
        let mut m = MaskSet::parse(spec);
        for p in m.include.iter_mut() {
            if !p.contains('*') && !p.contains('?') && !p.ends_with('\\') {
                *p = format!("*{p}*");
            }
        }
        m
    }

    pub fn matches(&self, name: &str, is_dir: bool) -> bool {
        let hit = |pats: &[String]| {
            pats.iter().any(|p| match p.strip_suffix('\\') {
                Some(dir_pat) => is_dir && wildcard(dir_pat, name),
                None => wildcard(p, name),
            })
        };
        // Directory-only include patterns must not hide plain files and vice versa:
        // a file is included when any non-directory pattern matches.
        let included = if is_dir {
            hit(&self.include)
        } else {
            self.include.iter().filter(|p| !p.ends_with('\\')).any(|p| wildcard(p, name))
        };
        included && !hit(&self.exclude)
    }
}

/// Splits on spaces and semicolons while honouring double quotes.
fn split_patterns(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            ' ' | ';' if !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Applies a TC copy/move target mask (`*.*`, `*.bak`, `h?.txt`, `start_**`)
/// to a file name. Name and extension parts are processed separately.
pub fn apply_rename_mask(mask: &str, name: &str) -> String {
    if mask.is_empty() || mask == "*.*" || mask == "*" {
        return name.to_string();
    }
    let (mask_name, mask_ext) = match mask.rfind('.') {
        Some(i) => (&mask[..i], Some(&mask[i + 1..])),
        None => (mask, None),
    };
    let (src_name, src_ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i + 1..]),
        _ => (name, ""),
    };
    let mut out = apply_part(mask_name, src_name);
    match mask_ext {
        Some(m) => {
            let ext = apply_part(m, src_ext);
            if !ext.is_empty() {
                out.push('.');
                out.push_str(&ext);
            }
        }
        None => {}
    }
    out
}

fn apply_part(mask: &str, src: &str) -> String {
    let m: Vec<char> = mask.chars().collect();
    let s: Vec<char> = src.chars().collect();
    let mut out = String::new();
    let mut pos = 0usize;
    let mut i = 0usize;
    while i < m.len() {
        match m[i] {
            '*' if m.get(i + 1) == Some(&'*') => {
                // "start**": insert the whole source part.
                out.extend(&s);
                pos = s.len();
                i += 2;
                continue;
            }
            '*' if m.get(i + 1) == Some(&'?') => {
                // "*?": reset the position in the source name.
                pos = 0;
                i += 2;
                continue;
            }
            '*' => {
                if pos < s.len() {
                    out.extend(&s[pos..]);
                }
                pos = s.len();
            }
            '?' => {
                if let Some(c) = s.get(pos) {
                    out.push(*c);
                }
                pos += 1;
            }
            c => {
                out.push(c);
                pos += 1;
            }
        }
        i += 1;
    }
    out
}

/// True when the string contains wildcard characters.
pub fn has_wildcards(s: &str) -> bool {
    s.contains('*') || s.contains('?')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_basics() {
        assert!(wildcard("*.txt", "README.TXT"));
        assert!(!wildcard("*.txt", "readme.md"));
        assert!(wildcard("*.*", "Makefile"));
        assert!(wildcard("read??.md", "readme.md"));
        assert!(wildcard("Makefile.*", "Makefile"));
        assert!(wildcard("*a*b*", "xxaYYbzz"));
        assert!(!wildcard("a*b", "acbd"));
    }

    #[test]
    fn mask_sets_with_exclusions() {
        let m = MaskSet::parse("*.* | *.bak *.old");
        assert!(m.matches("x.txt", false));
        assert!(!m.matches("x.bak", false));
        let m = MaskSet::parse("*.rs;*.toml");
        assert!(m.matches("main.rs", false));
        assert!(m.matches("Cargo.toml", false));
        assert!(!m.matches("a.md", false));
        let m = MaskSet::parse("* | target\\");
        assert!(!m.matches("target", true));
        assert!(m.matches("target", false));
        assert!(m.matches("src", true));
        assert!(MaskSet::parse("").matches("anything.x", false));
    }

    #[test]
    fn loose_masks_match_substrings() {
        let m = MaskSet::parse_loose("mand");
        assert!(m.matches("MCommander", true));
        assert!(!m.matches("Alarm", true));
    }

    #[test]
    fn rename_masks_follow_tc_help_examples() {
        assert_eq!(apply_rename_mask("*.*", "test.txt"), "test.txt");
        assert_eq!(apply_rename_mask("h?.txt", "test.txt"), "he.txt");
        assert_eq!(apply_rename_mask("f*.txt", "test.txt"), "fest.txt");
        assert_eq!(apply_rename_mask("F*Rest.*", "test.ext"), "FestRest.ext");
        assert_eq!(apply_rename_mask("start_**_rest.*", "test.txt"), "start_test_rest.txt");
        assert_eq!(apply_rename_mask("start_*?do*_rest.*", "name.txt"), "start_dome_rest.txt");
        assert_eq!(apply_rename_mask("*.bak", "a.txt"), "a.bak");
        assert_eq!(apply_rename_mask("new.txt", "a.md"), "new.txt");
    }
}
