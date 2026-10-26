//! Modal dialogs, including job progress and the overwrite prompt.

use crate::app::App;
use crate::config::{Config, HotlistEntry, QuickSearchMode, ThemeMode};
use crate::format;
use crate::ops::{Decision, OpKind, OpRequest, Source};
use crate::panel::FileFilter;
use crate::theme::Palette;
use egui::{Align, Context, Key, Layout, RichText, Ui};
use std::path::PathBuf;

/// Archive formats offered by the pack dialog.
const PACK_FORMATS: [&str; 6] = [".zip", ".7z", ".tar", ".tar.gz", ".tar.xz", ".tar.bz2"];

/// The archive extension at the end of `name` (".tar.gz" before ".gz"), or "".
fn archive_suffix(name: &str) -> &str {
    let lower = name.to_ascii_lowercase();
    let mut best = "";
    for ext in PACK_FORMATS {
        if lower.ends_with(ext) && ext.len() > best.len() {
            best = ext;
        }
    }
    &name[name.len() - best.len()..]
}

pub enum ConfirmAction {
    Delete(Vec<Source>, bool),
    /// Add a connection made with Ctrl+N to the saved list.
    SaveSite(Box<crate::remote::sites::Site>),
    /// Upload an edited temporary copy back to its server directory.
    Upload(PathBuf, std::sync::Arc<crate::remote::Connection>, String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKind {
    MkDir,
    NewFile,
    SelectGroup(bool),
    CustomFilter,
    QuickFilter,
}

pub struct AttrState {
    pub paths: Vec<PathBuf>,
    pub flags: [Option<bool>; 4],
    pub time: String,
    pub set_time: bool,
    pub recursive: bool,
}

pub enum Dialog {
    Message { title: String, text: String, error: bool },
    Confirm { title: String, text: String, action: ConfirmAction },
    Input { kind: InputKind, prompt: String, text: String, history: Vec<String>, init: bool },
    Transfer { mv: bool, sources: Vec<Source>, label: String, target: String, src_dir: Option<PathBuf>, init: bool },
    Pack { label: String, sources: Vec<Source>, target: String, move_files: bool, init: bool },
    Unpack { archives: Vec<PathBuf>, target: String, separate: bool, init: bool },
    Attributes(AttrState),
    Occupied { what: String, result: Option<(u64, u64, u64)> },
    Hotlist { entries: Vec<HotlistEntry>, sel: usize },
    Options(Box<Config>),
    MultiRename(Box<crate::rename::RenameTool>),
    Sync(Box<crate::sync::SyncTool>),
    Tree(Box<crate::tree::TreeDialog>),
    About,
    Keyboard,
    /// Saved FTP/SFTP connections (Ctrl+F).
    Sites { sel: usize },
    SiteEdit(Box<crate::remote_ui::SiteEdit>),
    /// Connect to a URL (Ctrl+N).
    QuickConnect { text: String, error: Option<String>, init: bool },
    Password(Box<crate::remote_ui::PasswordState>),
    HostKey { req: Box<crate::remote_ui::ConnectRequest>, info: crate::remote::HostKeyInfo },
    Certificate { req: Box<crate::remote_ui::ConnectRequest>, info: crate::remote::CertInfo },
    Connecting { label: String },
    /// Change button bar.
    BarEditor(Box<crate::bar_ui::BarEditor>),
    /// Edit "?" parameters before a button's program starts.
    RunParams(Box<crate::bar_ui::PendingRun>),
}

const ATTR_BITS: [(u32, &str); 4] = [
    (format::ATTR_READONLY, "Read only"),
    (format::ATTR_ARCHIVE, "Archive"),
    (format::ATTR_HIDDEN, "Hidden"),
    (format::ATTR_SYSTEM, "System"),
];

impl Dialog {
    pub fn input(kind: InputKind, prompt: &str, text: String, history: Vec<String>) -> Dialog {
        Dialog::Input { kind, prompt: prompt.to_string(), text, history, init: true }
    }

    pub fn attributes(paths: Vec<PathBuf>) -> Dialog {
        use std::os::windows::fs::MetadataExt;
        let metas: Vec<_> = paths.iter().filter_map(|p| std::fs::metadata(p).ok()).collect();
        let mut flags = [None; 4];
        for (i, (bit, _)) in ATTR_BITS.iter().enumerate() {
            let on = metas.iter().filter(|m| m.file_attributes() & bit != 0).count();
            flags[i] = if on == 0 {
                Some(false)
            } else if on == metas.len() {
                Some(true)
            } else {
                None
            };
        }
        let time = metas.first().and_then(|m| m.modified().ok()).map(format::datetime_secs).unwrap_or_default();
        Dialog::Attributes(AttrState { paths, flags, time, set_time: false, recursive: false })
    }
}

/// Shows a modal with a title; returns the inner value and whether Escape
/// was pressed.
pub(crate) fn modal<R>(ctx: &Context, id: &str, title: &str, pal: &Palette, width: f32, add: impl FnOnce(&mut Ui) -> R) -> (R, bool) {
    let frame = egui::Frame::window(&ctx.global_style()).fill(pal.face).inner_margin(egui::Margin::same(14));
    let resp = egui::Modal::new(egui::Id::unique(id)).frame(frame).show(ctx, |ui| {
        ui.set_width(width);
        ui.label(RichText::new(title).strong().size(ui.style().text_styles[&egui::TextStyle::Body].size + 1.5));
        ui.add_space(8.0);
        add(ui)
    });
    let esc = ctx.input(|i| i.key_pressed(Key::Escape));
    (resp.inner, esc)
}

/// Right-aligned button row; returns the index of the clicked button.
pub(crate) fn buttons(ui: &mut Ui, labels: &[&str]) -> Option<usize> {
    let mut clicked = None;
    ui.add_space(10.0);
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        for (i, l) in labels.iter().enumerate().rev() {
            if ui.add(egui::Button::new(*l).min_size(egui::vec2(84.0, 24.0))).clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

pub(crate) fn enter(ui: &Ui) -> bool {
    ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.any())
}

/// Focuses a text field and selects the char range `from..to` *before* it is
/// drawn: egui collapses the selection of fields that do not have focus.
pub fn focus_select(ui: &Ui, id: egui::Id, from: usize, to: usize) {
    ui.memory_mut(|m| m.request_focus(id));
    let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
    state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(from), egui::text::CCursor::new(to))));
    state.store(ui.ctx(), id);
}

