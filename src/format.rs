//! Display formatting that mirrors Total Commander's file list conventions.

use std::time::SystemTime;

pub const ATTR_READONLY: u32 = 0x1;
pub const ATTR_HIDDEN: u32 = 0x2;
pub const ATTR_SYSTEM: u32 = 0x4;
pub const ATTR_DIRECTORY: u32 = 0x10;
pub const ATTR_ARCHIVE: u32 = 0x20;
pub const ATTR_REPARSE: u32 = 0x400;
pub const ATTR_COMPRESSED: u32 = 0x800;

/// `136999212` -> `"136 999 212"`.
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Size in kilobytes as used in the status and free-space lines: `"133 788 k"`.
pub fn kilo(n: u64) -> String {
    format!("{} k", thousands(n / 1024))
}

/// Compact human size for dialogs: `"1.5 MB"`.
pub fn human(n: u64) -> String {
    const UNITS: [&str; 6] = ["bytes", "KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return format!("{n} bytes");
    }
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

pub fn datetime(t: SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Local> = t.into();
    dt.format("%Y-%m-%d %H:%M").to_string()
}

pub fn datetime_secs(t: SystemTime) -> String {
    let dt: chrono::DateTime<chrono::Local> = t.into();
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Attribute column text in TC order: read-only, archive, hidden, system.
pub fn attr_string(attrs: u32) -> String {
    let flag = |bit: u32, c: char| if attrs & bit != 0 { c } else { '-' };
    [
        flag(ATTR_READONLY, 'r'),
        flag(ATTR_ARCHIVE, 'a'),
        flag(ATTR_HIDDEN, 'h'),
        flag(ATTR_SYSTEM, 's'),
    ]
    .iter()
    .collect()
}

/// Splits `name.ext` at the last dot. Names starting with a dot and
/// directories have no extension.
pub fn split_ext(name: &str, is_dir: bool) -> (&str, &str) {
    if is_dir {
        return (name, "");
    }
    match name.rfind('.') {
        Some(0) | None => (name, ""),
        Some(i) => (&name[..i], &name[i + 1..]),
    }
}

/// Natural, case-insensitive comparison (digit runs compare numerically),
/// close to Explorer's `StrCmpLogicalW` ordering.
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let mut na = String::new();
                    while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                        na.push(c);
                        ai.next();
                    }
                    let mut nb = String::new();
                    while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                        nb.push(c);
                        bi.next();
                    }
                    let ta = na.trim_start_matches('0');
                    let tb = nb.trim_start_matches('0');
                    let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                    if ord != Ordering::Equal {
                        return ord;
                    }
                } else {
                    let la = ca.to_lowercase().next().unwrap_or(ca);
                    let lb = cb.to_lowercase().next().unwrap_or(cb);
                    if la != lb {
                        return la.cmp(&lb);
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn thousands_groups_like_tc() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1 000");
        assert_eq!(thousands(136_999_212), "136 999 212");
        assert_eq!(kilo(136_999_212), "133 788 k");
        assert_eq!(kilo(1_953_513_468 * 1024), "1 953 513 468 k");
    }

    #[test]
    fn attributes_in_tc_order() {
        assert_eq!(attr_string(0), "----");
        assert_eq!(attr_string(ATTR_HIDDEN), "--h-");
        assert_eq!(attr_string(ATTR_READONLY | ATTR_ARCHIVE), "ra--");
        assert_eq!(attr_string(ATTR_ARCHIVE | ATTR_SYSTEM), "-a-s");
    }

    #[test]
    fn extension_split() {
        assert_eq!(split_ext("Atlas.zip", false), ("Atlas", "zip"));
        assert_eq!(split_ext("a.tar.gz", false), ("a.tar", "gz"));
        assert_eq!(split_ext(".gitignore", false), (".gitignore", ""));
        assert_eq!(split_ext("Makefile", false), ("Makefile", ""));
        assert_eq!(split_ext("dir.v2", true), ("dir.v2", ""));
    }

    #[test]
    fn natural_order() {
        assert_eq!(natural_cmp("file2", "file10"), Ordering::Less);
        assert_eq!(natural_cmp("Maple", "maps_app"), Ordering::Less);
        assert_eq!(natural_cmp("maps_app", "MCommander"), Ordering::Less);
        assert_eq!(natural_cmp("abc", "ABC"), "abc".cmp("ABC"));
        assert_eq!(natural_cmp("a", "a1"), Ordering::Less);
    }
}
