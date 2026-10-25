//! Win32 integration: drives, shell execution, recycle bin, file clipboard,
//! shell icons, property sheets and a keyboard hook on the main window.

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW,
    HGDIOBJ, ReleaseDC,
};
use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES, GetDiskFreeSpaceExW, GetDriveTypeW,
    GetLogicalDrives, GetVolumeInformationW,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GHND, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_HDROP;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_ADD, VK_CONTROL, VK_DIVIDE, VK_MENU, VK_MULTIPLY, VK_SHIFT, VK_SUBTRACT};
use windows::Win32::UI::Shell::{
    DROPFILES, DefSubclassProc, DragQueryFileW, FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI,
    FOF_SILENT, HDROP, SHFILEINFOW, SHFILEOPSTRUCTW, SHFileOperationW, SHGFI_ICON, SHGFI_SMALLICON, SHGFI_TYPENAME,
    SHGFI_USEFILEATTRIBUTES, SHGetFileInfoW, SHOP_FILEPATH, SHObjectProperties, SetWindowSubclass, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, GetIconInfo, HICON, ICONINFO, SW_SHOWNORMAL, WM_CHAR, WM_DRAWITEM, WM_INITMENUPOPUP, WM_KEYDOWN,
    WM_KEYUP, WM_MEASUREITEM, WM_MENUCHAR, WM_SYSCHAR, WM_SYSCOMMAND, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use windows::core::PCWSTR;

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn wide_path(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

fn pcwstr(v: &[u16]) -> PCWSTR {
    PCWSTR(v.as_ptr())
}

// ---------------------------------------------------------------------------
// Drives

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveKind {
    Fixed,
    Removable,
    Network,
    Cdrom,
    Ram,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct DriveInfo {
    pub letter: char,
    pub kind: DriveKind,
}

pub fn drives() -> Vec<DriveInfo> {
    let mask = unsafe { GetLogicalDrives() };
    (0..26)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| {
            let letter = (b'A' + i as u8) as char;
            let root = wide(&format!("{letter}:\\"));
            let kind = match unsafe { GetDriveTypeW(pcwstr(&root)) } {
                2 => DriveKind::Removable,
                3 => DriveKind::Fixed,
                4 => DriveKind::Network,
                5 => DriveKind::Cdrom,
                6 => DriveKind::Ram,
                _ => DriveKind::Unknown,
            };
            DriveInfo { letter, kind }
        })
        .collect()
}

/// Volume label; may block on slow or disconnected drives, so call it off the UI thread.
pub fn volume_label(root: &Path) -> Option<String> {
    let w = wide_path(root);
    let mut buf = [0u16; 261];
    unsafe { GetVolumeInformationW(pcwstr(&w), Some(&mut buf), None, None, None, None) }.ok()?;
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..len]))
}

/// (free bytes available to the caller, total bytes).
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    let w = wide_path(path);
    let (mut free, mut total) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(pcwstr(&w), Some(&mut free), Some(&mut total), None) }.ok()?;
    Some((free, total))
}

// ---------------------------------------------------------------------------
// Shell

pub fn shell_open(target: &Path, dir: Option<&Path>) -> Result<(), String> {
    shell_execute("open", &target.to_string_lossy(), "", dir)
}

pub fn shell_execute(verb: &str, file: &str, params: &str, dir: Option<&Path>) -> Result<(), String> {
    let verb_w = wide(verb);
    let file_w = wide(file);
    let params_w = wide(params);
    let dir_w = dir.map(wide_path);
    let res = unsafe {
        ShellExecuteW(
            None,
            pcwstr(&verb_w),
            pcwstr(&file_w),
            if params.is_empty() { PCWSTR::null() } else { pcwstr(&params_w) },
            dir_w.as_ref().map(|d| pcwstr(d)).unwrap_or(PCWSTR::null()),
            SW_SHOWNORMAL,
        )
    };
    let code = res.0 as isize;
    if code > 32 {
        Ok(())
    } else {
        Err(match code {
            2 => "File not found".into(),
            3 => "Path not found".into(),
            5 => "Access denied".into(),
            31 => "No application is associated with this file type".into(),
            c => format!("ShellExecute failed (code {c})"),
        })
    }
}

