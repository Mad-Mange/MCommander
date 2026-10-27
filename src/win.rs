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
    FOF_SILENT, HDROP, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHDefExtractIconW, SHELLEXECUTEINFOW, SHFILEINFOW,
    SHFILEOPSTRUCTW, SHFileOperationW, SHGFI_ICON, SHGFI_LARGEICON, SHGFI_SMALLICON, SHGFI_TYPENAME, SHGFI_USEFILEATTRIBUTES,
    SHGetFileInfoW, SHOP_FILEPATH, SHObjectProperties, SetWindowSubclass, ShellExecuteExW, ShellExecuteW,
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
    shell_icon(path, is_dir, generic, false)
}

/// Like [`file_icon`], optionally the large (32 px) icon.
pub fn shell_icon(path: &str, is_dir: bool, generic: bool, large: bool) -> Option<IconImage> {
    let w = wide(path);
    let mut info = SHFILEINFOW::default();
    let mut flags = SHGFI_ICON | if large { SHGFI_LARGEICON } else { SHGFI_SMALLICON };
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

/// An icon resource of an exe, dll or ico file at about `size` pixels;
/// `index` as in ExtractIcon (negative numbers are resource ids). Other
/// files get their large shell icon.
pub fn extract_icon(path: &str, index: i32, size: u32) -> Option<IconImage> {
    let w = wide(path);
    let mut icon = HICON::default();
    let hr = unsafe { SHDefExtractIconW(pcwstr(&w), index, 0, Some(&mut icon), None, size) };
    if hr == windows::Win32::Foundation::S_OK && !icon.is_invalid() {
        let img = icon_to_rgba(icon);
        unsafe {
            let _ = DestroyIcon(icon);
        }
        return img;
    }
    shell_icon(path, Path::new(path).is_dir(), false, true)
}

/// Number of icons in an exe, dll or ico file.
pub fn icon_count(path: &str) -> u32 {
    let w = wide(path);
    unsafe { windows::Win32::UI::Shell::ExtractIconExW(pcwstr(&w), -1, None, None, 0) }
}

/// Starts a program or opens a document through the shell (`verb` "open"
/// or "runas" for administrator rights). `show` is a SW_ value. Returns the
/// process handle when there is one; pass it to [`delete_when_done`].
pub fn shell_execute_ex(hwnd: Option<HWND>, verb: &str, file: &str, params: &str, dir: Option<&Path>, show: i32) -> Result<Option<isize>, String> {
    let (verb_w, file_w, params_w) = (wide(verb), wide(file), wide(params));
    let dir_w = dir.map(wide_path);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        // NOASYNC: otherwise the shell may finish the start asynchronously
        // through this thread's message loop, which froze the window when a
        // program was started right after a dialog closed.
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC,
        hwnd: hwnd.unwrap_or_default(),
        lpVerb: pcwstr(&verb_w),
        lpFile: pcwstr(&file_w),
        lpParameters: if params.is_empty() { PCWSTR::null() } else { pcwstr(&params_w) },
        lpDirectory: dir_w.as_ref().map(|d| pcwstr(d)).unwrap_or(PCWSTR::null()),
        nShow: show,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| e.message())?;
    Ok((!info.hProcess.is_invalid()).then_some(info.hProcess.0 as isize))
}

/// Deletes a temporary file once the process ends (or after a minute when
/// there is no process to wait for), and closes the process handle.
pub fn delete_when_done(process: Option<isize>, file: PathBuf) {
    std::thread::spawn(move || {
        use windows::Win32::System::Threading::{INFINITE, WaitForSingleObject};
        match process {
            Some(h) => unsafe {
                let handle = HANDLE(h as *mut c_void);
                WaitForSingleObject(handle, INFINITE);
                let _ = windows::Win32::Foundation::CloseHandle(handle);
            },
            None => std::thread::sleep(std::time::Duration::from_secs(60)),
        }
        let _ = std::fs::remove_file(file);
    });
}

