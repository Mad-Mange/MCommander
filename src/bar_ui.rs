//! The button bars on screen: drawing, clicks, files dropped on buttons,
//! running a button, and the "Change button bar" dialog.

use crate::app::{App, PopupItem};
use crate::buttonbar::{Action, BarButton, BarKind, ButtonBar, IconSource, Step};
use crate::commands::Cmd;
use crate::paint_icons::{self, Icon};
use crate::panel::Side;
use crate::vfs::Location;
use egui::{Color32, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use std::path::{Path, PathBuf};

/// Size of a button in the bar under the menu.
const MAIN_BUTTON: f32 = 30.0;
/// Size of a button and its row in the vertical bar.
const VERT_BUTTON: f32 = 28.0;
pub const VERT_STEP: f32 = 32.0;

/// A program waiting to be started (after "?" parameters were edited).
pub struct PendingRun {
    pub program: String,
    pub args: String,
    pub dir: Option<PathBuf>,
    /// SW_ value.
    pub show: i32,
    pub admin: bool,
    pub list_file: Option<PathBuf>,
    /// Focus the parameters field (first frame of the "?" dialog).
    pub focus: bool,
}

impl App {
    /// The file a bar is kept in, next to the settings.
    pub fn bar_file(&self, kind: BarKind) -> PathBuf {
        self.cfg.file.parent().map(Path::to_path_buf).unwrap_or_default().join(kind.file_name())
    }

    /// Reads both bars (the built-in ones when there is no file yet).
    pub fn load_bars(&mut self) {
        for kind in [BarKind::Main, BarKind::Vertical] {
            self.bars[kind.index()] = ButtonBar::load(&self.bar_file(kind)).unwrap_or_else(|_| ButtonBar::default_for(kind));
        }
        self.bar_icons.clear();
    }

    pub fn set_bar(&mut self, kind: BarKind, bar: ButtonBar) {
        let file = self.bar_file(kind);
        self.bars[kind.index()] = bar;
        self.bar_icons.clear();
        if let Err(e) = self.bars[kind.index()].save(&file) {
            self.error(format!("Could not save {}: {e}", file.display()));
        }
    }

    /// Copies Total Commander's button bars; returns which were found.
    pub fn import_tc_bars(&mut self, tc_ini: &Path, ini: &crate::ini::Ini) -> Vec<&'static str> {
        let tc_dir = crate::buttonbar::tc_program_dir(ini).unwrap_or_default();
        let mut done = Vec::new();
        for (kind, file) in [BarKind::Main, BarKind::Vertical].into_iter().zip(crate::buttonbar::tc_bar_files(tc_ini, ini)) {
            let Some(file) = file else { continue };
            if let Ok(mut bar) = ButtonBar::load(&file) {
                bar.resolve_commander_path(&tc_dir);
                self.set_bar(kind, bar);
                done.push(kind.title());
            }
        }
        done
    }

    fn icon_source(&mut self, b: &BarButton) -> IconSource {
        let key = (b.icon.clone(), b.cmd.clone());
        if let Some(s) = self.bar_icons.get(&key) {
            return s.clone();
        }
        let s = crate::buttonbar::icon_source(b);
        self.bar_icons.insert(key, s.clone());
        s
    }

    /// Placeholder values (`%P`, `%N`, `%S`...) from the two panels.
    pub fn param_context(&self) -> crate::params::Context {
        let info = |side: Side| {
            let tab = self.tab(side);
            let results = matches!(tab.loc, Location::Results { .. });
            let dir = match &tab.loc {
                Location::Fs(p) => {
                    let s = p.to_string_lossy().into_owned();
                    if s.ends_with('\\') { s } else { format!("{s}\\") }
                }
                Location::Results { .. } => String::new(),
                other => other.display(),
            };
            let name = |e: &crate::vfs::FileEntry| match (&e.full_path, results) {
                (Some(p), true) => p.to_string_lossy().into_owned(),
                _ => e.name.clone(),
            };
            crate::params::PanelInfo {
                dir,
                cursor: tab.cursor_entry().filter(|e| !e.is_parent).map(name),
                selected: tab.entries.iter().filter(|e| e.selected).map(name).collect(),
            }
        };
        crate::params::Context { source: info(self.active), target: info(self.active.other()), source_is_left: self.active == Side::Left }
    }

    /// Runs a button: internal commands, a sub-bar menu or a program.
    pub fn run_button(&mut self, ctx: &egui::Context, b: BarButton, dropped: Vec<PathBuf>) {
        match b.action() {
            Action::Nothing => {}
            Action::SubBar(path) => self.open_sub_bar(&path),
            Action::Internal(steps) => {
                let unknown: Vec<&str> = steps.iter().filter_map(|s| if let Step::Unknown(n) = s { Some(n.as_str()) } else { None }).collect();
                if !unknown.is_empty() {
                    self.error(format!("{} is not available in MCommander (yet).", unknown.join(", ")));
                    return;
                }
                let mut delay = 0;
                let mut cd = false;
                for s in steps {
                    let cmd = match s {
                        Step::Wait(ms) => {
                            delay += ms;
                            continue;
                        }
                        Step::Cmd(c) => c,
                        Step::Cd(p) => {
                            cd = true;
                            Cmd::GotoPath(p)
                        }
                        Step::Unknown(_) => continue,
                    };
                    if delay == 0 {
                        self.exec(ctx, cmd);
                    } else {
                        self.delayed.push((std::time::Instant::now() + std::time::Duration::from_millis(delay), cmd));
                    }
                }
                // "cd" buttons: the start path goes to the other panel.
                let other = crate::vfs::expand_env(b.path.trim());
                if cd && !other.is_empty() {
                    let side = self.active.other();
                    self.goto_typed_path(side, &other);
                    self.active = side.other();
                }
            }
            Action::Program { program, args, admin } => {
                let show = match b.iconic {
                    1 => 7,  // SW_SHOWMINNOACTIVE
                    -1 => 3, // SW_SHOWMAXIMIZED
                    _ => 1,  // SW_SHOWNORMAL
                };
                let start = crate::vfs::expand_env(b.path.trim());
                let dir = if start.is_empty() { self.atab().loc.host_dir() } else { Some(PathBuf::from(start)) };
                let (extra, list_file, ask) = if dropped.is_empty() {
                    let e = crate::params::expand(&b.params, &self.param_context());
                    (e.args, e.list_file, e.ask)
                } else {
                    // Dropped files replace the parameters, as in TC.
                    let names: Vec<String> = dropped.iter().map(|p| quote(&p.to_string_lossy())).collect();
                    (names.join(" "), None, false)
                };
                let args = [args.trim(), extra.trim()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ");
                let run = PendingRun { program, args, dir, show, admin, list_file, focus: true };
                if ask {
                    self.dialog = Some(crate::dialogs::Dialog::RunParams(Box::new(run)));
                } else {
                    self.launch(run);
                }
            }
        }
    }

    pub fn launch(&mut self, r: PendingRun) {
        let resolved = if r.program.contains(['\\', '/', ':']) {
            let p = PathBuf::from(&r.program);
            match (&r.dir, p.is_relative()) {
                (Some(d), true) if d.join(&p).exists() => d.join(&p),
                _ => p,
            }
        } else {
            crate::win::find_on_path(&r.program, r.dir.as_deref()).unwrap_or_else(|| PathBuf::from(&r.program))
        };
        let prog = resolved.to_string_lossy().into_owned();
        let ext = resolved.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let result = if (ext == "cmd" || ext == "bat") && !r.admin && crate::win::batch_starts_gui_only(&resolved) {
            // A launcher such as VS Code's code.cmd: no console window.
            crate::win::spawn_hidden("cmd.exe", &format!("/S /C \"\"{prog}\" {}\"", r.args), r.dir.as_deref()).map(|_| None).map_err(|e| e.to_string())
        } else {
            crate::win::shell_execute_ex(self.hwnd, if r.admin { "runas" } else { "open" }, &prog, &r.args, r.dir.as_deref(), r.show)
        };
        match result {
            Ok(process) => match r.list_file {
                Some(f) => crate::win::delete_when_done(process, f),
                None => crate::win::close_process(process),
            },
            Err(e) => {
                if let Some(f) = r.list_file {
                    let _ = std::fs::remove_file(f);
                }
                self.error(format!("Could not run {prog}:\n{e}"));
            }
        }
    }

    /// Shows another bar file as a drop-down menu under the clicked button.
    pub fn open_sub_bar(&mut self, path: &Path) {
        let mut p = path.to_path_buf();
        if p.is_relative() {
            p = self.cfg.file.parent().map(|d| d.join(&p)).filter(|f| f.is_file()).unwrap_or(p);
        }
        let bar = match ButtonBar::load(&p) {
            Ok(b) => b,
            Err(e) => {
                self.error(format!("Could not open the button bar {}:\n{e}", p.display()));
                return;
            }
        };
        let items: Vec<PopupItem> = bar
            .buttons
            .into_iter()
            .filter(|b| !b.is_break())
            .map(|b| {
                if b.is_separator() {
                    PopupItem { label: String::new(), detail: String::new(), cmd: None }
                } else {
                    PopupItem { label: b.label(), detail: String::new(), cmd: Some(Cmd::RunButton { button: Box::new(b), dropped: Vec::new() }) }
                }
            })
            .collect();
        let pos = self.bar_anchor.unwrap_or_else(|| self.list_rects[self.active.index()].left_top());
        self.open_popup_at(items, pos);
    }

    /// The bar button under a screen position.
    pub fn bar_button_at(&self, p: Pos2) -> Option<(BarKind, usize)> {
        self.bar_hits.iter().find(|(_, _, r)| r.contains(p)).map(|(k, i, _)| (*k, *i))
    }

    /// Files dropped on a bar: on a button they are passed to its program;
    /// on an empty spot (or with Shift) they become new buttons. Returns
    /// false when `p` is not on a bar.
    pub fn drop_on_bar(&mut self, ctx: &egui::Context, p: Pos2, files: Vec<PathBuf>, shift: bool) -> bool {
        if files.is_empty() {
            return false;
        }
        if let (Some((kind, i)), false) = (self.bar_button_at(p), shift) {
            let b = self.bars[kind.index()].buttons[i].clone();
            match b.action() {
                Action::Program { .. } => self.run_button(ctx, b, files),
                _ => self.toast("Files can only be dropped on buttons that start a program"),
            }
            return true;
        }
        let Some(kind) = [BarKind::Main, BarKind::Vertical].into_iter().find(|k| self.bar_areas[k.index()].contains(p)) else { return false };
        let at = self.bar_button_at(p).map(|(_, i)| i + 1);
        let mut bar = self.bars[kind.index()].clone();
        let mut pos = at.unwrap_or(bar.buttons.len());
        for f in &files {
            let path = f.to_string_lossy().into_owned();
            let name = f.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let cmd = if path.contains(' ') { format!("\"{path}\"") } else { path.clone() };
            bar.buttons.insert(pos, BarButton { cmd, icon: path, tip: name, ..Default::default() });
            pos += 1;
        }
        self.set_bar(kind, bar);
        self.toast(format!("Added {} button(s) to the {}", files.len(), kind.title()));
        true
    }

    fn bar_menu(&mut self, kind: BarKind, index: Option<usize>, at: Pos2) {
        let mut items = Vec::new();
        if let Some(i) = index {
            items.push(PopupItem { label: "Change...".into(), detail: String::new(), cmd: Some(Cmd::EditBarButton(kind, i)) });
            items.push(PopupItem { label: "Delete".into(), detail: String::new(), cmd: Some(Cmd::DeleteBarButton(kind, i)) });
            items.push(PopupItem { label: String::new(), detail: String::new(), cmd: None });
        }
        let what = if kind == BarKind::Main { "button bar" } else { "vertical button bar" };
        items.push(PopupItem { label: format!("Change {what}..."), detail: String::new(), cmd: Some(Cmd::ConfigureBar(kind)) });
        items.push(PopupItem { label: format!("Hide {what}"), detail: String::new(), cmd: Some(Cmd::ToggleBar(kind)) });
        self.open_popup_at(items, at);
    }
}

fn quote(s: &str) -> String {
    if s.contains(' ') { format!("\"{s}\"") } else { s.to_string() }
}

/// Paints a button's picture.
pub fn paint_button_icon(app: &mut App, painter: &egui::Painter, rect: Rect, b: &BarButton) {
    let fg = app.pal.text;
    let px = (rect.width() * app.ppp).round().max(16.0) as u32;
    let tex = match app.icon_source(b) {
        IconSource::Painted(icon) => {
            paint_icons::paint(painter, rect, icon, fg);
            return;
        }
        IconSource::File(path, index) => app.icons.for_resource(&path, index, px),
        IconSource::FileType(ext) => app.icons.for_file_type(&ext, px),
    };
    match tex {
        Some(t) => {
            painter.image(t, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => paint_icons::paint(painter, rect, Icon::Command, fg.gamma_multiply(0.4)),
    }
}

/// Pressed look and whether the button can be used now.
fn button_state(app: &App, b: &BarButton) -> (bool, bool) {
    match b.action() {
        Action::Internal(steps) => match steps.as_slice() {
            [Step::Cmd(c)] => {
                let on = crate::ui::checked(app, c) == Some(true) || (*c == Cmd::FtpConnect && app.atab().loc.remote().is_some());
                let enabled = match c {
                    Cmd::HistoryBack => app.atab().can_go_back(),
                    Cmd::HistoryForward => app.atab().can_go_forward(),
                    _ => true,
                };
                (on, enabled)
            }
            s => (false, !s.iter().any(|s| matches!(s, Step::Unknown(_)))),
        },
        _ => (false, true),
    }
}

/// One button: hover look, click, right-click menu and tooltip.
fn bar_button(app: &mut App, ui: &mut Ui, rect: Rect, kind: BarKind, index: usize, inset: f32) {
    let b = app.bars[kind.index()].buttons[index].clone();
    let id = egui::Id::unique(("bar-button", kind.index(), index));
    let resp = ui.interact(rect, id, Sense::click());
    app.bar_hits.push((kind, index, rect));
    let (on, enabled) = button_state(app, &b);
    let pal = app.pal.clone();
    let dragging_over = app.drag.as_ref().is_some_and(|d| d.active) && ui.rect_contains_pointer(rect);
    if (enabled && (on || resp.hovered())) || dragging_over {
        let fill = if on { pal.pressed } else { pal.hover };
        ui.painter().rect(rect, 2.0, fill, Stroke::new(1.0, if dragging_over { pal.accent } else { pal.button_border }), StrokeKind::Inside);
    }
    let mut painter = ui.painter().clone();
    if !enabled {
        painter.multiply_opacity(0.35);
    }
    paint_button_icon(app, &painter, rect.shrink(inset), &b);
    if matches!(b.action(), Action::SubBar(_)) {
        // Small arrow: this button opens a menu.
        let c = rect.right_bottom() + vec2(-5.0, -5.0);
        painter.add(egui::Shape::convex_polygon(vec![c + vec2(-3.0, -2.0), c + vec2(3.0, -2.0), c + vec2(0.0, 1.5)], pal.text, Stroke::NONE));
    }
    let resp = resp.on_hover_ui(|ui| {
        let tip = b.tooltip();
        let connected = app.atab().loc.remote().is_some();
        let tip = if connected && matches!(b.action(), Action::Internal(ref s) if s.as_slice() == [Step::Cmd(Cmd::FtpConnect)]) {
            crate::buttonbar::describe(&Cmd::FtpDisconnect)
        } else {
            tip
        };
        ui.label(tip);
    });
    if resp.clicked() && enabled {
        app.bar_anchor = Some(rect.left_bottom());
        if on && matches!(b.action(), Action::Internal(ref s) if s.as_slice() == [Step::Cmd(Cmd::FtpConnect)]) {
            app.pending.push(Cmd::FtpDisconnect);
        } else {
            app.pending.push(Cmd::RunButton { button: Box::new(b), dropped: Vec::new() });
        }
    }
    if resp.secondary_clicked() {
        let at = resp.interact_pointer_pos().unwrap_or(rect.left_bottom());
        app.bar_menu(kind, Some(index), at);
    }
}

/// The bar under the main menu; rows break at TC's "-2" entries and wrap
/// when the window is narrow.
pub fn draw_main(app: &mut App, ui: &mut Ui) {
    let pal = app.pal.clone();
    let area = ui.max_rect();
    app.bar_areas[BarKind::Main.index()] = area;
    let bg = ui.interact(area, egui::Id::unique("main-bar-bg"), Sense::click());
    if bg.secondary_clicked() {
        let at = bg.interact_pointer_pos().unwrap_or(area.left_bottom());
        app.bar_menu(BarKind::Main, None, at);
    }
    let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
    for (i, b) in app.bars[0].buttons.iter().enumerate() {
        if b.is_break() {
            rows.push(Vec::new());
        } else {
            rows.last_mut().unwrap().push(i);
        }
    }
    ui.spacing_mut().item_spacing = vec2(1.0, 1.0);
    ui.vertical(|ui| {
        for row in rows {
            ui.horizontal_wrapped(|ui| {
                ui.set_min_height(MAIN_BUTTON);
                for i in row {
                    if app.bars[0].buttons[i].is_separator() {
                        let (r, _) = ui.allocate_exact_size(vec2(9.0, MAIN_BUTTON), Sense::hover());
                        ui.painter().vline(r.center().x, r.y_range().shrink(3.0), Stroke::new(1.0, pal.col_sep));
                        continue;
                    }
                    let (r, _) = ui.allocate_exact_size(vec2(MAIN_BUTTON, MAIN_BUTTON), Sense::hover());
                    bar_button(app, ui, r, BarKind::Main, i, 4.0);
                }
            });
        }
    });
}

/// The bar between the panels, from `top` down.
pub fn draw_vertical(app: &mut App, ui: &mut Ui, rect: Rect, top: f32) {
    app.bar_areas[BarKind::Vertical.index()] = rect;
    let mut y = top;
    for i in 0..app.bars[1].buttons.len() {
        if app.bars[1].buttons[i].is_spacer() {
            y += VERT_STEP;
            continue;
        }
        let r = Rect::from_center_size(pos2(rect.center().x, y + VERT_STEP / 2.0 - 4.0), vec2(VERT_BUTTON, VERT_BUTTON));
        if r.bottom() > rect.bottom() {
            break;
        }
        bar_button(app, ui, r, BarKind::Vertical, i, 5.0);
        y += VERT_STEP;
    }
}

/// Right-click on the panel divider: the vertical bar's menu.
pub fn vertical_bar_menu(app: &mut App, at: Pos2) {
    app.bar_menu(BarKind::Vertical, None, at);
}

// ---- "Change button bar" dialog ------------------------------------------------

pub struct BarEditor {
    pub kind: BarKind,
    pub bar: ButtonBar,
    pub sel: Option<usize>,
    /// Filter text of the command list; `None` while the list is closed.
    pub choose: Option<String>,
    /// Frames left to put the keyboard into the command filter (the click
    /// that opened the list takes focus back once).
    focus_choose: u8,
    /// Icon picker for the icon file is open.
    pub pick_icon: bool,
}

impl BarEditor {
    pub fn new(kind: BarKind, bar: ButtonBar, sel: Option<usize>) -> BarEditor {
        let sel = sel.or((!bar.buttons.is_empty()).then_some(0));
        BarEditor { kind, bar, sel, choose: None, focus_choose: 0, pick_icon: false }
    }
}

/// Draws the dialog; returns false when it is closed.
pub fn draw_editor(app: &mut App, ctx: &egui::Context, ed: &mut BarEditor) -> bool {
    let pal = app.pal.clone();
    let title = format!("Change {}", ed.kind.title());
    let file = app.bar_file(ed.kind);
    let (r, esc) = crate::dialogs::modal(ctx, "dlg-bar", &title, &pal, 700.0, |ui| {
        ui.label(RichText::new(format!("File: {}", file.display())).weak().small());
        ui.add_space(4.0);
        // The bar in raw form: click to select, Shift+Left/Right moves.
        egui::Frame::new().fill(pal.bar).stroke(Stroke::new(1.0, pal.border)).inner_margin(4.0).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
            ui.horizontal_wrapped(|ui| {
                for i in 0..ed.bar.buttons.len() {
                    let b = ed.bar.buttons[i].clone();
                    let w = if b.is_spacer() { 10.0 } else { 28.0 };
                    let (r, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
                    if ed.sel == Some(i) {
                        ui.painter().rect(r, 2.0, pal.cursor_fill, Stroke::new(1.0, pal.accent), StrokeKind::Inside);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(r, 2.0, pal.hover);
                    }
                    if b.is_break() {
                        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "\u{21B5}", egui::FontId::proportional(11.0), pal.text_dim);
                    } else if b.is_separator() {
                        ui.painter().vline(r.center().x, r.y_range().shrink(4.0), Stroke::new(1.0, pal.col_sep));
                    } else {
                        paint_button_icon(app, ui.painter(), r.shrink(4.0), &b);
                    }
                    if resp.clicked() {
                        ed.sel = Some(i);
                        ed.pick_icon = false;
                    }
                }
            });
        });
        // Shift+arrows move the button, unless a text field has the keyboard.
        let typing = ui.memory(|m| m.focused().is_some());
        let (shift_left, shift_right) = if typing {
            (false, false)
        } else {
            ui.input_mut(|i| (i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowLeft), i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowRight)))
        };
        ui.add_space(4.0);
        let n = ed.bar.buttons.len();
        ui.horizontal(|ui| {
            let after = ed.sel.map_or(n, |s| s + 1);
            if ui.button("Add").clicked() {
                ed.bar.buttons.insert(after, BarButton::command("cm_RereadSource"));
                ed.sel = Some(after);
                ed.choose = Some(String::new());
                ed.focus_choose = 2;
            }
            if ui.button("Add separator").clicked() {
                ed.bar.buttons.insert(after, BarButton::default());
                ed.sel = Some(after);
            }
            if ui.button(if ed.kind == BarKind::Main { "Add line break" } else { "Add gap" }).clicked() {
                ed.bar.buttons.insert(after, BarButton::command("-2"));
                ed.sel = Some(after);
            }
            let has = ed.sel.is_some_and(|s| s < n);
            if ui.add_enabled(has, egui::Button::new("Delete")).clicked() {
                let s = ed.sel.unwrap();
                ed.bar.buttons.remove(s);
                ed.sel = if ed.bar.buttons.is_empty() { None } else { Some(s.min(ed.bar.buttons.len() - 1)) };
            }
            if (ui.add_enabled(ed.sel.is_some_and(|s| s > 0 && s < n), egui::Button::new("\u{25C0} Move")).clicked() || shift_left) && ed.sel.is_some_and(|s| s > 0 && s < n) {
                let s = ed.sel.unwrap();
                ed.bar.buttons.swap(s, s - 1);
                ed.sel = Some(s - 1);
            }
            if (ui.add_enabled(ed.sel.is_some_and(|s| s + 1 < n), egui::Button::new("Move \u{25B6}")).clicked() || shift_right) && ed.sel.is_some_and(|s| s + 1 < n) {
                let s = ed.sel.unwrap();
                ed.bar.buttons.swap(s, s + 1);
                ed.sel = Some(s + 1);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Reset to default").on_hover_text("MCommander's own bar").clicked() {
                    ed.bar = ButtonBar::default_for(ed.kind);
                    ed.sel = Some(0);
                }
            });
        });
        ui.add_space(6.0);
        match ed.sel.filter(|&s| s < ed.bar.buttons.len()) {
            None => {
                ui.label(RichText::new("The bar is empty. Press Add to create a button.").weak());
            }
            Some(s) if ed.bar.buttons[s].is_spacer() => {
                ui.label(RichText::new("A separator (or line break) has no settings.").weak());
            }
            Some(s) => button_fields(app, ui, ed, s),
        }
        crate::dialogs::buttons(ui, &["OK", "Cancel"])
    });
    match (r, esc) {
        (Some(0), _) => {
            let (kind, bar) = (ed.kind, ed.bar.clone());
            app.set_bar(kind, bar);
            false
        }
        (Some(_), _) | (_, true) => false,
        _ => true,
    }
}

