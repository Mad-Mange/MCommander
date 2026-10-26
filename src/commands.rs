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
    /// Saved FTP/SFTP connections (Ctrl+F).
    FtpConnect,
    /// Connect to a URL or host (Ctrl+N).
    FtpNew,
    /// Disconnect the server in the active panel (Ctrl+Shift+F).
    FtpDisconnect,
    /// Jump to entry n of the active tab's directory history.
    HistoryGoto(usize),
    // View
    Brief,
    Full,
    Thumbnails,
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
    /// Run a button bar button; `dropped` are files dragged onto it.
    RunButton { button: Box<crate::buttonbar::BarButton>, dropped: Vec<std::path::PathBuf> },
    /// "Change button bar" dialog, optionally with a button selected.
    ConfigureBar(crate::buttonbar::BarKind),
    EditBarButton(crate::buttonbar::BarKind, usize),
    DeleteBarButton(crate::buttonbar::BarKind, usize),
    /// Show or hide a button bar.
    ToggleBar(crate::buttonbar::BarKind),
    /// Open main menu number n (0 = Files ... 7 = Help) as a keyboard menu.
    MenuOpen(usize),
}

/// An internal command as Total Commander names it in button bars, menus and
/// on the command line (`cm_Copy`), with its number from TC's totalcmd.inc.
pub struct TcCommand {
    pub name: &'static str,
    pub id: u32,
    pub cmd: Cmd,
    pub help: &'static str,
}

