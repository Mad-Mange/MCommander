//! Shell icon cache. Icons are extracted on a background thread (SHGetFileInfo
//! can be slow for executables and network paths) and uploaded as textures.

use crate::vfs::FileEntry;
use egui::{ColorImage, TextureHandle, TextureOptions};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};

/// Extensions whose icon is embedded in the file itself.
const PER_FILE: [&str; 9] = ["exe", "ico", "lnk", "url", "cur", "ani", "scr", "msc", "appref-ms"];

struct Request {
    key: String,
    path: String,
    is_dir: bool,
    generic: bool,
}

pub struct IconCache {
    textures: HashMap<String, Option<TextureHandle>>,
    pending: HashSet<String>,
    tx: Sender<Request>,
    rx: Receiver<(String, Option<crate::win::IconImage>)>,
}

impl IconCache {
    pub fn new(ctx: egui::Context) -> IconCache {
        let (tx, req_rx) = channel::<Request>();
        let (res_tx, rx) = channel();
        std::thread::Builder::new()
            .name("icon-loader".into())
            .spawn(move || {
                // Shell icon extraction requires COM on this thread.
                unsafe {
                    let _ = windows::Win32::System::Com::CoInitializeEx(
                        None,
                        windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
                    );
                }
                while let Ok(r) = req_rx.recv() {
                    let img = crate::win::file_icon(&r.path, r.is_dir, r.generic);
                    if res_tx.send((r.key, img)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawn icon thread");
        IconCache { textures: HashMap::new(), pending: HashSet::new(), tx, rx }
    }

    /// Uploads finished icons; call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((key, img)) = self.rx.try_recv() {
            self.pending.remove(&key);
            let tex = img.map(|i| {
                let image = ColorImage::from_rgba_unmultiplied([i.width, i.height], &i.rgba);
                ctx.load_texture(format!("icon:{key}"), image, TextureOptions::LINEAR)
            });
            self.textures.insert(key, tex);
        }
    }

    fn request(&mut self, key: String, path: String, is_dir: bool, generic: bool) -> Option<egui::TextureId> {
        if let Some(t) = self.textures.get(&key) {
            return t.as_ref().map(|t| t.id());
        }
        if self.pending.insert(key.clone()) {
            let _ = self.tx.send(Request { key, path, is_dir, generic });
        }
        None
    }

    /// Icon for a file list entry; `None` while loading or when unavailable.
    pub fn for_entry(&mut self, entry: &FileEntry, full: Option<&Path>) -> Option<egui::TextureId> {
        if entry.is_dir {
            // Folder icons are the same for every plain folder.
            return self.request("<dir>".into(), "folder".into(), true, true);
        }
        let ext = entry.ext().to_ascii_lowercase();
        if let Some(full) = full.filter(|_| PER_FILE.contains(&ext.as_str())) {
            let p = full.to_string_lossy().into_owned();
            let generic_id = self.request(format!(".{ext}"), format!("x.{ext}"), false, true);
            return self.request(format!("file:{p}"), p, false, false).or(generic_id);
        }
        self.request(format!(".{ext}"), format!("x.{ext}"), false, true)
    }

    pub fn for_drive(&mut self, letter: char) -> Option<egui::TextureId> {
        let root = format!("{letter}:\\");
        self.request(format!("drive:{letter}"), root, true, false)
    }
}
