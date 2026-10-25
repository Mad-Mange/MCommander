//! Internal commands (the equivalent of Total Commander's `cm_*` commands),
//! their default keyboard bindings and the main menu layout.

use crate::panel::Side;
use crate::vfs::SortColumn;
use egui::{Key, Modifiers};

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    // Cursor
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
    End,
    Left,
    Right,
    SelectUp,
    SelectDown,
    // Panels and navigation
    SwitchPanel,
    Open,
    OpenShell,
    Parent,
    Root,
    Reread,
    OpenInSide(Side),
    TargetEqualsSource,
    Exchange,
    HistoryBack,
    HistoryForward,
    HistoryMenu,
    DriveMenu(Side),
    GotoDrive(Side, char),
    GotoPath(String),
    NetworkDrives,
    MapNetworkDrive,
    DisconnectNetworkDrive,
    // View
    Brief,
    Full,
    SortBy(SortColumn),
    ReverseSort,
    ShowAll,
    ShowPrograms,
    ShowCustom,
    ToggleHidden,
    QuickView,
    QuickFilter,
    BranchView,
    NewTab,
    CloseTab,
    CloseAllTabs,
    NextTab,
    PrevTab,
    OpenDirInNewTab,
    // Selection
    Select,
    SelectSpace,
    SelectGroup,
    UnselectGroup,
    SelectAll,
    UnselectAll,
    InvertSelection,
    SelectSameExt,
    UnselectSameExt,
    SaveSelection,
    RestoreSelection,
    CopyNames,
    CopyFullNames,
    CopyDetails,
    CompareDirs,
    SyncDirs,
    CdTree,
    // File operations
    View,
    Edit,
    EditNew,
    Copy,
    CopySame,
    Move,
    Rename,
    MkDir,
    Delete,
    DeleteNow,
    Pack,
    Unpack,
    TestArchive,
    Properties,
    Attributes,
    OccupiedSpace,
    AllDirSizes,
    MultiRename,
    Search,
    CompareContent,
    Checksum,
    Hotlist,
    AddToHotlist,
    ConfigureHotlist,
    ClipCopy,
    ClipCut,
    ClipPaste,
    ContextMenu,
    OpenTerminal,
    OpenExplorer,
    OpenDesktop,
    // Command line
    CmdHistory,
    CmdAddName,
    CmdAddPath,
    CmdCopyPath,
    FocusCmdLine,
    // Application
    Options,
    ToggleTheme,
    ImportTc,
    SaveSettings,
    About,
    KeyboardHelp,
    Exit,
    /// Run an external command line in the active directory.
    Run(String),
    /// Open main menu number n (0 = Files ... 7 = Help) as a keyboard menu.
    MenuOpen(usize),
}

/// Alt+letter accelerators of the main menu titles (TC: &Files, &Mark,
/// &Commands, &Net, Sho&w, C&onfiguration, &Start, &Help).
pub fn menu_accelerator(c: char) -> Option<usize> {
    Some(match c.to_ascii_lowercase() {
        'f' => 0,
        'm' => 1,
        'c' => 2,
        'n' => 3,
        'w' => 4,
        'o' => 5,
        's' => 6,
        'h' => 7,
        _ => return None,
    })
}

pub const MENU_COUNT: usize = 8;

/// Context needed to resolve keys whose meaning depends on the command line.
#[derive(Clone, Copy, Debug)]
pub struct KeyCtx {
    pub cmdline_empty: bool,
    pub brief: bool,
}

