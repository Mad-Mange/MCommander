//! Button bars: the bar under the main menu and the vertical bar between the
//! panels. They are stored in Total Commander's `.bar` format, so bars made
//! for TC (and sub-bars they open) work as they are.

use crate::commands::{self, Cmd};
use crate::ini::Ini;
use crate::paint_icons::Icon;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarKind {
    /// Under the main menu.
    Main,
    /// Between the panels.
    Vertical,
}

impl BarKind {
    pub fn index(self) -> usize {
        self as usize
    }

    /// File name of the bar in the settings folder.
    pub fn file_name(self) -> &'static str {
        match self {
            BarKind::Main => "default.bar",
            BarKind::Vertical => "vertical.bar",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            BarKind::Main => "button bar",
            BarKind::Vertical => "vertical button bar",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct BarButton {
    /// Internal command(s) (`cm_Copy`, `cm_SelectAll,cm_Copy`, `cd c:\dir`), a
    /// program with fixed parameters, or another .bar file (a sub-bar).
    pub cmd: String,
    /// Parameters with placeholders such as `%P%N` (see `params`).
    pub params: String,
    /// Start directory; for `cd` buttons the directory for the other panel.
    pub path: String,
    /// `file,index` (`wcmicons.dll,62`, `notepad.exe`); empty: from the command.
    pub icon: String,
    /// Tooltip (and label in drop-down menus); `|` starts a new line.
    pub tip: String,
    /// 0 = normal window, 1 = minimized, -1 = maximized.
    pub iconic: i32,
}

impl BarButton {
    pub fn command(cmd: &str) -> BarButton {
        BarButton { cmd: cmd.into(), ..Default::default() }
    }

    pub fn is_separator(&self) -> bool {
        self.cmd.trim().is_empty() && self.icon.trim().is_empty()
    }

    /// TC: a command of -2 without an icon breaks the bar into another row
    /// (a gap in the vertical bar).
    pub fn is_break(&self) -> bool {
        self.cmd.trim() == "-2" && self.icon.trim().is_empty()
    }

    /// Takes no room and does nothing (separator or break).
    pub fn is_spacer(&self) -> bool {
        self.is_separator() || self.is_break()
    }

    pub fn action(&self) -> Action {
        parse_action(&self.cmd)
    }

    /// Tooltip text: the configured one (`|` = new line, `||` = `|`) or a
    /// description of the command.
    pub fn tooltip(&self) -> String {
        let tip = self.tip.trim();
        if !tip.is_empty() {
            return tip.replace("||", "\u{1}").replace('|', "\n").replace('\u{1}', "|");
        }
        match self.action() {
            Action::Internal(steps) => steps
                .iter()
                .map(|s| match s {
                    Step::Cmd(c) => describe(c),
                    Step::Cd(p) => format!("Change directory: {p}"),
                    Step::Wait(ms) => format!("Wait {ms} ms"),
                    Step::Unknown(n) => format!("{n} (not available in MCommander)"),
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Action::SubBar(p) => p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
            Action::Program { .. } => self.cmd.trim().to_string(),
            Action::Nothing => String::new(),
        }
    }

    /// One-line label for drop-down menus.
    pub fn label(&self) -> String {
        self.tooltip().lines().next().unwrap_or_default().to_string()
    }
}

/// A command's description with its keyboard shortcut, for tooltips.
pub fn describe(cmd: &Cmd) -> String {
    let help = commands::tc_commands().iter().find(|c| &c.cmd == cmd).map(|c| c.help).unwrap_or("Command");
    match commands::shortcut_of(cmd) {
        Some(key) => format!("{help} ({key})"),
        None => help.to_string(),
    }
}

/// What pressing a button does.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Internal commands, run one after the other.
    Internal(Vec<Step>),
    /// Another bar, shown as a drop-down menu.
    SubBar(PathBuf),
    /// An external program; `args` are the fixed parameters written after it
    /// in the command field. `admin`: the command started with `*`.
    Program { program: String, args: String, admin: bool },
    Nothing,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Step {
    Cmd(Cmd),
    /// `cd path`: change the active panel's directory.
    Cd(String),
    /// `cm_wait 500`.
    Wait(u64),
    /// A command MCommander does not have.
    Unknown(String),
}

pub fn parse_action(cmd: &str) -> Action {
    let t = cmd.trim();
    if t.is_empty() || t == "-2" {
        return Action::Nothing;
    }
    let (first, rest) = t.split_once(char::is_whitespace).map_or((t, ""), |(a, b)| (a, b.trim()));
    if ["OPENBARMENU", "OPENBAR", "OPENBAR1", "OPENBAR2"].contains(&first) && !rest.is_empty() {
        return Action::SubBar(PathBuf::from(crate::vfs::expand_env(rest.trim_matches('"'))));
    }
    let unquoted = t.trim_matches('"');
    if unquoted.to_ascii_lowercase().ends_with(".bar") {
        return Action::SubBar(PathBuf::from(crate::vfs::expand_env(unquoted)));
    }
    let lower = first.to_ascii_lowercase();
    let internal = lower.starts_with("cm_") || lower.starts_with("em_") || lower == "cd" || first.chars().all(|c| c.is_ascii_digit());
    if internal {
        return Action::Internal(split_commands(t).iter().map(|s| parse_step(s)).collect());
    }
    if first.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) && first.len() > 4 && !rest.is_empty() {
        // TC's capitalised commands (FTPOPEN, OPENTABS, SELECTFILES...).
        return Action::Internal(vec![Step::Unknown(first.to_string())]);
    }
    let (admin, line) = match t.strip_prefix('*') {
        Some(r) => (true, r.trim_start_matches('*').trim()),
        None => (false, t),
    };
    let (program, args) = crate::app::split_program(line);
    Action::Program { program: crate::vfs::expand_env(&program), args: args.to_string(), admin }
}

/// Splits `cd "c:\a,b",cm_SelectAll` at the commas outside quotes.
fn split_commands(t: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in t.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

fn parse_step(s: &str) -> Step {
    let (name, arg) = s.split_once(char::is_whitespace).map_or((s, ""), |(a, b)| (a, b.trim()));
    let lower = name.to_ascii_lowercase();
    if lower == "cd" {
        return Step::Cd(crate::vfs::expand_env(arg.trim_matches('"')));
    }
    if lower == "cm_wait" {
        return Step::Wait(arg.parse().unwrap_or(0).min(10_000));
    }
    match commands::tc_command(name) {
        Some(c) => Step::Cmd(c.cmd.clone()),
        None => Step::Unknown(name.to_string()),
    }
}

/// Where a button's picture comes from.
#[derive(Clone, Debug, PartialEq)]
pub enum IconSource {
    Painted(Icon),
    /// An icon resource in a file (`index` as in ExtractIcon: negative
    /// numbers are resource ids).
    File(PathBuf, i32),
    /// The shell's icon for a file type (`@name.ext`).
    FileType(String),
}

pub fn icon_source(b: &BarButton) -> IconSource {
    let spec = crate::vfs::expand_env(b.icon.trim());
    let fallback = || match b.action() {
        Action::Internal(steps) => match steps.first() {
            Some(Step::Cmd(c)) => IconSource::Painted(cmd_icon(c).unwrap_or(Icon::Command)),
            Some(Step::Cd(_)) => IconSource::Painted(Icon::Folder),
            _ => IconSource::Painted(Icon::Command),
        },
        Action::SubBar(_) => IconSource::Painted(Icon::Menu),
        Action::Program { program, .. } => match resolve_file(&program) {
            Some(p) => IconSource::File(p, 0),
            None => IconSource::Painted(Icon::Command),
        },
        Action::Nothing => IconSource::Painted(Icon::Command),
    };
    if spec.is_empty() {
        return fallback();
    }
    if let Some(ext) = spec.strip_prefix('@') {
        return IconSource::FileType(ext.to_string());
    }
    let (file, index) = match spec.rsplit_once(',') {
        Some((f, i)) if i.trim().parse::<i32>().is_ok() => (f.trim(), i.trim().parse().unwrap()),
        _ => (spec.as_str(), 0),
    };
    let file = file.trim_matches('"');
    let base = Path::new(file).file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if base == "wcmicons.dll" {
        // TC's icon library: use our own drawing of the same picture.
        return match wcm_icon(index) {
            Some(icon) => IconSource::Painted(icon),
            None => fallback(),
        };
    }
    match resolve_file(file) {
        Some(p) => IconSource::File(p, index),
        None => fallback(),
    }
}

/// An existing file for a name that may rely on the search path.
fn resolve_file(name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(name);
    if p.is_file() {
        return Some(p);
    }
    if p.is_absolute() {
        return None;
    }
    crate::win::find_on_path(name, None)
}

/// MCommander's picture for an internal command.
pub fn cmd_icon(cmd: &Cmd) -> Option<Icon> {
    use Cmd::*;
    Some(match cmd {
        Reread => Icon::Refresh,
        Brief => Icon::Brief,
        Full => Icon::Full,
        Thumbnails => Icon::Thumbs,
        QuickView => Icon::QuickView,
        BranchView => Icon::Branch,
        InvertSelection => Icon::Invert,
        HistoryBack => Icon::Back,
        HistoryForward => Icon::Forward,
        Pack => Icon::Pack,
        Unpack | TestArchive => Icon::Unpack,
        Search => Icon::Search,
        MultiRename => Icon::MultiRename,
        SyncDirs | CompareDirs | CompareContent => Icon::Compare,
        CopyNames | CopyFullNames | CopyDetails => Icon::CopyNames,
        ToggleHidden => Icon::Hidden,
        ToggleTheme => Icon::Theme,
        OpenTerminal => Icon::Terminal,
        View => Icon::View,
        Edit | EditNew => Icon::Edit,
        Copy | CopySame | ClipCopy => Icon::Copy,
        Move | Rename | ClipCut => Icon::Move,
        MkDir => Icon::MkDir,
        Delete | DeleteNow => Icon::Delete,
        Hotlist | AddToHotlist | ConfigureHotlist => Icon::Hotlist,
        FtpConnect | FtpNew | FtpDisconnect | MapNetworkDrive | DisconnectNetworkDrive | NetworkDrives => Icon::Network,
        Parent | Root => Icon::ParentDir,
        OpenDesktop | OpenExplorer => Icon::Folder,
        CdTree => Icon::Tree,
        Options | ConfigureBar(_) | ImportTc | SaveSettings => Icon::Gear,
        _ => return None,
    })
}

/// Our drawing for an icon of Total Commander's wcmicons.dll, by index.
fn wcm_icon(index: i32) -> Option<Icon> {
    Some(match index {
        0 => Icon::Refresh,
        1 | 2 | 51 | 76..=78 => Icon::Tree,
        3 => Icon::Brief,
        4 => Icon::Full,
        11 => Icon::Invert,
        14 | 34 => Icon::Gear,
        15 => Icon::ParentDir,
        16 | 17 | 53 | 54 => Icon::Network,
        18 => Icon::Back,
        19 => Icon::Forward,
        22 => Icon::QuickView,
        26 => Icon::Thumbs,
        27 => Icon::View,
        28 => Icon::Edit,
        29 => Icon::MkDir,
        30 => Icon::Pack,
        31 | 60 => Icon::Unpack,
        32 | 63 => Icon::Move,
        35 | 48 | 49 => Icon::Compare,
        45 | 59 => Icon::CopyNames,
        46 => Icon::MultiRename,
        47 => Icon::Search,
        50 => Icon::Branch,
        62 => Icon::Copy,
        64 => Icon::Delete,
        79 => Icon::Hidden,
        _ => return None,
    })
}

/// The pictures offered for `wcmicons.dll`, each under its first TC number.
pub fn wcm_icons() -> Vec<(i32, Icon)> {
    let mut out: Vec<(i32, Icon)> = Vec::new();
    for i in 0..100 {
        if let Some(icon) = wcm_icon(i).filter(|icon| !out.iter().any(|(_, o)| o == icon)) {
            out.push((i, icon));
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct ButtonBar {
    pub buttons: Vec<BarButton>,
}

impl ButtonBar {
    pub fn from_ini(ini: &Ini) -> ButtonBar {
        let sec = "Buttonbar";
        let count = ini.get_i64(sec, "Buttoncount").unwrap_or(0).clamp(0, 2000) as usize;
        let get = |k: &str, i: usize| ini.get(sec, &format!("{k}{i}")).unwrap_or("").to_string();
        let buttons = (1..=count)
            .map(|i| BarButton {
                cmd: get("cmd", i),
                params: get("param", i),
                path: get("path", i),
                icon: get("button", i),
                tip: get("menu", i),
                iconic: get("iconic", i).trim().parse().unwrap_or(0),
            })
            .collect();
        ButtonBar { buttons }
    }

    pub fn to_ini(&self) -> Ini {
        let mut ini = Ini::default();
        let sec = "Buttonbar";
        ini.set(sec, "Buttoncount", self.buttons.len());
        for (n, b) in self.buttons.iter().enumerate() {
            let i = n + 1;
            ini.set(sec, &format!("button{i}"), &b.icon);
            for (k, v) in [("cmd", &b.cmd), ("param", &b.params), ("path", &b.path), ("menu", &b.tip)] {
                if !v.is_empty() {
                    ini.set(sec, &format!("{k}{i}"), v);
                }
            }
            if b.iconic != 0 {
                ini.set(sec, &format!("iconic{i}"), b.iconic);
            }
        }
        ini
    }

    pub fn load(path: &Path) -> std::io::Result<ButtonBar> {
        Ini::load(path).map(|ini| ButtonBar::from_ini(&ini))
    }

    /// Writes the bar as UTF-16 with a byte order mark, which Total Commander
    /// reads too.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = self.to_ini().to_string().replace('\n', "\r\n");
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        std::fs::write(path, bytes)
    }

    /// The bar under the menu when none has been configured.
    pub fn default_main() -> ButtonBar {
        let sep = BarButton::default;
        let c = BarButton::command;
        let buttons = vec![
            c("cm_RereadSource"),
            sep(),
            c("cm_SrcShort"),
            c("cm_SrcLong"),
            c("cm_SrcThumbs"),
            c("cm_SrcQuickView"),
            sep(),
            c("cm_DirBranch"),
            sep(),
            c("cm_ExchangeSelection"),
            sep(),
            c("cm_GoToPreviousDir"),
            c("cm_GoToNextDir"),
            sep(),
            c("cm_PackFiles"),
            c("cm_UnpackFiles"),
            sep(),
            c("cm_SearchFor"),
            c("cm_MultiRenameFiles"),
            c("cm_FileSync"),
            c("cm_CopyFullNamesToClip"),
            sep(),
            c("cm_SwitchHidSys"),
            c("cm_DirectoryHotlist"),
            c("cm_FtpConnect"),
            sep(),
            c("cm_ExecuteDOS"),
            BarButton { cmd: "notepad.exe".into(), icon: "notepad.exe".into(), tip: "Notepad".into(), ..Default::default() },
            sep(),
            c("cm_SwitchDarkMode"),
        ];
        ButtonBar { buttons }
    }

    /// The bar between the panels when none has been configured.
    pub fn default_vertical() -> ButtonBar {
        let c = BarButton::command;
        let buttons = vec![
            c("-2"),
            c("-2"),
            c("cm_List"),
            c("cm_Edit"),
            c("cm_Copy"),
            c("cm_RenMov"),
            BarButton::default(),
            c("cm_PackFiles"),
            c("cm_MkDir"),
            c("cm_Delete"),
        ];
        ButtonBar { buttons }
    }

    pub fn default_for(kind: BarKind) -> ButtonBar {
        match kind {
            BarKind::Main => ButtonBar::default_main(),
            BarKind::Vertical => ButtonBar::default_vertical(),
        }
    }

    /// Replaces `%COMMANDER_PATH%` (TC's program folder) in all fields, for
    /// bars imported from Total Commander.
    pub fn resolve_commander_path(&mut self, tc_dir: &str) {
        let tc_dir = tc_dir.trim_end_matches('\\');
        for b in &mut self.buttons {
            for f in [&mut b.cmd, &mut b.params, &mut b.path, &mut b.icon] {
                *f = replace_ci(f, "%COMMANDER_PATH%", tc_dir);
            }
        }
    }
}

fn replace_ci(s: &str, pat: &str, with: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let pat = pat.to_ascii_lowercase();
    let mut out = String::new();
    let mut from = 0;
    while let Some(i) = lower[from..].find(&pat) {
        out.push_str(&s[from..from + i]);
        out.push_str(with);
        from += i + pat.len();
    }
    out.push_str(&s[from..]);
    out
}

/// Total Commander's main and vertical bar files for a `wincmd.ini`.
pub fn tc_bar_files(tc_ini: &Path, ini: &Ini) -> [Option<PathBuf>; 2] {
    let tc_dir = tc_program_dir(ini);
    let ini_dir = tc_ini.parent().map(Path::to_path_buf);
    let find = |section: &str, default: &str| -> Option<PathBuf> {
        if let Some(v) = ini.get(section, "Buttonbar").filter(|v| !v.trim().is_empty()) {
            let v = replace_ci(v.trim(), "%COMMANDER_PATH%", tc_dir.as_deref().unwrap_or(""));
            let mut p = PathBuf::from(crate::vfs::expand_env(&v));
            if p.is_relative() {
                if let Some(d) = &tc_dir {
                    p = Path::new(d).join(p);
                }
            }
            return p.is_file().then_some(p);
        }
        // Without a setting TC uses its copy next to wincmd.ini, else the one in its folder.
        [ini_dir.clone(), tc_dir.as_ref().map(PathBuf::from)].into_iter().flatten().map(|d| d.join(default)).find(|p| p.is_file())
    };
    [find("Buttonbar", "default.bar"), find("ButtonbarVertical", "vertical.bar")]
}

/// Total Commander's program folder (`%COMMANDER_PATH%`).
pub fn tc_program_dir(ini: &Ini) -> Option<String> {
    std::env::var("COMMANDER_PATH")
        .ok()
        .or_else(|| ini.get("Configuration", "InstallDir").map(str::to_string))
        .filter(|d| !d.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TC_BAR: &str = "[Buttonbar]\r\nButtoncount=6\r\nbutton1=wcmicons.dll\r\ncmd1=cm_Rereadsource\r\nbutton2=\r\nbutton3=wcmicons.dll,62\r\ncmd3=cm_Copy\r\nbutton4=notepad.exe\r\ncmd4=notepad.exe\r\nparam4=%P%N\r\nmenu4=Notepad|edit||view\r\niconic4=1\r\nbutton5=\r\ncmd5=-2\r\nbutton6=%COMMANDER_PATH%\\tools.ico\r\ncmd6=cd \"c:\\a,b\",cm_SelectAll,cm_wait 200,cm_NoSuchThing\r\n";

    #[test]
    fn reads_total_commander_bars() {
        let bar = ButtonBar::from_ini(&Ini::parse(TC_BAR));
        assert_eq!(bar.buttons.len(), 6);
        assert_eq!(bar.buttons[0].action(), Action::Internal(vec![Step::Cmd(Cmd::Reread)]));
        assert!(bar.buttons[1].is_separator());
        assert_eq!(icon_source(&bar.buttons[2]), IconSource::Painted(Icon::Copy));
        assert_eq!(bar.buttons[3].params, "%P%N");
        assert_eq!(bar.buttons[3].iconic, 1);
        assert_eq!(bar.buttons[3].tooltip(), "Notepad\nedit|view");
        assert!(matches!(bar.buttons[3].action(), Action::Program { ref program, admin: false, .. } if program == "notepad.exe"));
        assert!(bar.buttons[4].is_break());
        assert_eq!(
            bar.buttons[5].action(),
            Action::Internal(vec![Step::Cd(r"c:\a,b".into()), Step::Cmd(Cmd::SelectAll), Step::Wait(200), Step::Unknown("cm_NoSuchThing".into())])
        );
    }

    #[test]
    fn round_trips_through_bar_file() {
        let mut bar = ButtonBar::from_ini(&Ini::parse(TC_BAR));
        bar.resolve_commander_path(r"C:\Program Files\totalcmd\");
        assert_eq!(bar.buttons[5].icon, r"C:\Program Files\totalcmd\tools.ico");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.bar");
        bar.save(&path).unwrap();
        assert_eq!(&std::fs::read(&path).unwrap()[..2], &[0xFF, 0xFE]);
        assert_eq!(ButtonBar::load(&path).unwrap(), bar);
        assert_eq!(ButtonBar::from_ini(&ButtonBar::default_main().to_ini()), ButtonBar::default_main());
    }

    #[test]
    fn parses_programs_and_sub_bars() {
        assert_eq!(
            parse_action(r#"*"C:\Tools\my app.exe" -x"#),
            Action::Program { program: r"C:\Tools\my app.exe".into(), args: "-x".into(), admin: true }
        );
        assert_eq!(parse_action(r"d:\bars\tools.bar"), Action::SubBar(PathBuf::from(r"d:\bars\tools.bar")));
        assert_eq!(parse_action("OPENBARMENU extra.bar"), Action::SubBar(PathBuf::from("extra.bar")));
        assert_eq!(parse_action("905"), Action::Internal(vec![Step::Cmd(Cmd::Copy)]));
        assert_eq!(parse_action("FTPOPEN mysite"), Action::Internal(vec![Step::Unknown("FTPOPEN".into())]));
        assert_eq!(parse_action(""), Action::Nothing);
    }

    #[test]
    fn default_bars_use_known_commands() {
        for kind in [BarKind::Main, BarKind::Vertical] {
            for b in ButtonBar::default_for(kind).buttons.iter().filter(|b| !b.is_spacer()) {
                if let Action::Internal(steps) = b.action() {
                    assert!(steps.iter().all(|s| matches!(s, Step::Cmd(_))), "{}", b.cmd);
                    assert!(matches!(icon_source(b), IconSource::Painted(i) if i != Icon::Command), "{}", b.cmd);
                }
            }
        }
        assert_eq!(describe(&Cmd::Copy), "Copy (F5)");
    }

    #[test]
    fn every_tc_command_name_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for c in commands::tc_commands() {
            assert!(seen.insert(c.name.to_ascii_lowercase()), "{}", c.name);
            assert!(c.name.starts_with("cm_"));
        }
        assert_eq!(commands::tc_command("CM_COPY").map(|c| c.id), Some(905));
    }
}
