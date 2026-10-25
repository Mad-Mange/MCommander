//! Lister (F3): a separate window that shows a file as text, hex or image.

use crate::theme::Palette;
use egui::{Align2, Color32, FontId, Key, Rect, Sense, Stroke, ViewportBuilder, ViewportId, pos2, vec2};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const TEXT_CAP: u64 = 32 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Text,
    Hex,
    Image,
}

pub fn is_image(p: &Path) -> bool {
    let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "bmp" | "ico" | "webp" | "tif" | "tiff")
}

pub fn read_head(p: &Path, n: u64) -> std::io::Result<Vec<u8>> {
    let mut f = std::fs::File::open(p)?;
    let mut buf = Vec::new();
    f.by_ref().take(n).read_to_end(&mut buf)?;
    Ok(buf)
}

pub fn looks_binary(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) {
        return false;
    }
    let sample = &bytes[..bytes.len().min(8192)];
    let ctrl = sample.iter().filter(|&&b| b == 0 || (b < 9) || (b > 13 && b < 32 && b != 27)).count();
    ctrl * 20 > sample.len().max(1)
}

/// Decodes text with BOM detection; UTF-8 if valid, else Windows-1252.
pub fn decode(bytes: &[u8]) -> String {
    crate::ini::decode_text(bytes).replace("\r\n", "\n").replace('\r', "\n")
}

fn encoding_name(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        "UTF-16 LE"
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        "UTF-16 BE"
    } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        "UTF-8 (BOM)"
    } else if std::str::from_utf8(bytes).is_ok() {
        "UTF-8"
    } else {
        "ANSI (Windows-1252)"
    }
}

/// A Lister window. Its state is shared with a deferred viewport so the
/// window repaints independently of the main window.
pub struct ViewerWindow {
    id: ViewportId,
    state: Arc<Mutex<Viewer>>,
}

impl ViewerWindow {
    pub fn open(id: u64, path: PathBuf, pal: &Palette) -> ViewerWindow {
        let viewer = Viewer::new(ViewportId::from_hash_of(("lister", id)), path, pal.clone());
        ViewerWindow { id: viewer.id, state: Arc::new(Mutex::new(viewer)) }
    }

    pub fn viewport_id(&self) -> ViewportId {
        self.id
    }

    /// Keeps the window alive for this frame. Returns false once it was closed.
    pub fn show(&self, ctx: &egui::Context, pal: &Palette) -> bool {
        let title = {
            let mut v = self.state.lock().unwrap();
            if v.closed {
                return false;
            }
            v.pal = pal.clone();
            format!("Lister - [{}]", v.path.display())
        };
        let builder = ViewportBuilder::default().with_title(title).with_inner_size([960.0, 720.0]).with_min_inner_size([320.0, 200.0]);
        let state = self.state.clone();
        ctx.show_viewport_deferred(self.id, builder, move |ui, _class| {
            state.lock().unwrap().ui(ui);
        });
        true
    }
}

pub struct Viewer {
    id: ViewportId,
    pal: Palette,
    closed: bool,
    path: PathBuf,
    mode: Mode,
    size: u64,
    head: Vec<u8>,
    truncated: bool,
    lines: Vec<String>,
    encoding: &'static str,
    wrap: bool,
    /// Wrapped display lines: (line index, char start, char end).
    visual: Vec<(usize, usize, usize)>,
    visual_cols: usize,
    top: usize,
    sel: Option<(usize, usize)>,
    image: Option<Result<(egui::TextureHandle, [usize; 2]), String>>,
    fit: bool,
    find: String,
    find_open: bool,
    find_focus: bool,
    found: Option<(usize, usize)>,
    message: String,
    file: Option<std::fs::File>,
    hex_cache: (u64, Vec<u8>),
}

impl Viewer {
    fn new(id: ViewportId, path: PathBuf, pal: Palette) -> Viewer {
        let mut v = Viewer {
            id,
            pal,
            closed: false,
            path,
            mode: Mode::Text,
            size: 0,
            head: Vec::new(),
            truncated: false,
            lines: Vec::new(),
            encoding: "",
            wrap: true,
            visual: Vec::new(),
            visual_cols: 0,
            top: 0,
            sel: None,
            image: None,
            fit: true,
            find: String::new(),
            find_open: false,
            find_focus: false,
            found: None,
            message: String::new(),
            file: None,
            hex_cache: (u64::MAX, Vec::new()),
        };
        v.load();
        v
    }