/// Maps a key press to a command (file panel focused, no dialog open).
pub fn map_key(key: Key, m: Modifiers, kc: KeyCtx) -> Option<Cmd> {
    use Cmd::*;
    let none = !m.ctrl && !m.alt && !m.shift;
    let shift = m.shift && !m.ctrl && !m.alt;
    let ctrl = m.ctrl && !m.alt && !m.shift;
    let ctrl_shift = m.ctrl && m.shift && !m.alt;
    let alt = m.alt && !m.ctrl && !m.shift;
    let alt_shift = m.alt && m.shift && !m.ctrl;
    let empty = kc.cmdline_empty;
    Some(match key {
        Key::ArrowUp if none => Up,
        Key::ArrowDown if none => Down,
        Key::ArrowUp if shift => SelectUp,
        Key::ArrowDown if shift => SelectDown,
        Key::PageUp if none => PageUp,
        Key::PageDown if none => PageDown,
        Key::Home if none && empty => Home,
        Key::End if none && empty => End,
        Key::ArrowLeft if none && empty && kc.brief => Left,
        Key::ArrowRight if none && empty && kc.brief => Right,
        Key::ArrowRight if none && empty => FocusCmdLine,
        Key::Tab if none => SwitchPanel,
        Key::Tab if ctrl => NextTab,
        Key::Tab if ctrl_shift => PrevTab,
        Key::Enter if none && empty => Open,
        Key::Enter if shift && empty => OpenShell,
        Key::Enter if ctrl => CmdAddName,
        Key::Enter if ctrl_shift => CmdAddPath,
        Key::Enter if alt => Properties,
        Key::Enter if alt_shift => AllDirSizes,
        Key::Backspace if none && empty => Parent,
        Key::PageUp if ctrl => Parent,
        Key::PageDown if ctrl => Open,
        Key::Backslash if ctrl => Root,
        Key::ArrowLeft if ctrl => OpenInSide(Side::Left),
        Key::ArrowRight if ctrl => OpenInSide(Side::Right),
        Key::ArrowUp if ctrl => OpenDirInNewTab,
        Key::ArrowDown if ctrl => CmdHistory,
        Key::ArrowLeft if alt => HistoryBack,
        Key::ArrowRight if alt => HistoryForward,
        Key::ArrowDown if alt => HistoryMenu,
        Key::Insert if none => Select,
        Key::Space if none && empty => SelectSpace,
        Key::Delete if none && empty => Delete,
        Key::Delete if shift && empty => DeleteNow,
        Key::F1 if none => KeyboardHelp,
        Key::F2 if none => Reread,
        Key::F3 if none => View,
        Key::F4 if none => Edit,
        Key::F5 if none => Copy,
        Key::F6 if none => Move,
        Key::F7 if none => MkDir,
        Key::F8 if none => Delete,
        Key::F2 if shift => CompareDirs,
        Key::F4 if shift => EditNew,
        Key::F5 if shift => CopySame,
        Key::F6 if shift => Rename,
        Key::F8 if shift => DeleteNow,
        Key::F10 if shift => ContextMenu,
        Key::F1 if alt => DriveMenu(Side::Left),
        Key::F2 if alt => DriveMenu(Side::Right),
        Key::F5 if alt => Pack,
        Key::F6 | Key::F9 if alt => Unpack,
        Key::F9 if alt_shift => TestArchive,
        Key::F7 if alt => Search,
        Key::F8 if alt => CmdHistory,
        Key::F10 if alt => CdTree,
        Key::F9 | Key::F10 if none => MenuOpen(0),
        Key::F1 if ctrl => Brief,
        Key::F2 if ctrl => Full,
        Key::F3 if ctrl => SortBy(SortColumn::Name),
        Key::F4 if ctrl => SortBy(SortColumn::Ext),
        Key::F5 if ctrl => SortBy(SortColumn::Date),
        Key::F6 if ctrl => SortBy(SortColumn::Size),
        Key::F7 if ctrl => SortBy(SortColumn::Unsorted),
        Key::F10 if ctrl => ShowAll,
        Key::F11 if ctrl => ShowPrograms,
        Key::F12 if ctrl => ShowCustom,
        Key::A if ctrl => SelectAll,
        Key::B if ctrl => BranchView,
        Key::D if ctrl => Hotlist,
        Key::H if ctrl => ToggleHidden,
        Key::L if ctrl => OccupiedSpace,
        Key::M if ctrl => MultiRename,
        Key::P if ctrl => CmdCopyPath,
        Key::Q if ctrl => QuickView,
        Key::R if ctrl => Reread,
        Key::S if ctrl => QuickFilter,
        Key::T if ctrl => NewTab,
        Key::U if ctrl => Exchange,
        Key::W if ctrl => CloseTab,
        Key::W if ctrl_shift => CloseAllTabs,
        _ => return None,
    })
}

/// A main menu entry. `cmd == None` with an empty label is a separator.
pub struct MenuItem {
    pub label: &'static str,
    pub shortcut: &'static str,
    pub cmd: Option<Cmd>,
}

const fn item(label: &'static str, shortcut: &'static str, cmd: Cmd) -> MenuItem {
    MenuItem { label, shortcut, cmd: Some(cmd) }
}

const SEP: MenuItem = MenuItem { label: "", shortcut: "", cmd: None };