/// Like [`focus_select`] for the main field of a modal dialog: applied on the
/// first two frames, because a modal's first frame is an invisible sizing
/// pass that drops keyboard focus.
pub fn focus_select_modal(ui: &Ui, id: egui::Id, first: bool, from: usize, to: usize) {
    let key = id.with("focus-frames");
    let left = if first { 2u8 } else { ui.data(|d| d.get_temp::<u8>(key)).unwrap_or(0) };
    if left > 0 {
        focus_select(ui, id, from, to);
        ui.data_mut(|d| d.insert_temp(key, left - 1));
    }
}

fn text_field(ui: &mut Ui, text: &mut String, id: &str, first: bool) -> egui::Response {
    let id = egui::Id::unique(id);
    focus_select_modal(ui, id, first, 0, text.chars().count());
    ui.add(egui::TextEdit::singleline(text).id(id).desired_width(f32::INFINITY))
}

pub fn draw(app: &mut App, ctx: &Context) {
    if app.dialog.is_none() {
        draw_job(app, ctx);
        return;
    }
    let pal = app.pal.clone();
    let mut dialog = app.dialog.take().unwrap();
    let keep = match &mut dialog {
        Dialog::Message { title, text, error } => {
            let (r, esc) = modal(ctx, "dlg-msg", title, &pal, 440.0, |ui| {
                let color = if *error { pal.error } else { pal.text };
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    ui.add(egui::Label::new(RichText::new(text.as_str()).color(color)).wrap());
                });
                buttons(ui, &["OK"]).is_some() || enter(ui)
            });
            !(r || esc)
        }
        Dialog::Confirm { title, text, action } => {
            let (r, esc) = modal(ctx, "dlg-confirm", title, &pal, 420.0, |ui| {
                ui.add(egui::Label::new(text.as_str()).wrap());
                let b = buttons(ui, &["Yes", "No"]);
                if enter(ui) { Some(0) } else { b }
            });
            match (r, esc) {
                (Some(0), _) => {
                    match action {
                        ConfirmAction::Delete(sources, recycle) => {
                            let req = OpRequest { kind: OpKind::Delete { recycle: *recycle }, sources: std::mem::take(sources), dest: None, mask: String::new() };
                            app.start_job(req);
                        }
                        ConfirmAction::SaveSite(site) => {
                            app.cfg.sites.push((**site).clone());
                            app.save_config();
                            app.toast("Connection saved (Ctrl+F)");
                        }
                        ConfirmAction::Upload(local, conn, dir) => {
                            app.upload_edited(local.clone(), conn.clone(), dir.clone());
                        }
                    }
                    false
                }
                (Some(_), _) | (_, true) => false,
                _ => true,
            }
        }
        Dialog::Input { kind, prompt, text, history, init } => {
            let title = match kind {
                InputKind::MkDir => "New folder",
                InputKind::NewFile => "Edit new file",
                InputKind::SelectGroup(true) => "Select",
                InputKind::SelectGroup(false) => "Unselect",
                InputKind::CustomFilter => "Custom file filter",
                InputKind::QuickFilter => "Quick filter",
            };
            let first = *init;
            *init = false;
            let (r, esc) = modal(ctx, "dlg-input", title, &pal, 420.0, |ui| {
                ui.label(prompt.as_str());
                let resp = text_field(ui, text, "dlg-input-text", first);
                if !history.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("History:").weak());
                        for h in history.iter().take(8) {
                            if ui.small_button(h).clicked() {
                                *text = h.clone();
                            }
                        }
                    });
                }
                let _ = resp;
                let b = buttons(ui, &["OK", "Cancel"]);
                if enter(ui) { Some(0) } else { b }
            });
            match (r, esc) {
                (Some(0), _) => {
                    let (kind, value) = (*kind, text.clone());
                    finish_input(app, kind, value);
                    false
                }
                (Some(_), _) | (_, true) => false,
                _ => true,
            }
        }
        Dialog::Transfer { mv, sources, label, target, src_dir, init } => {
            let first = *init;
            *init = false;
            let title = if *mv { "Move / rename" } else { "Copy" };
            let (r, esc) = modal(ctx, "dlg-transfer", title, &pal, 520.0, |ui| {
                ui.label(label.as_str());
                let id = egui::Id::unique("dlg-transfer-target");
                // Select the file name part (after the last backslash), like TC.
                let start = target.rfind('\\').map(|i| target[..=i].chars().count()).unwrap_or(0);
                focus_select_modal(ui, id, first, start, target.chars().count());
                ui.add(egui::TextEdit::singleline(target).id(id).desired_width(f32::INFINITY));
                ui.add_space(4.0);
                ui.label(RichText::new("Wildcards rename on the fly: *.bak, new_*.*, h?.txt").weak().small());
                let b = buttons(ui, &["OK", "Cancel"]);
                if enter(ui) { Some(0) } else { b }
            });
            match (r, esc) {
                (Some(0), _) => {
                    let (mv, sources, target, src_dir) = (*mv, std::mem::take(sources), target.clone(), src_dir.clone());
                    app.start_transfer(mv, sources, &target, src_dir);
                    false
                }
                (Some(_), _) | (_, true) => false,
                _ => true,
            }
        }
        Dialog::Pack { label, sources, target, move_files, init } => {
            let first = std::mem::take(init);
            let (r, esc) = modal(ctx, "dlg-pack", "Pack files", &pal, 520.0, |ui| {
                ui.label(label.as_str());
                let id = egui::Id::unique("dlg-pack-target");
                // Select the archive name without ".zip".
                let n = target.chars().count();
                let start = target.rfind('\\').map(|i| target[..=i].chars().count()).unwrap_or(0);
                let end = n - archive_suffix(target).chars().count();
                focus_select_modal(ui, id, first, start, end);
                ui.add(egui::TextEdit::singleline(target).id(id).desired_width(f32::INFINITY));
                // Format choice rewrites the extension (like TC's packer radio buttons).
                ui.horizontal(|ui| {
                    ui.label("Packer:");
                    let current = archive_suffix(target).to_ascii_lowercase();
                    for ext in PACK_FORMATS {
                        if ui.radio(current == *ext, ext.trim_start_matches('.').to_uppercase()).clicked() {
                            let stem_len = target.len() - archive_suffix(target).len();
                            target.truncate(stem_len);
                            target.push_str(ext);
                        }
                    }
                });
                ui.checkbox(move_files, "Move to archive (delete originals)");
                ui.label(RichText::new("Existing ZIP archives are updated; 7z and TAR archives are created new.").weak().small());
                let b = buttons(ui, &["OK", "Cancel"]);
                if enter(ui) { Some(0) } else { b }
            });
            match (r, esc) {
                (Some(0), _) => {
                    let mut archive = PathBuf::from(crate::vfs::expand_env(target.trim().trim_matches('"')));
                    if archive.extension().is_none() {
                        archive.set_extension("zip");
                    }
                    let req = OpRequest {
                        kind: OpKind::Pack { archive, move_files: *move_files },
                        sources: std::mem::take(sources),
                        dest: None,
                        mask: "*.*".into(),
                    };
                    app.start_job(req);
                    false
                }
                (Some(_), _) | (_, true) => false,
                _ => true,
            }
        }
        Dialog::Unpack { archives, target, separate, init } => {
            let first = std::mem::take(init);
            let what = if archives.len() == 1 {
                archives[0].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            } else {
                format!("{} archives", archives.len())
            };
            let (r, esc) = modal(ctx, "dlg-unpack", "Unpack files", &pal, 520.0, |ui| {
                ui.label(format!("Unpack {what} to:"));
                let id = egui::Id::unique("dlg-unpack-target");
                focus_select_modal(ui, id, first, 0, target.chars().count());
                ui.add(egui::TextEdit::singleline(target).id(id).desired_width(f32::INFINITY));
                ui.checkbox(separate, "Unpack each archive to a separate subdirectory (name of the archive)");
                let b = buttons(ui, &["OK", "Cancel"]);
                if enter(ui) { Some(0) } else { b }
            });
            match (r, esc) {
                (Some(0), _) => {
                    let (a, t, s) = (std::mem::take(archives), target.clone(), *separate);
                    app.start_unpack(a, &t, s);
                    false
                }
                (Some(_), _) | (_, true) => false,
                _ => true,
            }
        }
        Dialog::Attributes(st) => {
            let (r, esc) = modal(ctx, "dlg-attr", "Change attributes", &pal, 400.0, |ui| {
                ui.label(format!("{} item(s)", st.paths.len()));
                ui.add_space(4.0);
                for (i, (_, name)) in ATTR_BITS.iter().enumerate() {
                    let mut on = st.flags[i].unwrap_or(false);
                    let label = if st.flags[i].is_none() { format!("{name} (mixed - unchanged)") } else { name.to_string() };
                    if ui.checkbox(&mut on, label).changed() {
                        st.flags[i] = Some(on);
                    }
                }
                ui.add_space(4.0);
                ui.checkbox(&mut st.set_time, "Change date/time (YYYY-MM-DD HH:MM:SS):");
                ui.add_enabled(st.set_time, egui::TextEdit::singleline(&mut st.time).desired_width(200.0));
                ui.checkbox(&mut st.recursive, "Process subdirectory contents");
                buttons(ui, &["OK", "Cancel"])
            });
            match (r, esc) {
                (Some(0), _) => {
                    match apply_attributes(st) {
                        Ok(n) => {
                            app.refresh_all();
                            app.toast(format!("Attributes changed on {n} item(s)"));
                        }
                        Err(e) => app.error(e),
                    }
                    false
                }
                (Some(_), _) | (_, true) => false,
                _ => true,
            }
        }
        Dialog::Occupied { what, result } => {
            let (r, esc) = modal(ctx, "dlg-occupied", "Occupied space", &pal, 420.0, |ui| {
                ui.label(what.as_str());
                ui.add_space(6.0);
                match result {
                    None => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Calculating...");
                        });
                    }
                    Some((b, f, d)) => {
                        egui::Grid::new("occ").num_columns(2).spacing([16.0, 4.0]).show(ui, |ui| {
                            ui.label("Total size:");
                            ui.label(format!("{} bytes ({})", format::thousands(*b), format::human(*b)));
                            ui.end_row();
                            ui.label("Files:");
                            ui.label(format::thousands(*f));
                            ui.end_row();
                            ui.label("Directories:");
                            ui.label(format::thousands(*d));
                            ui.end_row();
                        });
                    }
                }
                buttons(ui, &["OK"]).is_some() || enter(ui)
            });
            if r || esc {
                app.size_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                false
            } else {
                true
            }
        }
        Dialog::Hotlist { entries, sel } => {
            let current = app.atab().loc.fs_path().map(|p| (app.atab().loc.caption(), p.to_string_lossy().into_owned()));
            let (r, esc) = modal(ctx, "dlg-hotlist", "Directory hotlist", &pal, 640.0, |ui| {
                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    egui::Grid::new("hotgrid").num_columns(3).spacing([6.0, 3.0]).show(ui, |ui| {
                        for (i, e) in entries.iter_mut().enumerate() {
                            if ui.radio(*sel == i, "").clicked() {
                                *sel = i;
                            }
                            if e.path.is_empty() && e.name == "-" {
                                ui.label(RichText::new("----- separator -----").weak());
                                ui.label("");
                            } else {
                                ui.add(egui::TextEdit::singleline(&mut e.name).desired_width(160.0));
                                ui.add(egui::TextEdit::singleline(&mut e.path).desired_width(380.0));
                            }
                            ui.end_row();
                        }
                    });
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if let Some((name, path)) = &current {
                        if ui.button("Add current dir").clicked() {
                            entries.push(HotlistEntry { name: name.clone(), path: path.clone() });
                            *sel = entries.len() - 1;
                        }
                    }
                    if ui.button("Add separator").clicked() {
                        entries.push(HotlistEntry { name: "-".into(), path: String::new() });
                    }
                    if ui.button("Move up").clicked() && *sel > 0 && *sel < entries.len() {
                        entries.swap(*sel, *sel - 1);
                        *sel -= 1;
                    }
                    if ui.button("Move down").clicked() && *sel + 1 < entries.len() {
                        entries.swap(*sel, *sel + 1);
                        *sel += 1;
                    }
                    if ui.button("Delete").clicked() && *sel < entries.len() {
                        entries.remove(*sel);
                        *sel = sel.saturating_sub(1);
                    }
                });
                buttons(ui, &["OK", "Cancel"])
            });
            match (r, esc) {
                (Some(0), _) => {
                    app.cfg.hotlist = std::mem::take(entries);
                    app.save_config();
                    false
                }
                (Some(_), _) | (_, true) => false,
                _ => true,
            }
        }
        Dialog::Options(cfg) => {
            let (r, esc) = modal(ctx, "dlg-options", "Options", &pal, 560.0, |ui| options_ui(ui, cfg));
            match (r, esc) {
                (Some(0), _) => {
                    let new = (**cfg).clone();
                    let font_changed = new.font_size != app.cfg.font_size;
                    let list_changed = new.show_hidden != app.cfg.show_hidden;
                    app.cfg = new;
                    if font_changed {
                        crate::app::setup_fonts(ctx, app.cfg.font_size);
                    }
                    app.reapply_theme();
                    if list_changed {
                        app.refresh_all();
                    }
                    app.save_config();
                    false
                }
                (Some(_), _) | (_, true) => false,
                _ => true,
            }
        }
        Dialog::MultiRename(tool) => {
            let (r, esc) = modal(ctx, "dlg-mrt", "Multi-Rename Tool", &pal, 820.0, |ui| tool.ui(ui, &pal));
            match r {
                crate::rename::RenameResult::Done(msg) => {
                    app.refresh_all();
                    app.toast(msg);
                    false
                }
                crate::rename::RenameResult::Error(msg) => {
                    app.refresh_all();
                    app.error(msg);
                    false
                }
                crate::rename::RenameResult::Close => false,
                crate::rename::RenameResult::Open => !esc,
            }
        }
        Dialog::Sync(tool) => {
            let (r, esc) = modal(ctx, "dlg-sync", "Synchronize directories", &pal, 940.0, |ui| tool.ui(ui, &pal));
            match r {
                crate::sync::SyncResult::Start(pairs) => {
                    app.start_job(OpRequest { kind: OpKind::CopyPairs(pairs), sources: Vec::new(), dest: None, mask: String::new() });
                    false
                }
                crate::sync::SyncResult::Close => false,
                crate::sync::SyncResult::Open => !esc,
            }
        }
        Dialog::Tree(tree) => {
            let (r, esc) = modal(ctx, "dlg-tree", "Change directory (tree)", &pal, 460.0, |ui| tree.ui(ui, &pal));
            match r {
                crate::tree::TreeResult::Go(p) => {
                    let side = app.active;
                    app.go(side, crate::vfs::Location::fs(p), None);
                    false
                }
                crate::tree::TreeResult::Close => false,
                crate::tree::TreeResult::Open => !esc,
            }
        }
        Dialog::About => {
            let (r, esc) = modal(ctx, "dlg-about", "About MCommander", &pal, 460.0, |ui| {
                ui.label(RichText::new(format!("MCommander {}", env!("CARGO_PKG_VERSION"))).size(20.0).strong());
                ui.label("A dual-pane file manager for Windows, written in Rust.");
                ui.add_space(8.0);
                ui.label("Inspired by Total Commander (c) Christian Ghisler. MCommander is an independent, clean-room reimplementation and is not affiliated with or endorsed by Ghisler Software.");
                ui.add_space(8.0);
                ui.label(format!("Settings: {}", app.cfg.file.display()));
                ui.hyperlink_to("github.com/Mad-Mange/MCommander", "https://github.com/Mad-Mange/MCommander");
                ui.label(RichText::new("MIT License. Built with egui/eframe and the windows crate.").weak());
                buttons(ui, &["OK"]).is_some() || enter(ui)
            });
            !(r || esc)
        }
        Dialog::Keyboard => {
            let (r, esc) = modal(ctx, "dlg-keys", "Keyboard", &pal, 620.0, |ui| {
                egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| {
                    egui::Grid::new("keys").num_columns(2).striped(true).spacing([18.0, 3.0]).show(ui, |ui| {
                        for (k, d) in crate::commands::KEYBOARD_HELP {
                            ui.label(RichText::new(*k).strong());
                            ui.label(*d);
                            ui.end_row();
                        }
                    });
                });
                buttons(ui, &["OK"]).is_some()
            });
            !(r || esc)
        }
        Dialog::BarEditor(ed) => crate::bar_ui::draw_editor(app, ctx, ed),
        Dialog::RunParams(run) => crate::bar_ui::draw_run_params(app, ctx, run),
        Dialog::Sites { .. }
        | Dialog::SiteEdit(_)
        | Dialog::QuickConnect { .. }
        | Dialog::Password(_)
        | Dialog::HostKey { .. }
        | Dialog::Certificate { .. }
        | Dialog::Connecting { .. } => crate::remote_ui::draw_dialog(app, ctx, &mut dialog),
    };
    if keep && app.dialog.is_none() {
        app.dialog = Some(dialog);
    }
}