    fn load(&mut self) {
        self.size = std::fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        self.head = read_head(&self.path, TEXT_CAP).unwrap_or_default();
        self.truncated = self.size > self.head.len() as u64;
        self.file = std::fs::File::open(&self.path).ok();
        self.hex_cache = (u64::MAX, Vec::new());
        self.image = None;
        self.top = 0;
        self.sel = None;
        self.found = None;
        self.mode = if is_image(&self.path) {
            Mode::Image
        } else if looks_binary(&self.head) {
            Mode::Hex
        } else {
            Mode::Text
        };
        self.decode_text();
    }

    fn decode_text(&mut self) {
        self.encoding = encoding_name(&self.head);
        let text = decode(&self.head);
        self.lines = text.split('\n').map(|l| l.replace('\t', "    ")).collect();
        self.visual_cols = 0;
    }

    fn next_file(&mut self, delta: isize) {
        let Some(dir) = self.path.parent() else { return };
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
        files.sort_by(|a, b| crate::format::natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
        if let Some(i) = files.iter().position(|p| *p == self.path) {
            let j = i as isize + delta;
            if j >= 0 && (j as usize) < files.len() {
                self.path = files[j as usize].clone();
                self.load();
            }
        }
    }

    fn rewrap(&mut self, cols: usize) {
        if cols == self.visual_cols && !self.visual.is_empty() {
            return;
        }
        self.visual_cols = cols;
        self.visual.clear();
        for (i, l) in self.lines.iter().enumerate() {
            let n = l.chars().count();
            if !self.wrap || n <= cols || cols == 0 {
                self.visual.push((i, 0, n));
            } else {
                let mut s = 0;
                while s < n {
                    self.visual.push((i, s, (s + cols).min(n)));
                    s += cols;
                }
            }
        }
    }

    fn hex_bytes(&mut self, offset: u64, len: usize) -> Vec<u8> {
        if offset + len as u64 <= self.head.len() as u64 {
            return self.head[offset as usize..offset as usize + len].to_vec();
        }
        let block = offset / 65536 * 65536;
        if self.hex_cache.0 != block {
            let mut buf = vec![0u8; 65536 + 4096];
            let n = self
                .file
                .as_mut()
                .and_then(|f| f.seek(SeekFrom::Start(block)).ok().and_then(|_| f.read(&mut buf).ok()))
                .unwrap_or(0);
            buf.truncate(n);
            self.hex_cache = (block, buf);
        }
        let start = (offset - block) as usize;
        let data = &self.hex_cache.1;
        if start >= data.len() {
            return Vec::new();
        }
        data[start..(start + len).min(data.len())].to_vec()
    }

    fn find_next(&mut self, backwards: bool) {
        if self.find.is_empty() {
            return;
        }
        let needle = self.find.to_lowercase();
        let start = self.found.map(|(l, _)| l).unwrap_or(self.top.min(self.lines.len().saturating_sub(1)));
        let n = self.lines.len();
        for k in 1..=n {
            let i = if backwards { (start + n - k % n) % n } else { (start + k) % n };
            let i = if self.found.is_none() && k == 1 && !backwards { start } else { i };
            if let Some(pos) = self.lines[i].to_lowercase().find(&needle) {
                let col = self.lines[i][..pos.min(self.lines[i].len())].chars().count();
                self.found = Some((i, col));
                self.sel = Some((i, i));
                if let Some(v) = self.visual.iter().position(|&(li, s, e)| li == i && col >= s && col <= e.max(s)) {
                    self.top = v.saturating_sub(5);
                }
                self.message.clear();
                return;
            }
        }
        self.message = format!("\"{}\" not found", self.find);
    }

    fn close(&mut self, ctx: &egui::Context) {
        self.closed = true;
        // The main window drops the viewport on its next frame.
        ctx.request_repaint_of(ViewportId::ROOT);
    }

    /// Draws the viewer window contents (deferred viewport callback).
    fn ui(&mut self, ui: &mut egui::Ui) {
        let pal = &self.pal.clone();
        {
            let ctx = ui.ctx().clone();
            if ctx.input(|i| i.viewport().close_requested()) {
                self.close(&ctx);
                return;
            }
            self.handle_keys(&ctx);
            if self.closed {
                return;
            }
            egui::Panel::top("lister-bar")
                .frame(egui::Frame::new().fill(pal.bar).inner_margin(egui::Margin::symmetric(6, 4)))
                .show(ui, |ui| self.top_bar(ui));
            egui::Panel::bottom("lister-status")
                .frame(egui::Frame::new().fill(pal.face).inner_margin(egui::Margin::symmetric(6, 3)))
                .show(ui, |ui| self.status(ui));
            if self.find_open {
                egui::Panel::bottom("lister-find")
                    .frame(egui::Frame::new().fill(pal.face).inner_margin(egui::Margin::symmetric(6, 4)))
                    .show(ui, |ui| self.find_bar(ui));
            }
            egui::CentralPanel::no_frame().show(ui, |ui| {
                let rect = ui.max_rect();
                ui.painter().rect_filled(rect, 0.0, pal.list_bg);
                match self.mode {
                    Mode::Text => self.draw_text(ui, rect, pal),
                    Mode::Hex => self.draw_hex(ui, rect, pal),
                    Mode::Image => self.draw_image(ui, rect, pal),
                }
            });
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if self.find_focus {
            return;
        }
        let typing = ctx.text_edit_focused();
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
        let mods = ctx.input(|i| i.modifiers);
        if pressed(Key::Escape) {
            if self.find_open {
                self.find_open = false;
            } else {
                self.close(ctx);
            }
        }
        if typing {
            return;
        }
        if pressed(Key::Num1) {
            self.mode = Mode::Text;
        }
        if pressed(Key::Num3) {
            self.mode = Mode::Hex;
        }
        if pressed(Key::Num4) && is_image(&self.path) {
            self.mode = Mode::Image;
        }
        if pressed(Key::W) && !mods.ctrl {
            self.wrap = !self.wrap;
            self.visual_cols = 0;
        }
        if pressed(Key::N) && !mods.ctrl {
            self.next_file(1);
        }
        if pressed(Key::P) && !mods.ctrl {
            self.next_file(-1);
        }
        if pressed(Key::F7) || (mods.ctrl && pressed(Key::F)) {
            self.find_open = true;
            self.find_focus = true;
        }
        if pressed(Key::F3) {
            self.find_next(mods.shift);
        }
        if mods.ctrl && pressed(Key::A) {
            self.sel = Some((0, self.lines.len().saturating_sub(1)));
        }
        if ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
            if let Some((a, b)) = self.sel {
                let (a, b) = (a.min(b), a.max(b));
                let text = self.lines[a..=b.min(self.lines.len() - 1)].join("\r\n");
                ctx.copy_text(text);
            }
        }
        if pressed(Key::F) && !mods.ctrl && self.mode == Mode::Image {
            self.fit = !self.fit;
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.mode, Mode::Text, "1 Text");
            ui.selectable_value(&mut self.mode, Mode::Hex, "3 Hex");
            if is_image(&self.path) {
                ui.selectable_value(&mut self.mode, Mode::Image, "4 Image");
            }
            ui.separator();
            if ui.selectable_label(self.wrap, "W Wrap").clicked() {
                self.wrap = !self.wrap;
                self.visual_cols = 0;
            }
            if ui.button("F7 Find").clicked() {
                self.find_open = true;
                self.find_focus = true;
            }
            ui.separator();
            if ui.button("P Prev file").clicked() {
                self.next_file(-1);
            }
            if ui.button("N Next file").clicked() {
                self.next_file(1);
            }
            if self.mode == Mode::Image && ui.selectable_label(self.fit, "F Fit").clicked() {
                self.fit = !self.fit;
            }
        });
    }

    fn status(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let total = match self.mode {
                Mode::Text => self.visual.len().max(1),
                _ => 1,
            };
            let pct = if total > 1 { self.top * 100 / total } else { 0 };
            ui.label(format!(
                "{}   {} bytes   {}   {}%{}",
                self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                crate::format::thousands(self.size),
                if self.mode == Mode::Text { self.encoding } else { "" },
                pct,
                if self.truncated && self.mode == Mode::Text { "   (first 32 MB shown; use Hex for the rest)" } else { "" }
            ));
            if !self.message.is_empty() {
                ui.label(egui::RichText::new(&self.message).color(Color32::from_rgb(0xD0, 0x40, 0x30)));
            }
        });
    }

    fn find_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("Find:");
            let resp = ui.add(egui::TextEdit::singleline(&mut self.find).desired_width(300.0));
            if self.find_focus {
                resp.request_focus();
                self.find_focus = false;
            }
            let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if ui.button("Find next (F3)").clicked() || enter {
                self.found = None;
                self.find_next(false);
            }
            if ui.button("Close").clicked() {
                self.find_open = false;
            }
        });
    }

    fn draw_text(&mut self, ui: &mut egui::Ui, rect: Rect, pal: &Palette) {
        let font = FontId::monospace(13.0);
        let (cw, lh) = ui.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
        let inner = rect.shrink2(vec2(6.0, 4.0));
        let cols = ((inner.width() - 14.0) / cw).floor().max(10.0) as usize;
        self.rewrap(cols);
        let rows = (inner.height() / lh).floor().max(1.0) as usize;
        let total = self.visual.len();
        let resp = ui.interact(rect, egui::Id::unique(("lister-text", self.id)), Sense::click_and_drag());
        self.scroll(ui, &resp, rows, total, lh);
        let painter = ui.painter().with_clip_rect(inner);
        let sel = self.sel.map(|(a, b)| (a.min(b), a.max(b)));
        for r in 0..=rows {
            let vi = self.top + r;
            let Some(&(li, s, e)) = self.visual.get(vi) else { break };
            let y = inner.top() + r as f32 * lh;
            if sel.is_some_and(|(a, b)| li >= a && li <= b) {
                painter.rect_filled(Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), lh)), 0.0, pal.cursor_fill);
            }
            let line: String = self.lines[li].chars().skip(s).take(e - s).collect();
            if let Some((fl, fc)) = self.found {
                if fl == li && fc >= s && fc < e.max(s + 1) {
                    let x = inner.left() + (fc - s) as f32 * cw;
                    let w = self.find.chars().count() as f32 * cw;
                    painter.rect_filled(Rect::from_min_size(pos2(x, y), vec2(w, lh)), 1.0, Color32::from_rgb(0xFF, 0xD8, 0x4A));
                }
            }
            painter.text(pos2(inner.left(), y), Align2::LEFT_TOP, line, font.clone(), pal.text);
        }
        // Line selection with the mouse.
        if let Some(p) = resp.interact_pointer_pos() {
            let vi = self.top + ((p.y - inner.top()) / lh).floor().max(0.0) as usize;
            if let Some(&(li, _, _)) = self.visual.get(vi.min(total.saturating_sub(1))) {
                if resp.drag_started() || resp.clicked() {
                    self.sel = Some((li, li));
                } else if resp.dragged() {
                    if let Some((a, _)) = self.sel {
                        self.sel = Some((a, li));
                    }
                }
            }
        }
        scrollbar(ui, rect, self.top, rows, total, pal);
    }

    fn scroll(&mut self, ui: &egui::Ui, resp: &egui::Response, rows: usize, total: usize, lh: f32) {
        let max_top = total.saturating_sub(rows);
        if resp.hovered() {
            let lines = crate::ui::wheel_lines(ui, lh, rows);
            if lines != 0 {
                let step = lines.unsigned_abs();
                self.top = if lines > 0 { self.top.saturating_sub(step) } else { self.top + step };
            }
        }
        if !ui.ctx().text_edit_focused() {
            ui.input(|i| {
                if i.key_pressed(Key::ArrowDown) {
                    self.top += 1;
                }
                if i.key_pressed(Key::ArrowUp) {
                    self.top = self.top.saturating_sub(1);
                }
                if i.key_pressed(Key::PageDown) || i.key_pressed(Key::Space) {
                    self.top += rows.saturating_sub(1).max(1);
                }
                if i.key_pressed(Key::PageUp) {
                    self.top = self.top.saturating_sub(rows.saturating_sub(1).max(1));
                }
                if i.key_pressed(Key::Home) {
                    self.top = 0;
                }
                if i.key_pressed(Key::End) {
                    self.top = max_top;
                }
            });
        }
        self.top = self.top.min(max_top);
    }

    fn draw_hex(&mut self, ui: &mut egui::Ui, rect: Rect, pal: &Palette) {
        let font = FontId::monospace(13.0);
        let lh = ui.fonts_mut(|f| f.row_height(&font));
        let inner = rect.shrink2(vec2(6.0, 4.0));
        let rows = (inner.height() / lh).floor().max(1.0) as usize;
        let total = self.size.div_ceil(16) as usize;
        let resp = ui.interact(rect, egui::Id::unique(("lister-hex", self.id)), Sense::click_and_drag());
        self.scroll(ui, &resp, rows, total, lh);
        let painter = ui.painter().with_clip_rect(inner);
        for r in 0..=rows {
            let row = self.top + r;
            if row >= total {
                break;
            }
            let off = row as u64 * 16;
            let bytes = self.hex_bytes(off, 16);
            let mut hex = String::new();
            for (i, b) in bytes.iter().enumerate() {
                hex.push_str(&format!("{b:02X}"));
                hex.push(if i == 7 { '-' } else { ' ' });
            }
            let ascii: String = bytes.iter().map(|&b| if (32..127).contains(&b) { b as char } else { '.' }).collect();
            let line = format!("{off:08X}  {hex:<48}  {ascii}");
            painter.text(pos2(inner.left(), inner.top() + r as f32 * lh), Align2::LEFT_TOP, line, font.clone(), pal.text);
        }
        scrollbar(ui, rect, self.top, rows, total, pal);
    }

    fn draw_image(&mut self, ui: &mut egui::Ui, rect: Rect, pal: &Palette) {
        if self.image.is_none() {
            self.image = Some(match image::open(&self.path) {
                Ok(img) => {
                    let img = img.to_rgba8();
                    let size = [img.width() as usize, img.height() as usize];
                    let tex = ui.ctx().load_texture(
                        format!("lister:{}", self.path.display()),
                        egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()),
                        egui::TextureOptions::LINEAR,
                    );
                    Ok((tex, size))
                }
                Err(e) => Err(e.to_string()),
            });
        }
        match self.image.as_ref().unwrap() {
            Ok((tex, [w, h])) => {
                let (w, h) = (*w as f32, *h as f32);
                let scale = if self.fit { (rect.width() / w).min(rect.height() / h).min(1.0) } else { 1.0 };
                let r = Rect::from_center_size(rect.center(), vec2(w * scale, h * scale));
                // Checkerboard behind transparent images.
                ui.painter().rect_filled(r, 0.0, if pal.dark { Color32::from_gray(40) } else { Color32::from_gray(235) });
                ui.painter().image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                ui.painter().rect_stroke(r, 0.0, Stroke::new(1.0, pal.border), egui::StrokeKind::Outside);
                ui.painter().text(
                    rect.left_bottom() + vec2(8.0, -8.0),
                    Align2::LEFT_BOTTOM,
                    format!("{} x {}   {:.0}%", w as u32, h as u32, scale * 100.0),
                    FontId::proportional(12.0),
                    pal.text_dim,
                );
            }
            Err(e) => {
                ui.painter().text(rect.center(), Align2::CENTER_CENTER, format!("Cannot display image: {e}"), FontId::proportional(13.0), pal.error);
            }
        }
    }
}