fn button_fields(app: &mut App, ui: &mut Ui, ed: &mut BarEditor, s: usize) {
    let pal = app.pal.clone();
    let field_w = 470.0;
    egui::Grid::new("bar-fields").num_columns(2).spacing([10.0, 5.0]).show(ui, |ui| {
        let b = &mut ed.bar.buttons[s];
        ui.label("Command:");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut b.cmd).desired_width(field_w).hint_text("cm_Copy, a program, cd c:\\dir or a .bar file"));
            if ui.button("Choose...").on_hover_text("Internal commands").clicked() {
                ed.choose = if ed.choose.is_some() { None } else { Some(String::new()) };
                ed.focus_choose = 2;
            }
        });
        ui.end_row();
        ui.label("Parameters:");
        ui.add(egui::TextEdit::singleline(&mut b.params).desired_width(field_w).hint_text("%P%N = file under the cursor, %S = selected files, ? = ask first"));
        ui.end_row();
        ui.label("Start path:");
        ui.add(egui::TextEdit::singleline(&mut b.path).desired_width(field_w).hint_text("Empty: the current directory"));
        ui.end_row();
        ui.label("Icon file:");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut b.icon).desired_width(field_w).hint_text("Empty: from the command (an .exe, .dll or .ico, with ,index)"));
            if ui.button("Icons...").clicked() {
                ed.pick_icon = !ed.pick_icon;
            }
        });
        ui.end_row();
        ui.label("Tooltip:");
        ui.add(egui::TextEdit::singleline(&mut b.tip).desired_width(field_w).hint_text("Empty: describes the command; | starts a new line"));
        ui.end_row();
        ui.label("Run:");
        ui.horizontal(|ui| {
            ui.radio_value(&mut b.iconic, 0, "Normal");
            ui.radio_value(&mut b.iconic, 1, "Minimized");
            ui.radio_value(&mut b.iconic, -1, "Maximized");
        });
        ui.end_row();
    });
    if let Some(filter) = ed.choose.as_mut() {
        ui.add_space(4.0);
        let id = egui::Id::unique("bar-cmd-filter");
        if ed.focus_choose > 0 {
            ed.focus_choose -= 1;
            ui.memory_mut(|m| m.request_focus(id));
        }
        let entered = ui
            .horizontal(|ui| {
                ui.label("Find command:");
                let r = ui.add(egui::TextEdit::singleline(filter).id(id).desired_width(300.0).hint_text("Name or description; Enter takes the first"));
                r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
            })
            .inner;
        let f = filter.to_lowercase();
        let matches: Vec<&crate::commands::TcCommand> =
            crate::commands::tc_commands().iter().filter(|c| f.is_empty() || c.name.to_lowercase().contains(&f) || c.help.to_lowercase().contains(&f)).collect();
        let mut picked = if entered { matches.first().map(|c| c.name) } else { None };
        egui::ScrollArea::vertical().max_height(180.0).id_salt("bar-cmds").show(ui, |ui| {
            egui::Grid::new("bar-cmd-grid").num_columns(2).spacing([16.0, 2.0]).show(ui, |ui| {
                for c in &matches {
                    if ui.selectable_label(false, c.name).clicked() {
                        picked = Some(c.name);
                    }
                    ui.label(RichText::new(c.help).weak());
                    ui.end_row();
                }
            });
        });
        if let Some(name) = picked {
            let b = &mut ed.bar.buttons[s];
            b.cmd = name.to_string();
            // Use MCommander's picture for the new command.
            b.icon.clear();
            ed.choose = None;
        }
    }
    if ed.pick_icon {
        ui.add_space(4.0);
        let spec = ed.bar.buttons[s].icon.trim().to_string();
        let file = spec.rsplit_once(',').filter(|(_, i)| i.trim().parse::<i32>().is_ok()).map_or(spec.as_str(), |(f, _)| f).trim().to_string();
        let is_wcm = Path::new(&file).file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("wcmicons.dll"));
        let mut picked: Option<String> = None;
        egui::ScrollArea::vertical().max_height(150.0).id_salt("bar-icons").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if file.is_empty() || is_wcm {
                    // MCommander's own pictures, stored under TC's icon numbers.
                    for (index, icon) in crate::buttonbar::wcm_icons() {
                        let (r, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(r, 2.0, pal.hover);
                        }
                        paint_icons::paint(ui.painter(), r.shrink(4.0), icon, pal.text);
                        if resp.clicked() {
                            picked = Some(format!("wcmicons.dll,{index}"));
                        }
                    }
                } else {
                    let path = PathBuf::from(crate::vfs::expand_env(&file));
                    let count = crate::win::icon_count(&path.to_string_lossy()).min(300);
                    if count == 0 {
                        ui.label(RichText::new("No icons found in this file.").weak());
                    }
                    for index in 0..count as i32 {
                        let (r, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(r, 2.0, pal.hover);
                        }
                        let px = (22.0 * app.ppp).round() as u32;
                        if let Some(t) = app.icons.for_resource(&path, index, px) {
                            ui.painter().image(t, r.shrink(4.0), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                        }
                        if resp.clicked() {
                            picked = Some(format!("{file},{index}"));
                        }
                    }
                }
            });
        });
        if let Some(p) = picked {
            ed.bar.buttons[s].icon = p;
            ed.pick_icon = false;
        }
    }
}