pub fn main_menu() -> Vec<(&'static str, Vec<MenuItem>)> {
    use Cmd::*;
    vec![
        (
            "Files",
            vec![
                item("Change Attributes...", "", Attributes),
                item("Pack...", "Alt+F5", Pack),
                item("Unpack Specific Files...", "Alt+F9", Unpack),
                item("Test Archive(s)", "Alt+Shift+F9", TestArchive),
                item("Compare By Content...", "", CompareContent),
                item("Properties...", "Alt+Enter", Properties),
                item("Calculate Occupied Space...", "Ctrl+L", OccupiedSpace),
                item("Multi-Rename Tool...", "Ctrl+M", MultiRename),
                SEP,
                item("Create Checksum File(s) (SHA-256)...", "", Checksum),
                SEP,
                item("Quit", "Alt+F4", Exit),
            ],
        ),
        (
            "Mark",
            vec![
                item("Select Group...", "Num +", SelectGroup),
                item("Unselect Group...", "Num -", UnselectGroup),
                item("Select All", "Ctrl+Num +", SelectAll),
                item("Unselect All", "Ctrl+Num -", UnselectAll),
                item("Invert Selection", "Num *", InvertSelection),
                item("Select All With Same Extension", "Alt+Num +", SelectSameExt),
                SEP,
                item("Save Selection", "", SaveSelection),
                item("Restore Selection", "Num /", RestoreSelection),
                SEP,
                item("Copy Selected Names To Clipboard", "", CopyNames),
                item("Copy Names With Path To Clipboard", "", CopyFullNames),
                item("Copy To Clipboard With All Details", "", CopyDetails),
                SEP,
                item("Compare Directories", "Shift+F2", CompareDirs),
            ],
        ),
        (
            "Commands",
            vec![
                item("CD Tree...", "Alt+F10", CdTree),
                item("Search...", "Alt+F7", Search),
                item("Synchronize Dirs...", "", SyncDirs),
                item("Directory Hotlist", "Ctrl+D", Hotlist),
                item("Go Back", "Alt+Left", HistoryBack),
                item("Go Forward", "Alt+Right", HistoryForward),
                SEP,
                item("Open Command Prompt Window", "", OpenTerminal),
                item("Open Explorer Here", "", OpenExplorer),
                SEP,
                item("Branch View (With Subdirs)", "Ctrl+B", BranchView),
                item("Open Desktop Folder", "", OpenDesktop),
                SEP,
                item("Source <-> Target", "Ctrl+U", Exchange),
                item("Target = Source", "", TargetEqualsSource),
            ],
        ),
        (
            "Net",
            vec![
                item("Map Network Drive...", "", MapNetworkDrive),
                item("Disconnect Network Drive...", "", DisconnectNetworkDrive),
                item("Network Drives", "", NetworkDrives),
            ],
        ),
        (
            "Show",
            vec![
                item("Brief", "Ctrl+F1", Brief),
                item("Full", "Ctrl+F2", Full),
                item("Quick View Panel", "Ctrl+Q", QuickView),
                item("New Folder Tab", "Ctrl+T", NewTab),
                SEP,
                item("All Files", "Ctrl+F10", ShowAll),
                item("Programs", "Ctrl+F11", ShowPrograms),
                item("Custom...", "Ctrl+F12", ShowCustom),
                item("Quick Filter...", "Ctrl+S", QuickFilter),
                item("Hidden/System Files", "Ctrl+H", ToggleHidden),
                SEP,
                item("Name", "Ctrl+F3", SortBy(SortColumn::Name)),
                item("Extension", "Ctrl+F4", SortBy(SortColumn::Ext)),
                item("Time", "Ctrl+F5", SortBy(SortColumn::Date)),
                item("Size", "Ctrl+F6", SortBy(SortColumn::Size)),
                item("Unsorted", "Ctrl+F7", SortBy(SortColumn::Unsorted)),
                SEP,
                item("Reversed Order", "", ReverseSort),
                SEP,
                item("Reread Source", "Ctrl+R", Reread),
            ],
        ),
        (
            "Configuration",
            vec![
                item("Options...", "", Options),
                item("Dark <-> Normal", "", ToggleTheme),
                item("Import Total Commander Settings", "", ImportTc),
                item("Directory Hotlist...", "", ConfigureHotlist),
                SEP,
                item("Save Settings", "", SaveSettings),
            ],
        ),
        (
            "Start",
            vec![
                item("Command Prompt", "", Run("cmd.exe".into())),
                item("PowerShell", "", Run("powershell.exe".into())),
                item("Visual Studio Code Here", "", Run("code .".into())),
                item("Explorer Here", "", OpenExplorer),
                item("Notepad", "", Run("notepad.exe".into())),
            ],
        ),
    ]
}

pub fn help_menu() -> Vec<MenuItem> {
    use Cmd::*;
    vec![item("Keyboard", "F1", KeyboardHelp), SEP, item("About MCommander...", "", About)]
}