/// Closes a process handle from [`shell_execute_ex`] that isn't needed.
pub fn close_process(process: Option<isize>) {
    if let Some(h) = process {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(HANDLE(h as *mut c_void));
        }
    }
}

/// Pixels of a GDI bitmap as top-down BGRA.
fn bitmap_bgra(hbm: windows::Win32::Graphics::Gdi::HBITMAP) -> Option<(usize, usize, Vec<u8>)> {
    unsafe {
        let mut bm = BITMAP::default();
        if GetObjectW(HGDIOBJ(hbm.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void)) == 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth as usize, bm.bmHeight.unsigned_abs() as usize);
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
        let lines = GetDIBits(dc, hbm, 0, h as u32, Some(bgra.as_mut_ptr() as *mut c_void), &mut bmi, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        (lines > 0).then_some((w, h, bgra))
    }
}

/// Explorer's thumbnail for a file or folder (pictures, videos, documents),
/// falling back to its large icon, scaled to fit `size` pixels.
pub fn shell_thumbnail(path: &Path, size: i32) -> Option<IconImage> {
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::System::Com::IBindCtx;
    use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_RESIZETOFIT};
    let w = wide_path(path);
    unsafe {
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(pcwstr(&w), None::<&IBindCtx>).ok()?;
        let hbm = factory.GetImage(SIZE { cx: size, cy: size }, SIIGBF_RESIZETOFIT).ok()?;
        let pixels = bitmap_bgra(hbm);
        let _ = DeleteObject(HGDIOBJ(hbm.0));
        let (w, h, bgra) = pixels?;
        // 32-bit shell bitmaps are premultiplied; all-zero alpha means opaque.
        let has_alpha = bgra.chunks_exact(4).any(|p| p[3] != 0);
        let mut rgba = Vec::with_capacity(w * h * 4);
        for p in bgra.chunks_exact(4) {
            let a = if has_alpha { p[3] } else { 255 };
            let un = |c: u8| if a == 0 || a == 255 { c } else { ((c as u32 * 255) / a as u32).min(255) as u8 };
            rgba.extend_from_slice(&[un(p[2]), un(p[1]), un(p[0]), a]);
        }
        Some(IconImage { width: w, height: h, rgba })
    }
}

fn icon_to_rgba(icon: HICON) -> Option<IconImage> {
    unsafe {
        let mut ii = ICONINFO::default();
        GetIconInfo(icon, &mut ii).ok()?;
        let color = (!ii.hbmColor.is_invalid()).then(|| bitmap_bgra(ii.hbmColor)).flatten();
        let mask = (!ii.hbmMask.is_invalid()).then(|| bitmap_bgra(ii.hbmMask)).flatten();
        for hbm in [ii.hbmColor, ii.hbmMask] {
            if !hbm.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(hbm.0));
            }
        }
        let (w, h, bgra) = color?;
        // Monochrome masks of colour icons have the colour bitmap's size.
        let mask = mask.filter(|(mw, mh, _)| *mw == w && *mh >= h).map(|(_, _, m)| m);
        let has_mask = mask.is_some();
        let mask = mask.unwrap_or_default();
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

/// Sets the title bar of this process's top-level window with the given title
/// (child viewports don't expose their handle). Other programs, Total
/// Commander among them, can have windows with the same title.
pub fn set_dark_title_bar_by_title(title: &str, dark: bool) {
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetWindowThreadProcessId};
    let title = wide(title);
    let mut after = None;
    unsafe {
        while let Ok(hwnd) = FindWindowExW(None, after, PCWSTR::null(), pcwstr(&title)) {
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == std::process::id() {
                set_dark_title_bar(hwnd, dark);
                return;
            }
            after = Some(hwnd);
        }
    }
}

