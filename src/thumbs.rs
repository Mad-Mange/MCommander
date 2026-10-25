//! Thumbnail cache for the thumbnails view (Ctrl+Shift+F1). Thumbnails come
//! from Explorer's thumbnail providers on a worker thread; the most recently
//! requested (= visible) items are produced first.

use egui::{ColorImage, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::SystemTime;

/// A file and its modification time: a changed file gets a new thumbnail.
type Key = (PathBuf, Option<SystemTime>);

/// Thumbnail edge in physical pixels.
pub const THUMB_PX: i32 = 192;
/// At most this many requests wait; older ones (scrolled away) are dropped.
const MAX_QUEUE: usize = 256;
/// Textures kept before least-recently used ones are released.
const MAX_TEXTURES: usize = 600;

struct Entry {
    texture: Option<TextureHandle>,
    last_used: u64,
}

#[derive(Default)]
struct Queue {
    stack: Vec<Key>,
}

pub struct ThumbCache {
    entries: HashMap<Key, Entry>,
    pending: HashMap<Key, u64>,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    rx: Receiver<(Key, Option<crate::win::IconImage>)>,
    frame: u64,
}

impl ThumbCache {
    pub fn new(ctx: egui::Context) -> ThumbCache {
        let queue: Arc<(Mutex<Queue>, Condvar)> = Arc::default();
        let (tx, rx) = channel();
        let q = queue.clone();
        std::thread::Builder::new()
            .name("thumbnails".into())
            .spawn(move || {
                unsafe {
                    let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
                }
                loop {
                    let key = {
                        let (lock, cv) = &*q;
                        let mut guard = lock.lock().unwrap();
                        loop {
                            if let Some(p) = guard.stack.pop() {
                                break p;
                            }
                            guard = cv.wait(guard).unwrap();
                        }
                    };
                    let img = crate::win::shell_thumbnail(&key.0, THUMB_PX);
                    if tx.send((key, img)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawn thumbnail thread");
        ThumbCache { entries: HashMap::new(), pending: HashMap::new(), queue, rx, frame: 0 }
    }

    /// Uploads finished thumbnails and releases old ones; call once per frame.
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        while let Ok((key, img)) = self.rx.try_recv() {
            self.pending.remove(&key);
            let texture = img.map(|i| {
                let image = ColorImage::from_rgba_unmultiplied([i.width, i.height], &i.rgba);
                ctx.load_texture(format!("thumb:{}", key.0.display()), image, TextureOptions::LINEAR)
            });
            self.entries.insert(key, Entry { texture, last_used: self.frame });
        }
        if self.entries.len() > MAX_TEXTURES {
            let mut ages: Vec<u64> = self.entries.values().map(|e| e.last_used).collect();
            ages.sort_unstable();
            let cutoff = ages[ages.len() - MAX_TEXTURES / 2];
            self.entries.retain(|_, e| e.last_used >= cutoff);
        }
    }

    /// The thumbnail for `path` with its pixel size, or `None` while loading.
    pub fn get(&mut self, path: &Path, modified: Option<SystemTime>) -> Option<(egui::TextureId, egui::Vec2)> {
        let key: Key = (path.to_path_buf(), modified);
        if let Some(e) = self.entries.get_mut(&key) {
            e.last_used = self.frame;
            return e.texture.as_ref().map(|t| (t.id(), t.size_vec2()));
        }
        // (Re-)queue on top so visible items are produced first.
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().unwrap();
        if self.pending.get(&key) != Some(&self.frame) {
            q.stack.retain(|k| *k != key);
            q.stack.push(key.clone());
            self.pending.insert(key, self.frame);
            if q.stack.len() > MAX_QUEUE {
                let drop_n = q.stack.len() - MAX_QUEUE;
                for p in q.stack.drain(..drop_n) {
                    self.pending.remove(&p);
                }
            }
            cv.notify_one();
        }
        None
    }
}
