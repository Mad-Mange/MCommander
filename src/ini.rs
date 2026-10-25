//! Minimal INI reader/writer.
//!
//! Section and key lookups are case-insensitive (like Windows' profile API) but
//! the original spelling and order are preserved on write. Reading detects
//! UTF-16 LE/BE and UTF-8 byte-order marks and falls back to Windows-1252 for
//! legacy ANSI files such as older `wincmd.ini` versions.

use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct Section {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default)]
pub struct Ini {
    pub sections: Vec<Section>,
}

impl Ini {
    pub fn parse(text: &str) -> Ini {
        let mut ini = Ini::default();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                ini.sections.push(Section {
                    name: line[1..line.len() - 1].trim().to_string(),
                    entries: Vec::new(),
                });
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            if ini.sections.is_empty() {
                ini.sections.push(Section::default());
            }
            let sec = ini.sections.last_mut().unwrap();
            sec.entries.push((k.trim().to_string(), v.trim().to_string()));
        }
        ini
    }

    pub fn load(path: &Path) -> std::io::Result<Ini> {
        let bytes = std::fs::read(path)?;
        Ok(Ini::parse(&decode_text(&bytes)))
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Write atomically so a crash mid-save never truncates the settings.
        let tmp = path.with_extension("ini.tmp");
        std::fs::write(&tmp, self.to_string())?;
        std::fs::rename(&tmp, path)
    }

    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name.eq_ignore_ascii_case(name))
    }

    pub fn section_mut(&mut self, name: &str) -> &mut Section {
        if let Some(i) = self.sections.iter().position(|s| s.name.eq_ignore_ascii_case(name)) {
            return &mut self.sections[i];
        }
        self.sections.push(Section { name: name.to_string(), entries: Vec::new() });
        self.sections.last_mut().unwrap()
    }

    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        self.section(section)?.get(key)
    }

    pub fn get_or<'a>(&'a self, section: &str, key: &str, default: &'a str) -> &'a str {
        self.get(section, key).unwrap_or(default)
    }

    pub fn get_i64(&self, section: &str, key: &str) -> Option<i64> {
        self.get(section, key)?.trim().parse().ok()
    }

    pub fn get_bool(&self, section: &str, key: &str, default: bool) -> bool {
        match self.get(section, key).map(str::trim) {
            Some("1") | Some("true") | Some("yes") => true,
            Some("0") | Some("false") | Some("no") => false,
            _ => default,
        }
    }

    pub fn set(&mut self, section: &str, key: &str, value: impl ToString) {
        self.section_mut(section).set(key, value.to_string());
    }
}

impl std::fmt::Display for Ini {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for s in &self.sections {
            if !s.name.is_empty() {
                writeln!(f, "[{}]", s.name)?;
            }
            for (k, v) in &s.entries {
                writeln!(f, "{k}={v}")?;
            }
        }
        Ok(())
    }
}

impl Section {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }

    pub fn set(&mut self, key: &str, value: String) {
        if let Some(e) = self.entries.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            e.1 = value;
        } else {
            self.entries.push((key.to_string(), value));
        }
    }
}

/// Decodes a text file of unknown encoding (BOM sniffing, then UTF-8, then ANSI).
pub fn decode_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return encoding_rs::UTF_16LE.decode_with_bom_removal(bytes).0.into_owned();
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return encoding_rs::UTF_16BE.decode_with_bom_removal(bytes).0.into_owned();
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::WINDOWS_1252.decode(bytes).0.into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_lookup_is_case_insensitive() {
        let ini = Ini::parse("[Configuration]\nAltSearch=2\n; comment\n[left]\npath=d:\\Work\\\n");
        assert_eq!(ini.get("configuration", "altsearch"), Some("2"));
        assert_eq!(ini.get("LEFT", "Path"), Some("d:\\Work\\"));
        assert_eq!(ini.get_i64("Configuration", "AltSearch"), Some(2));
        assert!(ini.get("missing", "x").is_none());
    }

    #[test]
    fn set_preserves_order_and_round_trips() {
        let mut ini = Ini::default();
        ini.set("A", "one", 1);
        ini.set("A", "two", "2");
        ini.set("a", "ONE", 11);
        let text = ini.to_string();
        assert_eq!(text, "[A]\none=11\ntwo=2\n");
        let back = Ini::parse(&text);
        assert_eq!(back.get("A", "one"), Some("11"));
    }

    #[test]
    fn decodes_utf16_and_ansi() {
        let mut utf16 = vec![0xFF, 0xFE];
        for u in "[S]\nk=v\u{e5}".encode_utf16() {
            utf16.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_text(&utf16), "[S]\nk=v\u{e5}");
        assert_eq!(decode_text(b"k=v\xe5"), "k=v\u{e5}");
        assert_eq!(decode_text("k=\u{e5}".as_bytes()), "k=\u{e5}");
    }
}