fn finish_input(app: &mut App, kind: InputKind, value: String) {
    let side = app.active;
    match kind {
        InputKind::MkDir => {
            let name = value.trim().to_string();
            if name.is_empty() {
                return;
            }
            match app.atab().loc.clone() {
                crate::vfs::Location::Fs(dir) => {
                    let target = crate::vfs::resolve_input_path(&dir, &name);
                    match std::fs::create_dir_all(&target) {
                        Ok(()) => {
                            app.refresh(side);
                            let first = name.split(['\\', '/']).next().unwrap_or(&name).to_string();
                            if let Some(i) = app.atab().index_of(&first) {
                                app.atab_mut().set_cursor(i);
                            }
                        }
                        Err(e) => app.error(format!("Could not create {}:\n{e}", target.display())),
                    }
                }
                crate::vfs::Location::Remote { path, .. } => {
                    let dirs: Vec<String> = name.split(['/', '\\']).filter(|p| !p.is_empty()).map(str::to_string).collect();
                    let first = dirs.first().cloned();
                    app.remote_op(side, first, move |fs| {
                        let mut at = path;
                        for d in &dirs {
                            at = crate::remote::join(&at, d);
                            if let Err(e) = fs.mkdir(&at) {
                                // An existing parent is fine for "a/b".
                                if d == dirs.last().unwrap() {
                                    return Err(e);
                                }
                            }
                        }
                        Ok(())
                    });
                }
                crate::vfs::Location::Archive { archive, inner } => {
                    let tmp = std::env::temp_dir().join("MCommander").join("mkdir").join(&name);
                    let _ = std::fs::create_dir_all(&tmp);
                    let item = crate::archive::AddItem { source: tmp, name: if inner.is_empty() { name.clone() } else { format!("{inner}/{name}") } };
                    let cancel = std::sync::atomic::AtomicBool::new(false);
                    if let Err(e) = crate::archive::rebuild(&archive, &|_| true, &[item], &mut |_, _| true, &cancel) {
                        app.error(e.to_string());
                    }
                    app.refresh(side);
                }
                _ => {}
            }
        }
        InputKind::NewFile => {
            let Some(dir) = app.atab().loc.fs_path().map(|p| p.to_path_buf()) else { return };
            let path = crate::vfs::resolve_input_path(&dir, value.trim());
            if !path.exists() {
                if let Err(e) = std::fs::write(&path, b"") {
                    app.error(format!("{}: {e}", path.display()));
                    return;
                }
                app.refresh(side);
            }
            let editor = app.cfg.editor.clone();
            if let Err(e) = crate::win::spawn_detached(&editor, &format!("\"{}\"", path.display()), Some(&dir)) {
                app.error(format!("Could not start editor: {e}"));
            }
        }
        InputKind::SelectGroup(select) => {
            Config::push_history(&mut app.cfg.select_history, &value);
            let dirs = app.cfg.num_plus_selects_dirs || !select;
            let n = app.atab_mut().select_mask(&value, select, dirs);
            app.toast(format!("{} {n} item(s)", if select { "Selected" } else { "Unselected" }));
        }
        InputKind::CustomFilter => {
            Config::push_history(&mut app.cfg.select_history, &value);
            app.atab_mut().filter = if value.trim().is_empty() || value.trim() == "*.*" { FileFilter::All } else { FileFilter::Custom(value) };
            app.refresh(side);
        }
        InputKind::QuickFilter => {
            app.atab_mut().quick_filter = if value.trim().is_empty() { None } else { Some(value.trim().to_string()) };
            app.refresh(side);
            app.atab_mut().set_cursor(0);
        }
    }
}