/// Starts a program without blocking; console programs get their own console.
pub fn spawn_detached(program: &str, args_raw: &str, dir: Option<&Path>) -> std::io::Result<()> {
    const CREATE_NEW_CONSOLE: u32 = 0x10;
    spawn_with_flags(program, args_raw, dir, CREATE_NEW_CONSOLE)
}

/// Like [`spawn_detached`], but a console program gets no window.
pub fn spawn_hidden(program: &str, args_raw: &str, dir: Option<&Path>) -> std::io::Result<()> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    spawn_with_flags(program, args_raw, dir, CREATE_NO_WINDOW)
}

fn spawn_with_flags(program: &str, args_raw: &str, dir: Option<&Path>, flags: u32) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    let mut cmd = std::process::Command::new(program);
    if !args_raw.is_empty() {
        cmd.raw_arg(args_raw);
    }
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    cmd.creation_flags(flags).spawn().map(|_| ())
}

/// Whether an executable is a Windows (GUI) program rather than a console
/// program, from the subsystem field of its PE header.
pub fn exe_is_gui(path: &Path) -> Option<bool> {
    use std::io::Read;
    let mut head = vec![0u8; 4096];
    let n = std::fs::File::open(path).ok()?.read(&mut head).ok()?;
    head.truncate(n);
    let u32_at = |o: usize| head.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if head.get(..2)? != b"MZ" {
        return None;
    }
    let pe = u32_at(0x3C)? as usize;
    if head.get(pe..pe + 4)? != b"PE\0\0" {
        return None;
    }
    // The optional header follows the 20-byte file header; Subsystem is at
    // offset 68 in both its 32- and 64-bit layouts.
    let sub = head.get(pe + 24 + 68..pe + 24 + 70)?;
    match u16::from_le_bytes([sub[0], sub[1]]) {
        2 => Some(true),
        3 => Some(false),
        _ => None,
    }
}

/// Whether a batch file only starts Windows programs, like VS Code's
/// `code.cmd`. Total Commander opens such commands without a console window
/// showing up. Anything that runs a console program (`node`, `python`, an
/// unknown `%var%`) or waits for input keeps its console.
pub fn batch_starts_gui_only(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path) else { return false };
    if bytes.len() > 64 * 1024 {
        return false;
    }
    let text = String::from_utf8_lossy(&bytes);
    let dir = path.parent().map(|d| format!("{}\\", d.display())).unwrap_or_default();
    let mut found_gui = false;
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('@').trim();
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("rem ") || lower.starts_with("::") {
            continue;
        }
        if lower.starts_with("pause") || lower.contains("set /p") || lower.starts_with("choice") {
            return false;
        }
        for exe in exe_tokens(line) {
            let resolved = exe.replace("%~dp0", &dir).replace("%~DP0", &dir);
            if resolved.contains('%') || resolved.contains('!') {
                return false;
            }
            let p = if resolved.contains('\\') || resolved.contains('/') { Some(PathBuf::from(&resolved)) } else { find_on_path(&resolved, None) };
            match p.and_then(|p| exe_is_gui(&p)) {
                Some(true) => found_gui = true,
                _ => return false,
            }
        }
    }
    found_gui
}