pub fn show_properties(hwnd: Option<HWND>, path: &Path) {
    let w = wide_path(path);
    unsafe {
        let _ = SHObjectProperties(hwnd, SHOP_FILEPATH, pcwstr(&w), PCWSTR::null());
    }
}

/// Moves files to the recycle bin. Returns Err with a message on failure.
pub fn recycle(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    // pFrom is a double-NUL-terminated list of NUL-separated paths.
    let mut from: Vec<u16> = Vec::new();
    for p in paths {
        from.extend(p.as_os_str().encode_wide());
        from.push(0);
    }
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0 | FOF_SILENT.0 | FOF_NOERRORUI.0) as u16,
        ..Default::default()
    };
    let rc = unsafe { SHFileOperationW(&mut op) };
    if rc != 0 {
        Err(format!("Could not move to recycle bin (error 0x{rc:X})"))
    } else if op.fAnyOperationsAborted.as_bool() {
        Err("Operation aborted".into())
    } else {
        Ok(())
    }
}

/// Explorer-style type name ("Text Document") for a file name.
pub fn type_name(name: &str, is_dir: bool) -> String {
    let w = wide(name);
    let mut info = SHFILEINFOW::default();
    let attrs = if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_NORMAL };
    unsafe {
        SHGetFileInfoW(
            pcwstr(&w),
            attrs,
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_TYPENAME | SHGFI_USEFILEATTRIBUTES,
        );
    }
    let len = info.szTypeName.iter().position(|&c| c == 0).unwrap_or(0);
    String::from_utf16_lossy(&info.szTypeName[..len])
}

// ---------------------------------------------------------------------------
// Icons

pub struct IconImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Small shell icon for a path. With `generic` the file is not touched and
/// only its name/attributes decide the icon (fast, works for any extension).
pub fn file_icon(path: &str, is_dir: bool, generic: bool) -> Option<IconImage> {
    let w = wide(path);
    let mut info = SHFILEINFOW::default();
    let mut flags = SHGFI_ICON | SHGFI_SMALLICON;
    if generic {
        flags |= SHGFI_USEFILEATTRIBUTES;
    }
    let attrs: FILE_FLAGS_AND_ATTRIBUTES = if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { FILE_ATTRIBUTE_NORMAL };
    let ok = unsafe {
        SHGetFileInfoW(pcwstr(&w), attrs, Some(&mut info), std::mem::size_of::<SHFILEINFOW>() as u32, flags)
    };
    if ok == 0 || info.hIcon.is_invalid() {
        return None;
    }
    let img = icon_to_rgba(info.hIcon);
    unsafe {
        let _ = DestroyIcon(info.hIcon);
    }
    img
}

fn icon_to_rgba(icon: HICON) -> Option<IconImage> {
    unsafe {
        let mut ii = ICONINFO::default();
        GetIconInfo(icon, &mut ii).ok()?;
        let cleanup = |ii: &ICONINFO| {
            if !ii.hbmColor.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(ii.hbmColor.0));
            }
            if !ii.hbmMask.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(ii.hbmMask.0));
            }
        };
        if ii.hbmColor.is_invalid() {
            cleanup(&ii);
            return None;
        }
        let mut bm = BITMAP::default();
        if GetObjectW(HGDIOBJ(ii.hbmColor.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void)) == 0 {
            cleanup(&ii);
            return None;
        }
        let (w, h) = (bm.bmWidth as usize, bm.bmHeight as usize);
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = GetDC(None);
        let mut bgra = vec![0u8; w * h * 4];
        let lines = GetDIBits(dc, ii.hbmColor, 0, h as u32, Some(bgra.as_mut_ptr() as *mut c_void), &mut bmi, DIB_RGB_COLORS);
        let mut mask = vec![0u8; w * h * 4];
        let has_mask = !ii.hbmMask.is_invalid()
            && GetDIBits(dc, ii.hbmMask, 0, h as u32, Some(mask.as_mut_ptr() as *mut c_void), &mut bmi, DIB_RGB_COLORS) > 0;
        ReleaseDC(None, dc);
        cleanup(&ii);
        if lines == 0 {
            return None;
        }
        let has_alpha = bgra.chunks_exact(4).any(|p| p[3] != 0);
        let mut rgba = Vec::with_capacity(w * h * 4);
        for (i, p) in bgra.chunks_exact(4).enumerate() {
            let a = if has_alpha {
                p[3]
            } else if has_mask && mask[i * 4] != 0 {
                0
            } else {
                255
            };
            rgba.extend_from_slice(&[p[2], p[1], p[0], a]);
        }
        Some(IconImage { width: w, height: h, rgba })
    }
}