fn apply_attributes(st: &AttrState) -> Result<usize, String> {
    use std::os::windows::fs::MetadataExt;
    let time = if st.set_time {
        let dt = chrono::NaiveDateTime::parse_from_str(st.time.trim(), "%Y-%m-%d %H:%M:%S")
            .or_else(|_| chrono::NaiveDateTime::parse_from_str(st.time.trim(), "%Y-%m-%d %H:%M"))
            .map_err(|_| format!("Invalid date/time: {}", st.time))?;
        use chrono::TimeZone;
        Some(std::time::SystemTime::from(chrono::Local.from_local_datetime(&dt).earliest().ok_or("Invalid local time")?))
    } else {
        None
    };
    let mut all = Vec::new();
    for p in &st.paths {
        all.push(p.clone());
        if st.recursive && p.is_dir() {
            let mut stack = vec![p.clone()];
            while let Some(d) = stack.pop() {
                if let Ok(rd) = std::fs::read_dir(&d) {
                    for e in rd.flatten() {
                        let path = e.path();
                        if path.is_dir() {
                            stack.push(path.clone());
                        }
                        all.push(path);
                    }
                }
            }
        }
    }
    let mut errors = Vec::new();
    for p in &all {
        let Ok(meta) = std::fs::metadata(p) else { continue };
        let mut attrs = meta.file_attributes();
        for (i, (bit, _)) in ATTR_BITS.iter().enumerate() {
            match st.flags[i] {
                Some(true) => attrs |= bit,
                Some(false) => attrs &= !bit,
                None => {}
            }
        }
        let w = crate::win::wide_path(p);
        // Clear read-only first so the time can be written.
        let res = unsafe {
            windows::Win32::Storage::FileSystem::SetFileAttributesW(
                windows::core::PCWSTR(w.as_ptr()),
                windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(attrs & !format::ATTR_READONLY),
            )
        };
        if let Err(e) = res {
            errors.push(format!("{}: {e}", p.display()));
            continue;
        }
        if let (Some(t), false) = (time, meta.is_dir()) {
            if let Err(e) = std::fs::OpenOptions::new().write(true).open(p).and_then(|f| f.set_modified(t)) {
                errors.push(format!("{}: {e}", p.display()));
            }
        }
        if attrs & format::ATTR_READONLY != 0 {
            unsafe {
                let _ = windows::Win32::Storage::FileSystem::SetFileAttributesW(
                    windows::core::PCWSTR(w.as_ptr()),
                    windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(attrs),
                );
            }
        }
    }
    if errors.is_empty() { Ok(all.len()) } else { Err(errors.join("\n")) }
}