/// The `.exe` names mentioned on a batch file line, quoted or bare.
fn exe_tokens(line: &str) -> Vec<String> {
    let lower = line.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = lower[from..].find(".exe") {
        let end = from + i + 4;
        let at_boundary = lower[end..].chars().next().is_none_or(|c| c == '"' || c.is_whitespace());
        if at_boundary {
            let before = &line[..from + i];
            let start = match before.rfind('"') {
                // Inside quotes: the token starts after the opening quote.
                Some(q) if before[..q].matches('"').count() % 2 == 0 => q + 1,
                _ => before.rfind(|c: char| c.is_whitespace() || c == '=' || c == '(').map_or(0, |j| j + 1),
            };
            // `set "var=C:\x.exe"` quotes the whole assignment.
            let token = &line[start..end];
            out.push(token.rsplit_once('=').map_or(token, |(_, path)| path).to_string());
        }
        from = end;
    }
    out
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

/// Resolves files of one folder to shell items and calls `f` with the
/// parent folder and the child item ids (freed afterwards).
fn with_shell_items<R>(
    paths: &[PathBuf],
    f: impl FnOnce(&windows::Win32::UI::Shell::IShellFolder, &[*const windows::Win32::UI::Shell::Common::ITEMIDLIST]) -> windows::core::Result<R>,
) -> Result<R, String> {
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{ILFindLastID, ILFree, IShellFolder, SHBindToParent, SHParseDisplayName};
    if paths.is_empty() {
        return Err("nothing selected".into());
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
        let result = (|| {
            let folder: IShellFolder = SHBindToParent(pidls[0], None)?;
            let children: Vec<*const ITEMIDLIST> = pidls.iter().map(|&p| ILFindLastID(p) as *const ITEMIDLIST).collect();
            f(&folder, &children)
        })();
        for p in pidls {
            ILFree(Some(p));
        }
        result.map_err(|e| e.message())
    }
}

/// True when the mouse cursor is outside the window (drag leaving the app).
pub fn cursor_outside(hwnd: HWND) -> bool {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect};
    let mut pt = POINT::default();
    let mut r = RECT::default();
    unsafe {
        if GetCursorPos(&mut pt).is_err() || GetWindowRect(hwnd, &mut r).is_err() {
            return false;
        }
    }
    pt.x < r.left || pt.x >= r.right || pt.y < r.top || pt.y >= r.bottom
}

/// Drags files out of the window with OLE (to Explorer, browsers, editors).
/// Blocks until the drop; returns the effect the target chose (1 copy, 2 move, 4 link, 0 none).
pub fn drag_files_out(hwnd: HWND, paths: &[PathBuf]) -> Result<u32, String> {
    use windows::Win32::System::Com::IDataObject;
    use windows::Win32::System::Ole::{DROPEFFECT, IDropSource};
    use windows::Win32::UI::Shell::SHDoDragDrop;
    with_shell_items(paths, |folder, children| unsafe {
        let data: IDataObject = folder.GetUIObjectOf(hwnd, children, None)?;
        // Copy | move | link: the drop target picks, as with Explorer.
        let effect = SHDoDragDrop(Some(hwnd), &data, None::<&IDropSource>, DROPEFFECT(1 | 2 | 4))?;
        Ok(effect.0)
    })
}

/// Shows the Explorer context menu for items that share one parent folder.
/// `client_pt` is in physical pixels relative to the window's client area;
/// `None` uses the mouse position.
pub fn shell_context_menu(hwnd: HWND, paths: &[PathBuf], client_pt: Option<(i32, i32)>, extended: bool) -> Result<MenuOutcome, String> {
    use windows::Win32::UI::Shell::IContextMenu;
    if paths.is_empty() {
        return Ok(MenuOutcome::Cancelled);
    }
    with_shell_items(paths, |folder, children| unsafe {
        let cm: IContextMenu = folder.GetUIObjectOf(hwnd, children, None)?;
        track_shell_menu(hwnd, &cm, client_pt, extended)
    })
}

/// Shows Explorer's menu for the empty area of a folder (View, Paste,
/// New > Folder / Text Document / ..., Properties and what other programs add).
pub fn shell_background_menu(hwnd: HWND, dir: &Path, client_pt: Option<(i32, i32)>, extended: bool) -> Result<MenuOutcome, String> {
    let cm = folder_background_menu(hwnd, dir)?;
    unsafe { track_shell_menu(hwnd, &cm, client_pt, extended) }.map_err(|e| e.message())
}

