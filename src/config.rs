//! Persistent settings (`%APPDATA%\MCommander\mcommander.ini`) and the
//! one-time import of an existing Total Commander configuration.

use crate::ini::Ini;
use crate::vfs::{SortColumn, SortSpec};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    Light,
    Dark,
    System,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuickSearchMode {
    CtrlAlt = 0,
    Alt = 1,
    Letters = 2,
    LettersDialog = 3,
    Disabled = 4,
}

impl QuickSearchMode {
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => Self::Alt,
            2 => Self::Letters,
            3 => Self::LettersDialog,
            4 => Self::Disabled,
            _ => Self::CtrlAlt,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::CtrlAlt => "Ctrl+Alt+Letters",
            Self::Alt => "Alt+Letters",
            Self::Letters => "Letter only",
            Self::LettersDialog => "Letters, with search dialog",
            Self::Disabled => "Disabled",
        }
    }

    pub const ALL: [QuickSearchMode; 5] =
        [Self::CtrlAlt, Self::Alt, Self::Letters, Self::LettersDialog, Self::Disabled];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Full,
    Brief,
    Thumbs,
}

#[derive(Clone, Debug)]
pub struct PanelConfig {
    pub tabs: Vec<String>,
    pub active_tab: usize,
    pub sort: SortSpec,
    pub view: ViewMode,
}

impl PanelConfig {
    fn new(path: String) -> Self {
        PanelConfig { tabs: vec![path], active_tab: 0, sort: SortSpec::default(), view: ViewMode::Full }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct HotlistEntry {
    pub name: String,
    /// Target directory; empty for a separator line.
    pub path: String,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub file: PathBuf,
    pub theme: ThemeMode,
    pub quick_search: QuickSearchMode,
    pub show_hidden: bool,
    pub dir_brackets: bool,
    pub use_recycle_bin: bool,
    pub confirm_delete: bool,
    pub space_calculates_dirs: bool,
    pub num_plus_selects_dirs: bool,
    pub editor: String,
    pub terminal: String,
    pub font_size: f32,
    pub show_toolbar: bool,
    pub show_drive_buttons: bool,
    pub show_middle_bar: bool,
    pub show_cmdline: bool,
    pub show_fkeys: bool,
    pub show_status: bool,
    /// Widths of the Ext, Size, Date and Attr columns; Name takes the rest.
    pub columns: [f32; 4],
    /// Left panel share of the width (0..1).
    pub divider: f32,
    pub window: Option<[f32; 4]>,
    pub maximized: bool,
    pub left: PanelConfig,
    pub right: PanelConfig,
    pub active_right: bool,
    pub hotlist: Vec<HotlistEntry>,
    pub cmd_history: Vec<String>,
    pub select_history: Vec<String>,
    pub search_history: Vec<String>,
    pub imported_from: Option<String>,
}

pub const MAX_HISTORY: usize = 30;

impl Default for Config {
    fn default() -> Self {
        let home = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into());
        Config {
            file: default_config_path(),
            theme: ThemeMode::System,
            quick_search: QuickSearchMode::CtrlAlt,
            show_hidden: false,
            dir_brackets: true,
            use_recycle_bin: true,
            confirm_delete: true,
            space_calculates_dirs: true,
            num_plus_selects_dirs: false,
            editor: "notepad.exe".into(),
            terminal: "cmd.exe".into(),
            font_size: 12.5,
            show_toolbar: true,
            show_drive_buttons: true,
            show_middle_bar: true,
            show_cmdline: true,
            show_fkeys: true,
            show_status: true,
            columns: [100.0, 82.0, 108.0, 40.0],
            divider: 0.5,
            window: None,
            maximized: false,
            left: PanelConfig::new(format!("{home}\\")),
            right: PanelConfig::new("C:\\".into()),
            active_right: false,
            hotlist: Vec::new(),
            cmd_history: Vec::new(),
            select_history: vec!["*.*".into()],
            search_history: Vec::new(),
            imported_from: None,
        }
    }
}

/// Size of the virtual desktop, as Total Commander keys its window positions.
fn screen_size() -> Option<(i64, i64)> {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN};
    let (w, h) = unsafe { (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)) };
    (w > 0 && h > 0).then_some((w as i64, h as i64))
}

pub fn default_config_path() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("MCommander").join("mcommander.ini")
}