fn scrollbar(ui: &egui::Ui, rect: Rect, top: usize, rows: usize, total: usize, pal: &Palette) {
    if total <= rows {
        return;
    }
    let track = Rect::from_min_max(pos2(rect.right() - 10.0, rect.top()), rect.max);
    let len = track.height();
    let thumb = (len * rows as f32 / total as f32).max(20.0);
    let off = (len - thumb) * top as f32 / (total - rows) as f32;
    ui.painter().rect_filled(Rect::from_min_size(pos2(track.left() + 2.0, track.top() + off), vec2(6.0, thumb)), 3.0, pal.scrollbar_thumb);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_detection_and_decoding() {
        assert!(!looks_binary(b"hello\r\nworld\t!"));
        assert!(looks_binary(&[0u8, 1, 2, 3, 0, 0, 5, 6, 7, 0]));
        assert!(!looks_binary(&[0xFF, 0xFE, b'a', 0]));
        assert_eq!(decode(b"a\r\nb\rc"), "a\nb\nc");
        assert_eq!(encoding_name(&[0xEF, 0xBB, 0xBF, b'x']), "UTF-8 (BOM)");
        assert_eq!(encoding_name(b"caf\xe9"), "ANSI (Windows-1252)");
        assert!(is_image(Path::new("a.PNG")));
        assert!(!is_image(Path::new("a.txt")));
    }
}