/// The background context menu of `dir`, from the folder's own view object.
fn folder_background_menu(hwnd: HWND, dir: &Path) -> Result<windows::Win32::UI::Shell::IContextMenu, String> {
    use windows::Win32::UI::Shell::Common::ITEMIDLIST;
    use windows::Win32::UI::Shell::{ILFree, IShellFolder, SHBindToObject, SHParseDisplayName};
    unsafe {
        let w = wide_path(dir);
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        SHParseDisplayName(pcwstr(&w), None, &mut pidl, 0, None).map_err(|e| format!("{}: {}", dir.display(), e.message()))?;
        let result = (|| {
            let folder: IShellFolder = SHBindToObject(None, pidl, None)?;
            folder.CreateViewObject(hwnd)
        })();
        ILFree(Some(pidl));
        result.map_err(|e| e.message())
    }
}

/// Fills a popup menu from `cm`, tracks it at `client_pt` (or the mouse) and
/// runs the chosen command.
unsafe fn track_shell_menu(hwnd: HWND, cm: &windows::Win32::UI::Shell::IContextMenu, client_pt: Option<(i32, i32)>, extended: bool) -> windows::core::Result<MenuOutcome> {
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::UI::Shell::{
        CMF_CANRENAME, CMF_EXPLORE, CMF_EXTENDEDVERBS, CMF_NORMAL, CMIC_MASK_PTINVOKE, CMINVOKECOMMANDINFO, CMINVOKECOMMANDINFOEX,
        GCS_VERBW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreatePopupMenu, DestroyMenu, GetCursorPos, SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
    };
    use windows::core::{PCSTR, PSTR};
    const CMIC_MASK_UNICODE: u32 = 0x4000;
    unsafe {
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
        // Submenus such as "New" fill themselves through the menu messages
        // the window procedure forwards while this menu is open.
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_exe_names_on_batch_lines() {
        assert_eq!(exe_tokens(r#""%~dp0..\Code.exe" "%~dp0..\res\cli.js" %*"#), vec![r"%~dp0..\Code.exe"]);
        assert_eq!(exe_tokens(r#"IF EXIST "%dp0%\node.exe" ("#), vec![r"%dp0%\node.exe"]);
        assert_eq!(exe_tokens(r"start notepad.exe readme.txt"), vec!["notepad.exe"]);
        assert_eq!(exe_tokens(r#"set "_prog=C:\Tools\a b\x.exe""#), vec![r"C:\Tools\a b\x.exe"]);
        assert!(exe_tokens("echo foo.exec").is_empty());
    }

    #[test]
    fn gui_launcher_batch_files_run_without_console() {
        let sys = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join("System32");
        assert_eq!(exe_is_gui(&sys.join("notepad.exe")), Some(true));
        assert_eq!(exe_is_gui(&sys.join("cmd.exe")), Some(false));
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, text: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, text).unwrap();
            p
        };
        // Shaped like VS Code's code.cmd, with notepad standing in for Code.exe.
        let code = write(
            "code.cmd",
            &format!("@echo off\r\nsetlocal\r\nset ELECTRON_RUN_AS_NODE=1\r\n\"{}\" \"%~dp0cli.js\" %*\r\nendlocal\r\n", sys.join("notepad.exe").display()),
        );
        assert!(batch_starts_gui_only(&code));
        std::fs::copy(sys.join("notepad.exe"), dir.path().join("gui.exe")).unwrap();
        assert!(batch_starts_gui_only(&write("rel.cmd", "@\"%~dp0gui.exe\" %*\r\n")));
        // npm shims (codex.cmd) run node through a variable: keep the console.
        assert!(!batch_starts_gui_only(&write("codex.cmd", "@ECHO off\r\nIF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n)\r\n")));
        assert!(!batch_starts_gui_only(&write("console.cmd", &format!("\"{}\" /c dir\r\n", sys.join("cmd.exe").display()))));
        assert!(!batch_starts_gui_only(&write("wait.cmd", &format!("\"{}\"\r\npause\r\n", sys.join("notepad.exe").display()))));
        assert!(!batch_starts_gui_only(&write("plain.bat", "echo hello\r\n")));
    }

    #[test]
    fn shell_data_object_offers_file_drop_format() {
        use windows::Win32::System::Com::{
            COINIT_APARTMENTTHREADED, CoInitializeEx, DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL,
        };
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<PathBuf> = ["a.txt", "b.txt"].iter().map(|n| dir.path().join(n)).collect();
        for f in &files {
            std::fs::write(f, b"x").unwrap();
        }
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let ok = with_shell_items(&files, |folder, children| unsafe {
            let data: IDataObject = folder.GetUIObjectOf(HWND::default(), children, None)?;
            let fmt = FORMATETC {
                cfFormat: CF_HDROP.0,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as u32,
            };
            Ok(data.QueryGetData(&fmt).is_ok())
        })
        .unwrap();
        assert!(ok, "shell data object should offer CF_HDROP");
        assert!(with_shell_items(&[], |_, _| Ok(())).is_err());
    }

    #[test]
    fn folder_background_menu_creates_new_items() {
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
        use windows::Win32::UI::Shell::{CMF_EXPLORE, CMF_NORMAL, CMINVOKECOMMANDINFO, GCS_VERBW, IContextMenu3};
        use windows::Win32::UI::WindowsAndMessaging::{CreatePopupMenu, DestroyMenu, GetMenuItemCount, GetMenuItemID, GetSubMenu};
        use windows::core::{Interface, PCSTR, PSTR};
        let dir = tempfile::tempdir().unwrap();
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let cm = folder_background_menu(HWND::default(), dir.path()).unwrap();
        unsafe {
            let menu = CreatePopupMenu().unwrap();
            cm.QueryContextMenu(menu, 0, 1, 0x7FFF, CMF_NORMAL | CMF_EXPLORE).ok().unwrap();
            // Command ids of every submenu item by verb ("NewFolder", ".txt", ...).
            let mut verbs = Vec::new();
            for i in 0..GetMenuItemCount(Some(menu)) {
                let sub = GetSubMenu(menu, i);
                if sub.is_invalid() {
                    continue;
                }
                // Shell submenus are filled when they open (WM_INITMENUPOPUP).
                if let Ok(cm3) = cm.cast::<IContextMenu3>() {
                    let _ = cm3.HandleMenuMsg2(0x0117, WPARAM(sub.0 as usize), LPARAM(i as isize), None);
                }
                for j in 0..GetMenuItemCount(Some(sub)) {
                    let id = GetMenuItemID(sub, j);
                    if id == 0 || id == u32::MAX {
                        continue; // separator or nested submenu
                    }
                    let mut verb = [0u16; 64];
                    if cm.GetCommandString(id as usize - 1, GCS_VERBW, None, PSTR(verb.as_mut_ptr() as *mut u8), verb.len() as u32).is_ok() {
                        let len = verb.iter().position(|&c| c == 0).unwrap_or(0);
                        verbs.push((String::from_utf16_lossy(&verb[..len]), id));
                    }
                }
            }
            let new_folder = verbs.iter().find(|(v, _)| v.eq_ignore_ascii_case("NewFolder")).map(|&(_, id)| id);
            let Some(id) = new_folder else {
                let _ = DestroyMenu(menu);
                panic!("no New > Folder in the background menu: {verbs:?}");
            };
            // The item is created before InvokeCommand returns, so the panel
            // can be re-read right away to rename it.
            let info = CMINVOKECOMMANDINFO {
                cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
                lpVerb: PCSTR((id - 1) as usize as *const u8),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            cm.InvokeCommand(&info).unwrap();
            let _ = DestroyMenu(menu);
            let made: Vec<PathBuf> = std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.path()).collect();
            assert!(made.len() == 1 && made[0].is_dir(), "{made:?}");
        }
    }
}