/// Text for the F1 keyboard reference dialog.
pub const KEYBOARD_HELP: &[(&str, &str)] = &[
    ("F9 / F10, Alt+F/M/C/N/W/O/S/H", "Open the main menu (arrows to navigate)"),
    ("F2 / Ctrl+R", "Reread source window"),
    ("F3", "View file (Lister)"),
    ("F4", "Edit file"),
    ("F5 / Shift+F5", "Copy / copy in same directory"),
    ("F6 / Shift+F6", "Move or rename / rename in place"),
    ("F7", "Create directory"),
    ("F8, Del", "Delete (recycle bin); Shift: delete permanently"),
    ("Alt+F1 / Alt+F2", "Change left / right drive"),
    ("Alt+F5 / Alt+F9", "Pack / unpack"),
    ("Alt+F7", "Find files"),
    ("Alt+F8, Ctrl+Down", "Command line history"),
    ("Alt+F10", "Directory tree of the current drive"),
    ("Alt+Left / Right / Down", "Directory history back / forward / list"),
    ("Shift+F2", "Compare directories"),
    ("Shift+F4", "Create new text file and edit"),
    ("Shift+F10", "Context menu"),
    ("Tab", "Switch between panels"),
    ("Insert / Space", "Select file (Space also counts directory size)"),
    ("Num + / Num - / Num *", "Select group / unselect group / invert"),
    ("Ctrl+Num + / Ctrl+Num -", "Select all / unselect all"),
    ("Alt+Num +", "Select all files with the same extension"),
    ("Num /", "Restore selection"),
    ("Backspace, Ctrl+PgUp", "Parent directory"),
    ("Ctrl+PgDn", "Open directory or archive"),
    ("Ctrl+\\", "Root directory"),
    ("Ctrl+Left / Ctrl+Right", "Open directory in left / right panel"),
    ("Ctrl+F1 / Ctrl+F2", "Brief / full view"),
    ("Ctrl+F3..F7", "Sort by name / extension / time / size / unsorted"),
    ("Ctrl+F10 / F11 / F12", "All files / programs / custom filter"),
    ("Ctrl+A", "Select all"),
    ("Ctrl+B", "Branch view (all files in subdirectories)"),
    ("Ctrl+C / X / V", "Copy / cut / paste files via clipboard"),
    ("Ctrl+D", "Directory hotlist"),
    ("Ctrl+H", "Show hidden and system files"),
    ("Ctrl+L", "Calculate occupied space"),
    ("Ctrl+M", "Multi-rename tool"),
    ("Ctrl+P", "Copy current path to command line"),
    ("Ctrl+Q", "Quick view panel"),
    ("Ctrl+S", "Quick filter"),
    ("Ctrl+T / Ctrl+W", "New tab / close tab"),
    ("Ctrl+Tab", "Next tab"),
    ("Ctrl+U", "Exchange panels"),
    ("Ctrl+Up", "Open directory under cursor in new tab"),
    ("Ctrl+Enter", "Copy file name to command line"),
    ("Alt+Enter", "Properties"),
    ("Alt+Shift+Enter", "Calculate size of all directories"),
    ("Ctrl+Alt+Letters", "Quick search (configurable)"),
    ("Enter (command line)", "Run command; cd/md/rd handled internally"),
];

#[cfg(test)]
mod tests {
    use super::*;

    const CTX: KeyCtx = KeyCtx { cmdline_empty: true, brief: false };

    #[test]
    fn function_keys() {
        assert_eq!(map_key(Key::F5, Modifiers::NONE, CTX), Some(Cmd::Copy));
        assert_eq!(map_key(Key::F6, Modifiers::SHIFT, CTX), Some(Cmd::Rename));
        assert_eq!(map_key(Key::F7, Modifiers::ALT, CTX), Some(Cmd::Search));
        assert_eq!(map_key(Key::F3, Modifiers::CTRL, CTX), Some(Cmd::SortBy(SortColumn::Name)));
    }

    #[test]
    fn command_line_dependent_keys() {
        let typing = KeyCtx { cmdline_empty: false, brief: false };
        assert_eq!(map_key(Key::Enter, Modifiers::NONE, CTX), Some(Cmd::Open));
        assert_eq!(map_key(Key::Enter, Modifiers::NONE, typing), None);
        assert_eq!(map_key(Key::Backspace, Modifiers::NONE, typing), None);
        assert_eq!(map_key(Key::Space, Modifiers::NONE, typing), None);
        assert_eq!(map_key(Key::ArrowUp, Modifiers::NONE, typing), Some(Cmd::Up));
        let brief = KeyCtx { cmdline_empty: true, brief: true };
        assert_eq!(map_key(Key::ArrowRight, Modifiers::NONE, brief), Some(Cmd::Right));
    }

    #[test]
    fn menus_have_no_duplicate_shortcuts() {
        let mut seen = std::collections::HashSet::new();
        for (_, items) in main_menu() {
            for it in items.iter().filter(|i| !i.shortcut.is_empty()) {
                assert!(seen.insert(it.shortcut), "duplicate shortcut {}", it.shortcut);
            }
        }
    }
}