/// "?" parameters: edit them before the program starts.
pub fn draw_run_params(app: &mut App, ctx: &egui::Context, run: &mut PendingRun) -> bool {
    let pal = app.pal.clone();
    let (r, esc) = crate::dialogs::modal(ctx, "dlg-run-params", "Parameters", &pal, 520.0, |ui| {
        ui.label(format!("Start {} with:", run.program));
        let id = egui::Id::unique("dlg-run-params-text");
        let len = run.args.chars().count();
        crate::dialogs::focus_select_modal(ui, id, std::mem::take(&mut run.focus), len, len);
        ui.add(egui::TextEdit::singleline(&mut run.args).id(id).desired_width(f32::INFINITY));
        let b = crate::dialogs::buttons(ui, &["OK", "Cancel"]);
        if crate::dialogs::enter(ui) { Some(0) } else { b }
    });
    match (r, esc) {
        (Some(0), _) => {
            let r = PendingRun { program: run.program.clone(), args: run.args.clone(), dir: run.dir.clone(), show: run.show, admin: run.admin, list_file: run.list_file.take(), focus: false };
            app.launch(r);
            false
        }
        (Some(_), _) | (_, true) => {
            if let Some(f) = run.list_file.take() {
                let _ = std::fs::remove_file(f);
            }
            false
        }
        _ => true,
    }
}