fn options_ui(ui: &mut Ui, cfg: &mut Config) -> Option<usize> {
    egui::Grid::new("opts").num_columns(2).spacing([20.0, 6.0]).show(ui, |ui| {
        ui.label(RichText::new("Display").strong());
        ui.end_row();
        ui.label("Theme");
        ui.horizontal(|ui| {
            ui.radio_value(&mut cfg.theme, ThemeMode::Light, "Light");
            ui.radio_value(&mut cfg.theme, ThemeMode::Dark, "Dark");
            ui.radio_value(&mut cfg.theme, ThemeMode::System, "Follow Windows");
        });
        ui.end_row();
        ui.label("Font size");
        ui.add(egui::Slider::new(&mut cfg.font_size, 10.0..=18.0).step_by(0.5));
        ui.end_row();
        ui.label("File list");
        ui.vertical(|ui| {
            ui.checkbox(&mut cfg.show_hidden, "Show hidden and system files");
            ui.checkbox(&mut cfg.dir_brackets, "Show square brackets around directories");
        });
        ui.end_row();
        ui.label("Window parts");
        ui.vertical(|ui| {
            ui.checkbox(&mut cfg.show_toolbar, "Button bar");
            ui.checkbox(&mut cfg.show_drive_buttons, "Drive buttons");
            ui.checkbox(&mut cfg.show_middle_bar, "Vertical button bar");
            ui.checkbox(&mut cfg.show_status, "Status lines");
            ui.checkbox(&mut cfg.show_cmdline, "Command line");
            ui.checkbox(&mut cfg.show_fkeys, "Function key buttons");
        });
        ui.end_row();
        ui.label(RichText::new("Operation").strong());
        ui.end_row();
        ui.label("Quick search");
        egui::ComboBox::from_id_salt("qs").selected_text(cfg.quick_search.label()).show_ui(ui, |ui| {
            for m in QuickSearchMode::ALL {
                ui.selectable_value(&mut cfg.quick_search, m, m.label());
            }
        });
        ui.end_row();
        ui.label("Mouse selection");
        ui.vertical(|ui| {
            ui.radio_value(&mut cfg.right_button_select, true, "Right button selects, hold it for the context menu (Total Commander)");
            ui.radio_value(&mut cfg.right_button_select, false, "Left button with Ctrl/Shift, right button for the context menu (Explorer)");
        });
        ui.end_row();
        ui.label("Selection");
        ui.vertical(|ui| {
            ui.checkbox(&mut cfg.space_calculates_dirs, "Space on a directory calculates its size");
            ui.checkbox(&mut cfg.num_plus_selects_dirs, "Num + / Num * include directories");
        });
        ui.end_row();
        ui.label("Delete");
        ui.vertical(|ui| {
            ui.checkbox(&mut cfg.use_recycle_bin, "Delete to recycle bin (Shift+Del deletes permanently)");
            ui.checkbox(&mut cfg.confirm_delete, "Confirm before deleting");
        });
        ui.end_row();
        ui.label(RichText::new("Programs").strong());
        ui.end_row();
        ui.label("Editor (F4)");
        ui.add(egui::TextEdit::singleline(&mut cfg.editor).desired_width(320.0));
        ui.end_row();
        ui.label("Command prompt");
        ui.add(egui::TextEdit::singleline(&mut cfg.terminal).desired_width(320.0));
        ui.end_row();
    });
    ui.add_space(4.0);
    ui.label(RichText::new(format!("Settings file: {}", cfg.file.display())).weak().small());
    buttons(ui, &["OK", "Cancel"])
}