// ---------------------------------------------------------------------------
// File clipboard (CF_HDROP + "Preferred DropEffect", compatible with Explorer)

const DROPEFFECT_COPY: u32 = 1;
const DROPEFFECT_MOVE: u32 = 2;
const DROPEFFECT_LINK: u32 = 4;

fn drop_effect_format() -> u32 {
    let name = wide("Preferred DropEffect");
    unsafe { RegisterClipboardFormatW(pcwstr(&name)) }
}

unsafe fn global_from_bytes(bytes: &[u8]) -> Option<HGLOBAL> {
    unsafe {
        let h = GlobalAlloc(GHND, bytes.len()).ok()?;
        let p = GlobalLock(h) as *mut u8;
        if p.is_null() {
            return None;
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        let _ = GlobalUnlock(h);
        Some(h)
    }
}

pub fn set_clipboard_files(paths: &[PathBuf], cut: bool) -> Result<(), String> {
    let mut data = Vec::new();
    let header = DROPFILES {
        pFiles: std::mem::size_of::<DROPFILES>() as u32,
        pt: POINT { x: 0, y: 0 },
        fNC: false.into(),
        fWide: true.into(),
    };
    let header_bytes =
        unsafe { std::slice::from_raw_parts(&header as *const DROPFILES as *const u8, std::mem::size_of::<DROPFILES>()) };
    data.extend_from_slice(header_bytes);
    for p in paths {
        for u in p.as_os_str().encode_wide().chain(std::iter::once(0)) {
            data.extend_from_slice(&u.to_le_bytes());
        }
    }
    data.extend_from_slice(&[0, 0]);
    let effect = if cut { DROPEFFECT_MOVE } else { DROPEFFECT_COPY | DROPEFFECT_LINK };
    unsafe {
        OpenClipboard(None).map_err(|e| e.to_string())?;
        let result = (|| {
            EmptyClipboard().map_err(|e| e.to_string())?;
            let h = global_from_bytes(&data).ok_or("out of memory")?;
            SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(h.0))).map_err(|e| e.to_string())?;
            let h2 = global_from_bytes(&effect.to_le_bytes()).ok_or("out of memory")?;
            SetClipboardData(drop_effect_format(), Some(HANDLE(h2.0))).map_err(|e| e.to_string())?;
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

/// Files on the clipboard and whether they were cut (move) rather than copied.
pub fn get_clipboard_files() -> Option<(Vec<PathBuf>, bool)> {
    unsafe {
        IsClipboardFormatAvailable(CF_HDROP.0 as u32).ok()?;
        OpenClipboard(None).ok()?;
        let result = (|| {
            let h = GetClipboardData(CF_HDROP.0 as u32).ok()?;
            let hdrop = HDROP(h.0);
            let count = DragQueryFileW(hdrop, 0xFFFF_FFFF, None);
            let mut files = Vec::new();
            for i in 0..count {
                let len = DragQueryFileW(hdrop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(hdrop, i, Some(&mut buf));
                files.push(PathBuf::from(String::from_utf16_lossy(&buf[..len])));
            }
            let mut cut = false;
            if let Ok(h) = GetClipboardData(drop_effect_format()) {
                let p = GlobalLock(HGLOBAL(h.0)) as *const u32;
                if !p.is_null() {
                    cut = *p & DROPEFFECT_MOVE != 0 && *p & DROPEFFECT_COPY == 0;
                    let _ = GlobalUnlock(HGLOBAL(h.0));
                }
            }
            Some((files, cut))
        })();
        let _ = CloseClipboard();
        result
    }
}

// ---------------------------------------------------------------------------
// Keyboard hook on the main window.
//
// egui-winit maps NumpadAdd/NumpadSubtract to the same keys as the main
// keyboard and turns Ctrl+C/X/V into text clipboard events (with no event at
// all when the clipboard holds files). Total Commander gives these keys
// file-panel meanings, so we intercept them natively while the file panel
// has the keyboard.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RawKey {
    pub vk: u16,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

pub const VK_NUM_PLUS: u16 = VK_ADD.0;
pub const VK_NUM_MINUS: u16 = VK_SUBTRACT.0;
pub const VK_NUM_STAR: u16 = VK_MULTIPLY.0;
pub const VK_NUM_SLASH: u16 = VK_DIVIDE.0;

/// Set by the app every frame: true while keys belong to the file panel.
pub static PANEL_KEYS: AtomicBool = AtomicBool::new(false);
static SWALLOW_CHAR: AtomicBool = AtomicBool::new(false);
static RAW_KEYS: Mutex<Vec<RawKey>> = Mutex::new(Vec::new());
static EGUI_CTX: OnceLock<egui::Context> = OnceLock::new();

/// Queues a key as if it came from the keyboard hook (scripted tests).
pub fn push_raw_key(k: RawKey) {
    RAW_KEYS.lock().unwrap().push(k);
}

pub fn take_raw_keys() -> Vec<RawKey> {
    std::mem::take(&mut *RAW_KEYS.lock().unwrap())
}

fn down(vk: u16) -> bool {
    unsafe { GetKeyState(vk as i32) < 0 }
}

pub fn shift_down() -> bool {
    down(VK_SHIFT.0)
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    // Owner-drawn shell submenus ("Send to", "Open with") need these messages.
    if matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
        if let Some(r) = forward_menu_msg(msg, wparam, lparam) {
            return r;
        }
    }
    match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN => {
            SWALLOW_CHAR.store(false, Ordering::Relaxed);
            if PANEL_KEYS.load(Ordering::Relaxed) {
                let vk = wparam.0 as u16;
                let key = RawKey { vk, ctrl: down(VK_CONTROL.0), shift: down(VK_SHIFT.0), alt: down(VK_MENU.0) };
                let numpad = matches!(vk, v if v == VK_ADD.0 || v == VK_SUBTRACT.0 || v == VK_MULTIPLY.0 || v == VK_DIVIDE.0);
                let clip = key.ctrl && !key.alt && matches!(vk, 0x43 | 0x58 | 0x56); // C X V
                if numpad || clip {
                    RAW_KEYS.lock().unwrap().push(key);
                    SWALLOW_CHAR.store(true, Ordering::Relaxed);
                    if let Some(ctx) = EGUI_CTX.get() {
                        ctx.request_repaint();
                    }
                    return LRESULT(0);
                }
            }
        }
        WM_CHAR | WM_SYSCHAR if SWALLOW_CHAR.swap(false, Ordering::Relaxed) => return LRESULT(0),
        WM_KEYUP | WM_SYSKEYUP => {
            let vk = wparam.0 as u16;
            if PANEL_KEYS.load(Ordering::Relaxed)
                && (vk == VK_ADD.0 || vk == VK_SUBTRACT.0 || vk == VK_MULTIPLY.0 || vk == VK_DIVIDE.0)
            {
                return LRESULT(0);
            }
        }
        // Alt or F10 alone would enter the (invisible) system menu loop and
        // freeze rendering; keep Alt+Space working.
        WM_SYSCOMMAND if (wparam.0 & 0xFFF0) == 0xF100 && lparam.0 == 0 => return LRESULT(0),
        _ => {}
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

pub fn install_key_hook(hwnd: HWND, ctx: &egui::Context) {
    let _ = EGUI_CTX.set(ctx.clone());
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(subclass_proc), 0x4D43, 0);
    }
}

/// Main window handle from the eframe creation context.
pub fn hwnd_of(handle: &impl raw_window_handle::HasWindowHandle) -> Option<HWND> {
    match handle.window_handle().ok()?.as_raw() {
        raw_window_handle::RawWindowHandle::Win32(h) => Some(HWND(h.hwnd.get() as *mut c_void)),
        _ => None,
    }
}

/// Dark or light title bar (Windows 10 1809+).
pub fn set_dark_title_bar(hwnd: HWND, dark: bool) {
    use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
    let value: i32 = dark as i32;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &value as *const i32 as *const c_void,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

/// Starts a program without blocking; console programs get their own console.
pub fn spawn_detached(program: &str, args_raw: &str, dir: Option<&Path>) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x10;
    let mut cmd = std::process::Command::new(program);
    if !args_raw.is_empty() {
        cmd.raw_arg(args_raw);
    }
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    cmd.creation_flags(CREATE_NEW_CONSOLE).spawn().map(|_| ())
}

/// Finds an executable on PATH using PATHEXT, like cmd.exe does.
pub fn find_on_path(name: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let exts: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .filter(|e| !e.is_empty())
        .map(|e| e.to_ascii_lowercase())
        .collect();
    let has_ext = Path::new(name).extension().is_some();
    let try_dir = |dir: &Path| -> Option<PathBuf> {
        let base = dir.join(name);
        if has_ext && base.is_file() {
            return Some(base);
        }
        exts.iter().map(|e| dir.join(format!("{name}{e}"))).find(|p| p.is_file())
    };
    if name.contains('\\') || name.contains('/') {
        let p = Path::new(name);
        if has_ext && p.is_file() {
            return Some(p.to_path_buf());
        }
        return exts.iter().map(|e| PathBuf::from(format!("{name}{e}"))).find(|p| p.is_file());
    }
    if let Some(c) = cwd {
        if let Some(p) = try_dir(c) {
            return Some(p);
        }
    }
    std::env::split_paths(&std::env::var_os("PATH")?).find_map(|d| try_dir(&d))
}

// ---------------------------------------------------------------------------
// Explorer context menu (right click / Shift+F10)

thread_local! {
    static ACTIVE_MENU: std::cell::RefCell<Option<windows::Win32::UI::Shell::IContextMenu>> =
        const { std::cell::RefCell::new(None) };
}

fn forward_menu_msg(msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    use windows::Win32::UI::Shell::{IContextMenu2, IContextMenu3};
    use windows::core::Interface;
    ACTIVE_MENU.with(|m| {
        let m = m.borrow();
        let cm = m.as_ref()?;
        if let Ok(cm3) = cm.cast::<IContextMenu3>() {
            let mut res = LRESULT(0);
            if unsafe { cm3.HandleMenuMsg2(msg, wparam, lparam, Some(&mut res)) }.is_ok() {
                return Some(res);
            }
        } else if let Ok(cm2) = cm.cast::<IContextMenu2>() {
            if unsafe { cm2.HandleMenuMsg(msg, wparam, lparam) }.is_ok() {
                return Some(LRESULT(0));
            }
        }
        None
    })
}

pub enum MenuOutcome {
    Cancelled,
    Invoked,
    /// The user picked "Rename"; the file manager renames inline itself.
    Rename,
}

/// Shows the Explorer context menu for items that share one parent folder.
/// `client_pt` is in physical pixels relative to the window's client area;
/// `None` uses the mouse position.
pub fn shell_context_menu(hwnd: HWND, paths: &[PathBuf], client_pt: Option<(i32, i32)>, extended: bool) -> Result<MenuOutcome, String> {
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{
        CMF_CANRENAME, CMF_EXPLORE, CMF_EXTENDEDVERBS, CMF_NORMAL, CMIC_MASK_PTINVOKE, CMINVOKECOMMANDINFO,
        CMINVOKECOMMANDINFOEX, GCS_VERBW, IContextMenu, IShellFolder, ILFindLastID, ILFree, SHBindToParent,
        SHParseDisplayName,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreatePopupMenu, DestroyMenu, GetCursorPos, SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
    };
    use windows::core::{PCSTR, PSTR};
    const CMIC_MASK_UNICODE: u32 = 0x4000;
    if paths.is_empty() {
        return Ok(MenuOutcome::Cancelled);
    }
    unsafe {
        let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
        for p in paths {
            let w = wide_path(p);
            let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
            if let Err(e) = SHParseDisplayName(pcwstr(&w), None, &mut pidl, 0, None) {
                for q in &pidls {
                    ILFree(Some(*q));
                }
                return Err(format!("{}: {}", p.display(), e.message()));
            }
            pidls.push(pidl);
        }
        let result = (|| -> windows::core::Result<MenuOutcome> {
            let folder: IShellFolder = SHBindToParent(pidls[0], None)?;
            let children: Vec<*const ITEMIDLIST> = pidls.iter().map(|&p| ILFindLastID(p) as *const ITEMIDLIST).collect();
            let cm: IContextMenu = folder.GetUIObjectOf(hwnd, &children, None)?;
            let menu = CreatePopupMenu()?;
            let flags = CMF_NORMAL | CMF_EXPLORE | CMF_CANRENAME | if extended { CMF_EXTENDEDVERBS } else { 0 };
            cm.QueryContextMenu(menu, 0, 1, 0x7FFF, flags).ok()?;
            let mut pt = POINT::default();
            match client_pt {
                Some((x, y)) => {
                    pt = POINT { x, y };
                    let _ = ClientToScreen(hwnd, &mut pt);
                }
                None => GetCursorPos(&mut pt)?,
            }
            ACTIVE_MENU.with(|m| *m.borrow_mut() = Some(cm.clone()));
            let _ = SetForegroundWindow(hwnd);
            let cmd = TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, pt.x, pt.y, hwnd, None).0;
            ACTIVE_MENU.with(|m| *m.borrow_mut() = None);
            let outcome = if cmd > 0 {
                let id = (cmd - 1) as usize;
                let mut verb = [0u16; 64];
                let is_rename = cm.GetCommandString(id, GCS_VERBW, None, PSTR(verb.as_mut_ptr() as *mut u8), verb.len() as u32).is_ok() && {
                    let len = verb.iter().position(|&c| c == 0).unwrap_or(0);
                    String::from_utf16_lossy(&verb[..len]).eq_ignore_ascii_case("rename")
                };
                if is_rename {
                    MenuOutcome::Rename
                } else {
                    let info = CMINVOKECOMMANDINFOEX {
                        cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
                        fMask: CMIC_MASK_UNICODE | CMIC_MASK_PTINVOKE,
                        hwnd,
                        lpVerb: PCSTR(id as *const u8),
                        lpVerbW: PCWSTR(id as *const u16),
                        nShow: SW_SHOWNORMAL.0,
                        ptInvoke: pt,
                        ..Default::default()
                    };
                    cm.InvokeCommand(&info as *const CMINVOKECOMMANDINFOEX as *const CMINVOKECOMMANDINFO)?;
                    MenuOutcome::Invoked
                }
            } else {
                MenuOutcome::Cancelled
            };
            let _ = DestroyMenu(menu);
            Ok(outcome)
        })();
        for p in pidls {
            ILFree(Some(p));
        }
        result.map_err(|e| e.message())
    }
}