/// The `cm_*` commands MCommander understands. Names without a TC number
/// (id 0) are MCommander's own.
pub fn tc_commands() -> &'static [TcCommand] {
    use Cmd::*;
    static LIST: std::sync::LazyLock<Vec<TcCommand>> = std::sync::LazyLock::new(|| {
        let c = |name, id, cmd, help| TcCommand { name, id, cmd, help };
        vec![
            c("cm_SrcShort", 301, Brief, "Brief view: only file names"),
            c("cm_SrcLong", 302, Full, "Full view: all file details"),
            c("cm_SrcThumbs", 269, Thumbnails, "Thumbnails view"),
            c("cm_SrcQuickView", 304, QuickView, "Quick view panel"),
            c("cm_SrcByName", 321, SortBy(SortColumn::Name), "Sort by name"),
            c("cm_SrcByExt", 322, SortBy(SortColumn::Ext), "Sort by extension"),
            c("cm_SrcBySize", 323, SortBy(SortColumn::Size), "Sort by size"),
            c("cm_SrcByDateTime", 324, SortBy(SortColumn::Date), "Sort by date"),
            c("cm_SrcUnsorted", 325, SortBy(SortColumn::Unsorted), "Unsorted"),
            c("cm_SrcNegOrder", 330, ReverseSort, "Reversed sort order"),
            c("cm_SrcAllFiles", 312, ShowAll, "Show all files"),
            c("cm_SrcExecs", 311, ShowPrograms, "Show only programs"),
            c("cm_SrcUserDef", 314, ShowCustom, "Show files of a chosen type"),
            c("cm_LeftOpenDrives", 131, DriveMenu(Side::Left), "Open the left drive list"),
            c("cm_RightOpenDrives", 231, DriveMenu(Side::Right), "Open the right drive list"),
            c("cm_CDtree", 500, CdTree, "Directory tree"),
            c("cm_SearchFor", 501, Search, "Find files"),
            c("cm_SetAttrib", 502, Attributes, "Change attributes"),
            c("cm_GetFileSpace", 503, OccupiedSpace, "Calculate occupied space"),
            c("cm_PackFiles", 508, Pack, "Pack files"),
            c("cm_UnpackFiles", 509, Unpack, "Unpack files"),
            c("cm_ExecuteDOS", 511, OpenTerminal, "Open command prompt window"),
            c("cm_NetConnect", 512, MapNetworkDrive, "Map network drive"),
            c("cm_NetDisconnect", 513, DisconnectNetworkDrive, "Disconnect network drive"),
            c("cm_TestArchive", 518, TestArchive, "Test archives"),
            c("cm_SpreadSelection", 521, SelectGroup, "Select group"),
            c("cm_ShrinkSelection", 522, UnselectGroup, "Unselect group"),
            c("cm_SelectAll", 523, SelectAll, "Select all"),
            c("cm_ClearAll", 524, UnselectAll, "Unselect all"),
            c("cm_ExchangeSelection", 525, InvertSelection, "Invert selection"),
            c("cm_DirectoryHotlist", 526, Hotlist, "Directory hotlist"),
            c("cm_SelectCurrentExtension", 527, SelectSameExt, "Select all files with the same extension"),
            c("cm_UnselectCurrentExtension", 528, UnselectSameExt, "Unselect all files with the same extension"),
            c("cm_RestoreSelection", 529, RestoreSelection, "Restore selection"),
            c("cm_SaveSelection", 530, SaveSelection, "Save selection"),
            c("cm_Exchange", 531, Exchange, "Swap panels"),
            c("cm_CompareDirs", 533, CompareDirs, "Compare directories"),
            c("cm_RereadSource", 540, Reread, "Reread source"),
            c("cm_FtpConnect", 550, FtpConnect, "Connect to FTP/SFTP server"),
            c("cm_FtpNew", 551, FtpNew, "New FTP/SFTP connection"),
            c("cm_FtpDisconnect", 552, FtpDisconnect, "Disconnect from server"),
            c("cm_CRCcreate", 564, Checksum, "Create checksum files"),
            c("cm_GoToPreviousDir", 570, HistoryBack, "Back"),
            c("cm_GoToNextDir", 571, HistoryForward, "Forward"),
            c("cm_DirectoryHistory", 572, HistoryMenu, "Directory history"),
            c("cm_ConfigSaveSettings", 580, SaveSettings, "Save settings"),
            c("cm_Config", 490, Options, "Options"),
            c("cm_ButtonConfig", 498, ConfigureBar(crate::buttonbar::BarKind::Main), "Change button bar"),
            c("cm_HelpIndex", 610, KeyboardHelp, "Help"),
            c("cm_Keyboard", 620, KeyboardHelp, "Keyboard help"),
            c("cm_About", 690, About, "About MCommander"),
            c("cm_List", 903, View, "View (Lister)"),
            c("cm_Edit", 904, Edit, "Edit"),
            c("cm_Copy", 905, Copy, "Copy"),
            c("cm_RenMov", 906, Move, "Move / rename"),
            c("cm_MkDir", 907, MkDir, "New folder"),
            c("cm_Delete", 908, Delete, "Delete"),
            c("cm_Return", 1001, Open, "Enter (open)"),
            c("cm_RenameOnly", 1002, Rename, "Rename in place"),
            c("cm_Properties", 1003, Properties, "Properties"),
            c("cm_GoToRoot", 2001, Root, "Go to root directory"),
            c("cm_GoToParent", 2002, Parent, "Go to parent directory"),
            c("cm_GoToDir", 2003, Open, "Open directory or archive"),
            c("cm_CutToClipboard", 2007, ClipCut, "Cut files to the clipboard"),
            c("cm_CopyToClipboard", 2008, ClipCopy, "Copy files to the clipboard"),
            c("cm_PasteFromClipboard", 2009, ClipPaste, "Paste files from the clipboard"),
            c("cm_SwitchHidSys", 2011, ToggleHidden, "Show hidden/system files"),
            c("cm_CountDirContent", 2014, AllDirSizes, "Calculate the size of all directories"),
            c("cm_CopyNamesToClip", 2017, CopyNames, "Copy names to the clipboard"),
            c("cm_CopyFullNamesToClip", 2018, CopyFullNames, "Copy names with path to the clipboard"),
            c("cm_AddPathToCmdline", 2019, CmdCopyPath, "Copy path to the command line"),
            c("cm_FileSync", 2020, SyncDirs, "Synchronize directories"),
            c("cm_CompareFilesByContent", 2022, CompareContent, "Compare by content"),
            c("cm_DirBranch", 2026, BranchView, "Branch view (all files in subdirectories)"),
            c("cm_CopyFileDetailsToClip", 2036, CopyDetails, "Copy names with all details to the clipboard"),
            c("cm_OpenDesktop", 2121, OpenDesktop, "Desktop folder"),
            c("cm_MultiRenameFiles", 2400, MultiRename, "Multi-Rename Tool"),
            c("cm_ContextMenu", 2500, ContextMenu, "Context menu"),
            c("cm_VisButtonBar", 2901, ToggleBar(crate::buttonbar::BarKind::Main), "Show/hide button bar"),
            c("cm_EditNewFile", 2931, EditNew, "Create and edit a new file"),
            c("cm_VisButtonBar2", 2944, ToggleBar(crate::buttonbar::BarKind::Vertical), "Show/hide vertical button bar"),
            c("cm_SwitchDarkMode", 2950, ToggleTheme, "Dark <-> normal"),
            c("cm_OpenNewTab", 3001, NewTab, "New folder tab"),
            c("cm_OpenDirInNewTab", 3003, OpenDirInNewTab, "Open directory in a new tab"),
            c("cm_SwitchToNextTab", 3005, NextTab, "Next tab"),
            c("cm_SwitchToPreviousTab", 3006, PrevTab, "Previous tab"),
            c("cm_CloseCurrentTab", 3007, CloseTab, "Close tab"),
            c("cm_CloseAllTabs", 3008, CloseAllTabs, "Close all tabs"),
            c("cm_QuickFilter", 3019, QuickFilter, "Quick filter"),
            c("cm_AddFileNameToCmdline", 3021, CmdAddName, "Copy file name to the command line"),
            c("cm_AddPathAndFileNameToCmdline", 3022, CmdAddPath, "Copy path and file name to the command line"),
            c("cm_ShowCmdLineHistory", 3023, CmdHistory, "Command line history"),
            c("cm_CopySamePanel", 3100, CopySame, "Copy within the panel"),
            c("cm_FocusCmdLine", 4003, FocusCmdLine, "Focus the command line"),
            c("cm_Exit", 24340, Exit, "Exit"),
            c("cm_ExplorerHere", 0, OpenExplorer, "Open Explorer here"),
            c("cm_AddToHotlist", 0, AddToHotlist, "Add the current directory to the hotlist"),
            c("cm_ConfigDirHotlist", 0, ConfigureHotlist, "Configure the directory hotlist"),
            c("cm_ButtonConfigVertical", 0, ConfigureBar(crate::buttonbar::BarKind::Vertical), "Change vertical button bar"),
            c("cm_ImportTcSettings", 0, ImportTc, "Import Total Commander settings"),
        ]
    });
    &LIST
}