// ---- job progress -----------------------------------------------------------------------

fn bar(ui: &mut Ui, frac: f32, text: String) {
    // ProgressBar takes its width literally (unlike TextEdit): an infinite
    // width turns into NaN layout during a modal's sizing pass.
    let frac = if frac.is_finite() { frac.clamp(0.0, 1.0) } else { 0.0 };
    ui.add(egui::ProgressBar::new(frac).text(text).desired_width(ui.available_width().min(2000.0)));
}

fn draw_job(app: &mut App, ctx: &Context) {
    let Some(idx) = app.jobs.iter().position(|j| !j.background) else { return };
    let pal = app.pal.clone();
    let p = app.jobs[idx].progress();
    if let Some(c) = &p.conflict {
        let (r, _) = modal(ctx, "dlg-overwrite", "File exists!", &pal, 560.0, |ui| {
            ui.label("Overwrite:");
            ui.label(RichText::new(&c.target).strong());
            ui.label(format!(
                "{} bytes   {}",
                format::thousands(c.dst_size),
                c.dst_time.map(format::datetime_secs).unwrap_or_default()
            ));
            ui.add_space(6.0);
            ui.label("With:");
            ui.label(format!(
                "{} bytes   {}",
                format::thousands(c.src_size),
                c.src_time.map(format::datetime_secs).unwrap_or_default()
            ));
            ui.add_space(10.0);
            let mut choice = None;
            ui.horizontal_wrapped(|ui| {
                for (label, d) in [
                    ("Overwrite", Decision::Overwrite),
                    ("Overwrite all", Decision::OverwriteAll),
                    ("Skip", Decision::Skip),
                    ("Skip all", Decision::SkipAll),
                    ("Overwrite all older", Decision::OverwriteOlder),
                    ("Rename copied", Decision::AutoRename),
                    ("Auto-rename all", Decision::AutoRenameAll),
                    ("Cancel", Decision::Cancel),
                ] {
                    if ui.add(egui::Button::new(label).min_size(egui::vec2(96.0, 24.0))).clicked() {
                        choice = Some(d);
                    }
                }
            });
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                choice = Some(Decision::Cancel);
            }
            choice
        });
        if let Some(d) = r {
            if d == Decision::Cancel {
                app.jobs[idx].cancel();
            } else {
                app.jobs[idx].decide(d);
            }
        }
        return;
    }
    let (r, esc) = modal(ctx, "dlg-progress", &p.title, &pal, 520.0, |ui| {
        if p.counting {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Counting files...");
            });
        } else {
            ui.add(egui::Label::new(RichText::new(&p.current).small()).truncate());
            if !p.target.is_empty() {
                ui.add(egui::Label::new(RichText::new(format!("-> {}", p.target)).small()).truncate());
            }
            ui.add_space(4.0);
            if p.cur_total > 0 {
                bar(ui, p.cur_done as f32 / p.cur_total as f32, format!("{} / {}", format::human(p.cur_done), format::human(p.cur_total)));
            }
            let frac = if p.bytes_total > 0 {
                p.bytes_done as f32 / p.bytes_total as f32
            } else if p.files_total > 0 {
                p.files_done as f32 / p.files_total as f32
            } else {
                0.0
            };
            let secs = p.started.elapsed().as_secs_f64().max(0.001);
            let speed = p.bytes_done as f64 / secs;
            bar(ui, frac, format!("{} / {} file(s)   {}/s", p.files_done, p.files_total, format::human(speed as u64)));
        }
        buttons(ui, &["Background", "Cancel"])
    });
    match (r, esc) {
        (Some(0), _) => app.jobs[idx].background = true,
        (Some(1), _) | (_, true) => app.jobs[idx].cancel(),
        _ => {}
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
}