/// Location of Total Commander's main ini file, if one exists.
pub fn find_tc_ini() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(p) = std::env::var_os("COMMANDER_INI") {
        candidates.push(PathBuf::from(p));
    }
    if let Some(a) = std::env::var_os("APPDATA") {
        candidates.push(PathBuf::from(a).join("GHISLER").join("wincmd.ini"));
    }
    for dir in [r"C:\Program Files\totalcmd", r"C:\totalcmd", r"C:\Program Files (x86)\totalcmd"] {
        candidates.push(Path::new(dir).join("wincmd.ini"));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn sort_to_ini(s: SortSpec) -> String {
    format!("{},{}", s.column.index(), s.reverse as i32)
}

fn sort_from_ini(v: &str) -> SortSpec {
    let mut it = v.split(',');
    let col = it.next().and_then(|c| c.trim().parse().ok()).unwrap_or(0);
    let rev = it.next().map(|r| r.trim() == "1").unwrap_or(false);
    SortSpec { column: SortColumn::from_index(col), reverse: rev }
}

impl Config {
    /// Loads the configuration from `file`, importing Total Commander
    /// settings when the file does not exist yet.
    pub fn load(file: PathBuf) -> Config {
        if let Ok(ini) = Ini::load(&file) {
            let mut c = Config::from_ini(&ini);
            c.file = file;
            return c;
        }
        let mut c = Config { file, ..Config::default() };
        if let Some(tc) = find_tc_ini() {
            if let Ok(ini) = Ini::load(&tc) {
                c.import_tc(&ini);
                c.imported_from = Some(tc.to_string_lossy().into_owned());
            }
        }
        c
    }

    fn from_ini(ini: &Ini) -> Config {
        let d = Config::default();
        let b = |k: &str, def: bool| ini.get_bool("Configuration", k, def);
        let s = |k: &str, def: &str| ini.get_or("Configuration", k, def).to_string();
        let theme = match ini.get_or("Configuration", "Theme", "system") {
            "light" => ThemeMode::Light,
            "dark" => ThemeMode::Dark,
            _ => ThemeMode::System,
        };
        let panel = |sec: &str, def: &PanelConfig| -> PanelConfig {
            let mut tabs = Vec::new();
            for i in 0..64 {
                match ini.get(sec, &format!("tab{i}")) {
                    Some(p) => tabs.push(p.to_string()),
                    None => break,
                }
            }
            if tabs.is_empty() {
                tabs = def.tabs.clone();
            }
            let active_tab = (ini.get_i64(sec, "activetab").unwrap_or(0).max(0) as usize).min(tabs.len() - 1);
            PanelConfig {
                tabs,
                active_tab,
                sort: ini.get(sec, "sort").map(sort_from_ini).unwrap_or_default(),
                view: match ini.get_or(sec, "view", "full") {
                    "brief" => ViewMode::Brief,
                    "thumbs" => ViewMode::Thumbs,
                    _ => ViewMode::Full,
                },
            }
        };
        let list = |sec: &str| -> Vec<String> {
            ini.section(sec).map(|s| s.entries.iter().map(|(_, v)| v.clone()).collect()).unwrap_or_default()
        };
        let mut hotlist = Vec::new();
        if let Some(sec) = ini.section("Hotlist") {
            for i in 1..1000 {
                let (Some(n), Some(p)) = (sec.get(&format!("name{i}")), sec.get(&format!("path{i}"))) else { break };
                hotlist.push(HotlistEntry { name: n.to_string(), path: p.to_string() });
            }
        }
        let columns = ini
            .get("Layout", "columns")
            .map(|v| {
                let mut c = d.columns;
                for (i, w) in v.split(',').take(4).enumerate() {
                    if let Ok(w) = w.trim().parse::<f32>() {
                        c[i] = w.clamp(20.0, 600.0);
                    }
                }
                c
            })
            .unwrap_or(d.columns);
        let window = ini.get("Layout", "window").and_then(|v| {
            let n: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            (n.len() == 4 && n[2] > 200.0 && n[3] > 150.0).then(|| [n[0], n[1], n[2], n[3]])
        });
        Config {
            file: d.file.clone(),
            theme,
            quick_search: QuickSearchMode::from_i64(ini.get_i64("Configuration", "QuickSearch").unwrap_or(0)),
            show_hidden: b("ShowHidden", d.show_hidden),
            dir_brackets: b("DirBrackets", d.dir_brackets),
            use_recycle_bin: b("UseRecycleBin", d.use_recycle_bin),
            confirm_delete: b("ConfirmDelete", d.confirm_delete),
            space_calculates_dirs: b("SpaceCalculatesDirs", d.space_calculates_dirs),
            num_plus_selects_dirs: b("NumPlusSelectsDirs", d.num_plus_selects_dirs),
            editor: s("Editor", &d.editor),
            terminal: s("Terminal", &d.terminal),
            font_size: ini
                .get("Configuration", "FontSize")
                .and_then(|v| v.parse().ok())
                .unwrap_or(d.font_size)
                .clamp(9.0, 24.0),
            show_toolbar: b("ShowToolbar", true),
            show_drive_buttons: b("ShowDriveButtons", true),
            show_middle_bar: b("ShowMiddleBar", true),
            show_cmdline: b("ShowCmdLine", true),
            show_fkeys: b("ShowFKeys", true),
            show_status: b("ShowStatus", true),
            columns,
            divider: ini
                .get("Layout", "divider")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.5f32)
                .clamp(0.1, 0.9),
            window,
            maximized: ini.get_bool("Layout", "maximized", false),
            left: panel("Left", &d.left),
            right: panel("Right", &d.right),
            active_right: ini.get_bool("Layout", "activeright", false),
            hotlist,
            cmd_history: list("CmdHistory"),
            select_history: {
                let l = list("SelectHistory");
                if l.is_empty() { d.select_history.clone() } else { l }
            },
            search_history: list("SearchHistory"),
            imported_from: ini.get("Configuration", "ImportedFrom").map(str::to_string),
        }
    }

    pub fn to_ini(&self) -> Ini {
        let mut ini = Ini::default();
        let c = "Configuration";
        ini.set(c, "Theme", match self.theme {
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
            ThemeMode::System => "system",
        });
        ini.set(c, "QuickSearch", self.quick_search as i32);
        let flags = [
            ("ShowHidden", self.show_hidden),
            ("DirBrackets", self.dir_brackets),
            ("UseRecycleBin", self.use_recycle_bin),
            ("ConfirmDelete", self.confirm_delete),
            ("SpaceCalculatesDirs", self.space_calculates_dirs),
            ("NumPlusSelectsDirs", self.num_plus_selects_dirs),
            ("ShowToolbar", self.show_toolbar),
            ("ShowDriveButtons", self.show_drive_buttons),
            ("ShowMiddleBar", self.show_middle_bar),
            ("ShowCmdLine", self.show_cmdline),
            ("ShowFKeys", self.show_fkeys),
            ("ShowStatus", self.show_status),
        ];
        for (k, v) in flags {
            ini.set(c, k, v as i32);
        }
        ini.set(c, "Editor", &self.editor);
        ini.set(c, "Terminal", &self.terminal);
        ini.set(c, "FontSize", self.font_size);
        if let Some(src) = &self.imported_from {
            ini.set(c, "ImportedFrom", src);
        }
        let l = "Layout";
        ini.set(l, "columns", self.columns.iter().map(|w| format!("{w:.0}")).collect::<Vec<_>>().join(","));
        ini.set(l, "divider", format!("{:.4}", self.divider));
        if let Some(w) = self.window {
            ini.set(l, "window", format!("{:.0},{:.0},{:.0},{:.0}", w[0], w[1], w[2], w[3]));
        }
        ini.set(l, "maximized", self.maximized as i32);
        ini.set(l, "activeright", self.active_right as i32);
        for (sec, p) in [("Left", &self.left), ("Right", &self.right)] {
            for (i, t) in p.tabs.iter().enumerate() {
                ini.set(sec, &format!("tab{i}"), t);
            }
            ini.set(sec, "activetab", p.active_tab);
            ini.set(sec, "sort", sort_to_ini(p.sort));
            ini.set(sec, "view", match p.view {
                ViewMode::Brief => "brief",
                ViewMode::Thumbs => "thumbs",
                ViewMode::Full => "full",
            });
        }
        for (i, h) in self.hotlist.iter().enumerate() {
            ini.set("Hotlist", &format!("name{}", i + 1), &h.name);
            ini.set("Hotlist", &format!("path{}", i + 1), &h.path);
        }
        for (sec, list) in [
            ("CmdHistory", &self.cmd_history),
            ("SelectHistory", &self.select_history),
            ("SearchHistory", &self.search_history),
        ] {
            for (i, v) in list.iter().take(MAX_HISTORY).enumerate() {
                ini.set(sec, &i.to_string(), v);
            }
        }
        ini
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.to_ini().save(&self.file)
    }

    /// Imports the parts of a `wincmd.ini` that map onto MCommander settings.
    pub fn import_tc(&mut self, ini: &Ini) {
        self.import_tc_window(ini, screen_size());
        let cfg = "Configuration";
        if let Some(v) = ini.get_i64(cfg, "AltSearch") {
            self.quick_search = QuickSearchMode::from_i64(v);
        }
        if let Some(v) = ini.get_i64(cfg, "DarkMode") {
            self.theme = match v {
                0 => ThemeMode::Light,
                2 => ThemeMode::Dark,
                _ => ThemeMode::System,
            };
        }
        if let Some(v) = ini.get_i64(cfg, "ShowHiddenSystem") {
            self.show_hidden = v != 0;
        }
        if let Some(v) = ini.get_i64(cfg, "DirBrackets") {
            self.dir_brackets = v & 1 == 1;
        }
        for (sec, tabs_sec, target) in [("left", "lefttabs", &mut self.left), ("right", "righttabs", &mut self.right)] {
            let mut tabs = Vec::new();
            for i in 0..64 {
                match ini.get(tabs_sec, &format!("{i}_path")) {
                    Some(p) => tabs.push(p.to_string()),
                    None => break,
                }
            }
            let active_path = ini.get(sec, "path").map(str::to_string);
            let active = ini.get_i64(tabs_sec, "activetab").unwrap_or(0).max(0) as usize;
            if let Some(p) = active_path {
                // The active tab's path lives in [left]/[right], not in the tabs section.
                if tabs.is_empty() {
                    tabs.push(p);
                } else if active < tabs.len() {
                    tabs[active] = p;
                } else {
                    tabs.push(p);
                }
            }
            if !tabs.is_empty() {
                target.active_tab = active.min(tabs.len() - 1);
                target.tabs = tabs;
            }
            let col = match ini.get_i64(sec, "sortorder").unwrap_or(0) {
                1 => SortColumn::Ext,
                2 => SortColumn::Size,
                3 => SortColumn::Date,
                4 => SortColumn::Unsorted,
                _ => SortColumn::Name,
            };
            target.sort = SortSpec { column: col, reverse: ini.get_bool(sec, "negative Sortorder", false) };
        }
        if let Some(menu) = ini.section("DirMenu") {
            let mut list = Vec::new();
            for i in 1..1000 {
                let Some(name) = menu.get(&format!("menu{i}")) else { break };
                let cmd = menu.get(&format!("cmd{i}")).unwrap_or("");
                if name == "-" {
                    list.push(HotlistEntry { name: "-".into(), path: String::new() });
                } else if let Some(path) = cmd.strip_prefix("cd ").or_else(|| cmd.strip_prefix("CD ")) {
                    list.push(HotlistEntry { name: name.to_string(), path: path.trim().to_string() });
                }
            }
            if !list.is_empty() {
                self.hotlist = list;
            }
        }
        if let Some(h) = ini.section("Command line history") {
            self.cmd_history = h.entries.iter().map(|(_, v)| v.clone()).take(MAX_HISTORY).collect();
        }
        if let Some(h) = ini.section("SearchName") {
            let list: Vec<String> = h.entries.iter().map(|(_, v)| v.clone()).filter(|v| !v.is_empty()).collect();
            if !list.is_empty() {
                self.search_history = list;
            }
        }
    }

    /// TC stores the window position per desktop resolution in sections named
    /// like `[5120x1440 (8x16)]`; prefer the one matching the current screen.
    fn import_tc_window(&mut self, ini: &Ini, screen: Option<(i64, i64)>) {
        let parse = |name: &str| -> Option<(i64, i64)> {
            let res = name.split_whitespace().next()?;
            let (w, h) = res.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        };
        let candidates: Vec<_> = ini
            .sections
            .iter()
            .filter(|s| parse(&s.name).is_some() && s.get("dx").is_some() && s.get("dy").is_some())
            .collect();
        let Some(sec) = candidates
            .iter()
            .find(|s| screen.is_some() && parse(&s.name) == screen)
            .or(candidates.first())
        else {
            return;
        };
        let num = |k: &str| sec.get(k).and_then(|v| v.trim().parse::<f32>().ok());
        if let (Some(x), Some(y), Some(w), Some(h)) = (num("x"), num("y"), num("dx"), num("dy")) {
            if w > 200.0 && h > 150.0 {
                self.window = Some([x, y, w, h]);
            }
        }
        self.maximized = sec.get("maximized").is_some_and(|v| v.trim() == "1");
    }

    pub fn push_history(list: &mut Vec<String>, value: &str) {
        if value.trim().is_empty() {
            return;
        }
        list.retain(|v| v != value);
        list.insert(0, value.to_string());
        list.truncate(MAX_HISTORY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TC_INI: &str = r"[Configuration]
AltSearch=2
DarkMode=0
ShowHiddenSystem=2
DirBrackets=1
[DirMenu]
menu1=Downloads
cmd1=cd c:\Users\me\Downloads
menu2=-
menu3=Games
cmd3=cd D:\Games
[left]
path=d:\Work\Website\
sortorder=3
negative Sortorder=1
[right]
path=c:\Users\me\Downloads\
[Command line history]
0=code .
1=cmd
";

    #[test]
    fn imports_total_commander_settings() {
        let mut c = Config::default();
        c.import_tc(&Ini::parse(TC_INI));
        assert_eq!(c.quick_search, QuickSearchMode::Letters);
        assert_eq!(c.theme, ThemeMode::Light);
        assert!(c.show_hidden);
        assert_eq!(c.left.tabs, vec![r"d:\Work\Website\".to_string()]);
        assert_eq!(c.left.sort, SortSpec { column: SortColumn::Date, reverse: true });
        assert_eq!(c.right.tabs[0], r"c:\Users\me\Downloads\");
        assert_eq!(c.hotlist.len(), 3);
        assert_eq!(c.hotlist[0], HotlistEntry { name: "Downloads".into(), path: r"c:\Users\me\Downloads".into() });
        assert_eq!(c.hotlist[1].path, "");
        assert_eq!(c.cmd_history, vec!["code .".to_string(), "cmd".to_string()]);
    }

    #[test]
    fn imports_window_position_for_matching_resolution() {
        let ini = Ini::parse(
            "[1920x1080 (8x16)]\nx=10\ny=20\ndx=900\ndy=700\nmaximized=1\n[5120x1440 (8x16)]\nx=2010\ny=66\ndx=1100\ndy=1201\nmaximized=0\n",
        );
        let mut c = Config::default();
        c.import_tc_window(&ini, Some((5120, 1440)));
        assert_eq!(c.window, Some([2010.0, 66.0, 1100.0, 1201.0]));
        assert!(!c.maximized);
        c.import_tc_window(&ini, Some((800, 600)));
        assert_eq!(c.window, Some([10.0, 20.0, 900.0, 700.0]));
        assert!(c.maximized);
    }

    #[test]
    fn config_round_trips_through_ini() {
        let mut c = Config::default();
        c.theme = ThemeMode::Dark;
        c.quick_search = QuickSearchMode::Alt;
        c.left.tabs = vec!["c:\\".into(), "d:\\x\\".into()];
        c.left.active_tab = 1;
        c.left.sort = SortSpec { column: SortColumn::Size, reverse: true };
        c.right.view = ViewMode::Brief;
        c.hotlist.push(HotlistEntry { name: "Src".into(), path: "d:\\src".into() });
        c.cmd_history = vec!["code .".into()];
        c.columns = [90.0, 70.0, 120.0, 44.0];
        c.window = Some([10.0, 20.0, 1100.0, 900.0]);
        let back = Config::from_ini(&Ini::parse(&c.to_ini().to_string()));
        assert_eq!(back.theme, ThemeMode::Dark);
        assert_eq!(back.quick_search, QuickSearchMode::Alt);
        assert_eq!(back.left.tabs, c.left.tabs);
        assert_eq!(back.left.active_tab, 1);
        assert_eq!(back.left.sort, c.left.sort);
        assert_eq!(back.right.view, ViewMode::Brief);
        assert_eq!(back.hotlist, c.hotlist);
        assert_eq!(back.cmd_history, c.cmd_history);
        assert_eq!(back.columns, c.columns);
        assert_eq!(back.window, c.window);
    }

    #[test]
    fn history_dedupes_and_caps() {
        let mut h = vec!["a".to_string(), "b".to_string()];
        Config::push_history(&mut h, "b");
        assert_eq!(h, vec!["b", "a"]);
        for i in 0..50 {
            Config::push_history(&mut h, &i.to_string());
        }
        assert_eq!(h.len(), MAX_HISTORY);
    }
}