/// Looks up `cm_Name` (any case) or a command number.
pub fn tc_command(name: &str) -> Option<&'static TcCommand> {
    let name = name.trim();
    match name.parse::<u32>() {
        Ok(id) if id > 0 => tc_commands().iter().find(|c| c.id == id),
        _ => tc_commands().iter().find(|c| c.name.eq_ignore_ascii_case(name)),
    }
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
        Key::F1 if ctrl_shift => Thumbnails,
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
        Key::F if ctrl => FtpConnect,
        Key::F if ctrl_shift => FtpDisconnect,
        Key::N if ctrl => FtpNew,
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
                item("FTP/SFTP Connect...", "Ctrl+F", FtpConnect),
                item("FTP/SFTP New Connection...", "Ctrl+N", FtpNew),
                item("FTP/SFTP Disconnect", "Ctrl+Shift+F", FtpDisconnect),
                SEP,
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
                item("Thumbnails", "Ctrl+Shift+F1", Thumbnails),
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
                item("Button Bar...", "", ConfigureBar(crate::buttonbar::BarKind::Main)),
                item("Vertical Button Bar...", "", ConfigureBar(crate::buttonbar::BarKind::Vertical)),
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

/// The keyboard shortcut of a command, for tooltips.
pub fn shortcut_of(cmd: &Cmd) -> Option<&'static str> {
    use Cmd::*;
    let fkey = match cmd {
        View => "F3",
        Edit => "F4",
        Copy => "F5",
        Move => "F6",
        MkDir => "F7",
        Delete => "F8",
        Rename => "Shift+F6",
        CopySame => "Shift+F5",
        EditNew => "Shift+F4",
        ContextMenu => "Shift+F10",
        NewTab => "Ctrl+T",
        CloseTab => "Ctrl+W",
        _ => "",
    };
    if !fkey.is_empty() {
        return Some(fkey);
    }
    main_menu()
        .into_iter()
        .flat_map(|(_, items)| items)
        .chain(help_menu())
        .find(|i| i.cmd.as_ref() == Some(cmd) && !i.shortcut.is_empty())
        .map(|i| i.shortcut)
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
    ("Ctrl+Shift+F1", "Thumbnails view"),
    ("Ctrl+F3..F7", "Sort by name / extension / time / size / unsorted"),
    ("Ctrl+F10 / F11 / F12", "All files / programs / custom filter"),
    ("Ctrl+A", "Select all"),
    ("Ctrl+B", "Branch view (all files in subdirectories)"),
    ("Ctrl+C / X / V", "Copy / cut / paste files via clipboard"),
    ("Ctrl+D", "Directory hotlist"),
    ("Ctrl+F", "Connect to an FTP/FTPS/SFTP server (saved connections)"),
    ("Ctrl+N", "New FTP/FTPS/SFTP connection (host name or URL)"),
    ("Ctrl+Shift+F", "Disconnect from the server"),
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
    ("Click on the path line", "A folder: go there; elsewhere: type or paste a path (Enter)"),
    ("Right mouse button", "Select / unselect; drag to select several; hold for the context menu"),
    ("Shift+right click", "Select everything from the last clicked file"),
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
        assert_eq!(map_key(Key::F, Modifiers::CTRL, CTX), Some(Cmd::FtpConnect));
        assert_eq!(map_key(Key::F, Modifiers::CTRL | Modifiers::SHIFT, CTX), Some(Cmd::FtpDisconnect));
        assert_eq!(map_key(Key::N, Modifiers::CTRL, CTX), Some(Cmd::FtpNew));
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
