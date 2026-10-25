//! Main window layout and the custom-drawn file panels.

use crate::app::{App, DragState, Preview};
use crate::commands::{self, Cmd};
use crate::config::ViewMode;
use crate::format;
use crate::paint_icons::{self, Icon};
use crate::panel::{FileFilter, Side};
use crate::theme::Palette;
use crate::vfs::{FileEntry, Location, SortColumn};
use egui::{Align2, Color32, CursorIcon, FontId, Key, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

const DRIVEBAR_H: f32 = 25.0;
const INFO_H: f32 = 23.0;
const TABS_H: f32 = 21.0;
const HEADER_H: f32 = 18.0;
const COLHDR_H: f32 = 19.0;
const STATUS_H: f32 = 19.0;
const MIDBAR_W: f32 = 36.0;
const SCROLL_W: f32 = 16.0;

pub fn row_height(app: &App) -> f32 {
    row_h(app)
}

fn row_h(app: &App) -> f32 {
    (app.cfg.font_size + 4.5).round()
}

fn font(app: &App) -> FontId {
    FontId::proportional(app.cfg.font_size)
}

pub fn draw(app: &mut App, ui: &mut Ui) {
    let pal = app.pal.clone();
    egui::Panel::top("menubar")
        .frame(egui::Frame::new().fill(pal.bar).inner_margin(egui::Margin::symmetric(2, 1)))
        .show(ui, |ui| menu_bar(app, ui));
    if app.cfg.show_toolbar {
        egui::Panel::top("toolbar")
            .frame(egui::Frame::new().fill(pal.bar).inner_margin(egui::Margin::symmetric(4, 3)))
            .show(ui, |ui| toolbar(app, ui));
    }
    if app.cfg.show_fkeys {
        egui::Panel::bottom("fkeys")
            .frame(egui::Frame::new().fill(pal.face).inner_margin(egui::Margin::symmetric(0, 0)))
            .exact_size(24.0)
            .show(ui, |ui| fkey_bar(app, ui));
    }
    if app.cfg.show_cmdline {
        egui::Panel::bottom("cmdline")
            .frame(egui::Frame::new().fill(pal.face).inner_margin(egui::Margin { left: 4, right: 4, top: 3, bottom: 2 }))
            .show(ui, |ui| command_line(app, ui));
    }
    egui::CentralPanel::no_frame().show(ui, |ui| {
        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0.0, pal.face);
        // Separator under the toolbar, like the Windows toolbar edge.
        ui.painter().hline(full.x_range(), full.top(), Stroke::new(1.0, pal.border.gamma_multiply(0.5)));
        let mid_w = if app.cfg.show_middle_bar { MIDBAR_W } else { 6.0 };
        let avail = full.width() - mid_w;
        let left_w = (avail * app.cfg.divider).round();
        let left = Rect::from_min_size(full.min, vec2(left_w, full.height()));
        let mid = Rect::from_min_size(pos2(left.right(), full.top()), vec2(mid_w, full.height()));
        let right = Rect::from_min_max(pos2(mid.right(), full.top()), full.max);
        draw_half(app, ui, left, Side::Left);
        draw_half(app, ui, right, Side::Right);
        middle_bar(app, ui, mid, full);
        draw_drag_overlay(app, ui);
        handle_external_drop(app, ui);
        if let Some(t) = &app.toast {
            let galley = ui.painter().layout_no_wrap(t.text.clone(), font(app), if t.error { pal.error } else { pal.text });
            let size = galley.size() + vec2(20.0, 10.0);
            let r = Rect::from_center_size(pos2(full.center().x, full.bottom() - 40.0), size);
            ui.painter().rect(r, 4.0, pal.bar, Stroke::new(1.0, pal.border), StrokeKind::Inside);
            ui.painter().galley(r.min + vec2(10.0, 5.0), galley, pal.text);
        }
    });
}

// ---- menu bar ---------------------------------------------------------------------

fn checked(app: &App, cmd: &Cmd) -> Option<bool> {
    let tab = app.atab();
    Some(match cmd {
        Cmd::Brief => tab.view == ViewMode::Brief,
        Cmd::Full => tab.view == ViewMode::Full,
        Cmd::SortBy(c) => tab.sort.column == *c,
        Cmd::ReverseSort => tab.sort.reverse,
        Cmd::ShowAll => tab.filter == FileFilter::All,
        Cmd::ShowPrograms => tab.filter == FileFilter::Programs,
        Cmd::ShowCustom => matches!(tab.filter, FileFilter::Custom(_)),
        Cmd::ToggleHidden => app.cfg.show_hidden,
        Cmd::QuickView => app.quick_view,
        Cmd::BranchView => tab.branch,
        Cmd::QuickFilter => tab.quick_filter.is_some(),
        Cmd::ToggleTheme => app.dark,
        _ => return None,
    })
}

fn menu_items(app: &mut App, ui: &mut Ui, items: Vec<commands::MenuItem>) {
    ui.set_min_width(260.0);
    for it in items {
        let Some(cmd) = it.cmd else {
            ui.separator();
            continue;
        };
        let check = checked(app, &cmd);
        let label = match check {
            Some(true) => format!("\u{2714}  {}", it.label),
            Some(false) => format!("     {}", it.label),
            None => format!("     {}", it.label),
        };
        let btn = egui::Button::new(label).shortcut_text(it.shortcut).frame_when_inactive(false);
        if ui.add(btn).clicked() {
            app.pending.push(cmd);
            ui.close();
        }
    }
}

fn menu_bar(app: &mut App, ui: &mut Ui) {
    egui::MenuBar::new().ui(ui, |ui| {
        for (title, items) in commands::main_menu() {
            ui.menu_button(title, |ui| menu_items(app, ui, items));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.menu_button("Help", |ui| menu_items(app, ui, commands::help_menu()));
        });
    });
}

// ---- toolbar ------------------------------------------------------------------------

fn tool_button(app: &mut App, ui: &mut Ui, icon: Icon, tip: &str, cmd: Cmd, on: bool) {
    let size = vec2(28.0, 28.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let pal = &app.pal;
    if on || resp.hovered() {
        let fill = if on { pal.pressed } else { pal.hover };
        ui.painter().rect(rect, 2.0, fill, Stroke::new(1.0, pal.button_border), StrokeKind::Inside);
    }
    paint_icons::paint(ui.painter(), rect.shrink(5.0), icon, pal.text);
    if resp.on_hover_text(tip).clicked() {
        app.pending.push(cmd);
    }
}

fn tool_sep(ui: &mut Ui, pal: &Palette) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 28.0), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range().shrink(3.0), Stroke::new(1.0, pal.col_sep));
}

fn toolbar(app: &mut App, ui: &mut Ui) {
    let pal = app.pal.clone();
    ui.spacing_mut().item_spacing.x = 1.0;
    ui.horizontal(|ui| {
        let brief = app.atab().view == ViewMode::Brief;
        tool_button(app, ui, Icon::Refresh, "Reread source (Ctrl+R)", Cmd::Reread, false);
        tool_sep(ui, &pal);
        tool_button(app, ui, Icon::Brief, "Brief view (Ctrl+F1)", Cmd::Brief, brief);
        tool_button(app, ui, Icon::Full, "Full view (Ctrl+F2)", Cmd::Full, !brief);
        tool_button(app, ui, Icon::QuickView, "Quick view panel (Ctrl+Q)", Cmd::QuickView, app.quick_view);
        tool_sep(ui, &pal);
        let branch = app.atab().branch;
        tool_button(app, ui, Icon::Branch, "Branch view: all files in subdirectories (Ctrl+B)", Cmd::BranchView, branch);
        tool_sep(ui, &pal);
        tool_button(app, ui, Icon::Invert, "Invert selection (Num *)", Cmd::InvertSelection, false);
        tool_sep(ui, &pal);
        tool_button(app, ui, Icon::Back, "Back (Alt+Left)", Cmd::HistoryBack, false);
        tool_button(app, ui, Icon::Forward, "Forward (Alt+Right)", Cmd::HistoryForward, false);
        tool_sep(ui, &pal);
        tool_button(app, ui, Icon::Pack, "Pack files (Alt+F5)", Cmd::Pack, false);
        tool_button(app, ui, Icon::Unpack, "Unpack files (Alt+F9)", Cmd::Unpack, false);
        tool_sep(ui, &pal);
        tool_button(app, ui, Icon::Search, "Find files (Alt+F7)", Cmd::Search, false);
        tool_button(app, ui, Icon::MultiRename, "Multi-rename tool (Ctrl+M)", Cmd::MultiRename, false);
        tool_button(app, ui, Icon::Compare, "Synchronize directories", Cmd::SyncDirs, false);
        tool_button(app, ui, Icon::CopyNames, "Copy names with path to clipboard", Cmd::CopyFullNames, false);
        tool_sep(ui, &pal);
        let hidden = app.cfg.show_hidden;
        tool_button(app, ui, Icon::Hidden, "Show hidden/system files (Ctrl+H)", Cmd::ToggleHidden, hidden);
        tool_button(app, ui, Icon::Hotlist, "Directory hotlist (Ctrl+D)", Cmd::Hotlist, false);
        tool_sep(ui, &pal);
        tool_button(app, ui, Icon::Terminal, "Open command prompt here", Cmd::OpenTerminal, false);
        tool_button(app, ui, Icon::Notepad, "Notepad", Cmd::Run("notepad.exe".into()), false);
        tool_sep(ui, &pal);
        tool_button(app, ui, Icon::Theme, "Dark <-> normal", Cmd::ToggleTheme, false);
    });
}

// ---- F-key bar and command line ---------------------------------------------------

fn fkey_bar(app: &mut App, ui: &mut Ui) {
    let pal = app.pal.clone();
    let m = ui.input(|i| i.modifiers);
    let keys: [(&str, Cmd); 7] = if m.shift && !m.alt {
        [
            ("F3 View", Cmd::View),
            ("Shift+F4 New file", Cmd::EditNew),
            ("Shift+F5 Copy here", Cmd::CopySame),
            ("Shift+F6 Rename", Cmd::Rename),
            ("F7 NewFolder", Cmd::MkDir),
            ("Shift+F8 Delete!", Cmd::DeleteNow),
            ("Alt+F4 Exit", Cmd::Exit),
        ]
    } else if m.alt && !m.shift {
        [
            ("Alt+F1 Left drive", Cmd::DriveMenu(Side::Left)),
            ("Alt+F2 Right drive", Cmd::DriveMenu(Side::Right)),
            ("Alt+F5 Pack", Cmd::Pack),
            ("Alt+F7 Search", Cmd::Search),
            ("Alt+F8 History", Cmd::CmdHistory),
            ("Alt+F9 Unpack", Cmd::Unpack),
            ("Alt+F4 Exit", Cmd::Exit),
        ]
    } else {
        [
            ("F3 View", Cmd::View),
            ("F4 Edit", Cmd::Edit),
            ("F5 Copy", Cmd::Copy),
            ("F6 Move", Cmd::Move),
            ("F7 NewFolder", Cmd::MkDir),
            ("F8 Delete", Cmd::Delete),
            ("Alt+F4 Exit", Cmd::Exit),
        ]
    };
    let full = ui.max_rect();
    let w = full.width() / keys.len() as f32;
    for (i, (label, cmd)) in keys.into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(full.left() + i as f32 * w, full.top()), vec2(w, full.height()));
        let resp = ui.allocate_rect(r, Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(r.shrink(1.0), 2.0, pal.hover);
        }
        if i > 0 {
            ui.painter().vline(r.left(), r.y_range().shrink(4.0), Stroke::new(1.0, pal.border));
        }
        ui.painter().text(r.center(), Align2::CENTER_CENTER, label, font(app), pal.text);
        if resp.clicked() {
            app.pending.push(cmd);
        }
    }
}

fn command_line(app: &mut App, ui: &mut Ui) {
    let prompt = match app.atab().loc.host_dir() {
        Some(_) if !matches!(app.atab().loc, Location::Fs(_)) => format!("{}>", app.atab().loc.host_dir().unwrap().display()),
        _ => format!("{}>", app.atab().loc.display().trim_end_matches('\\')),
    };
    let prompt = if prompt.len() == 3 && prompt.ends_with(":>") { prompt.replace(":>", ":\\>") } else { prompt };
    let total = ui.available_width();
    ui.horizontal(|ui| {
        let mut galley = ui.painter().layout_no_wrap(prompt.clone(), font(app), app.pal.text);
        let label_w = galley.size().x.max(total * 0.33 - 8.0).min(total * 0.6);
        // Too long: keep the end of the path ("...\scratchpad\sbx\left>").
        let mut shown: Vec<char> = prompt.chars().collect();
        while galley.size().x > label_w - 4.0 && shown.len() > 4 {
            shown.drain(..2);
            galley = ui.painter().layout_no_wrap(format!("\u{2026}{}", shown.iter().collect::<String>()), font(app), app.pal.text);
        }
        let (r, _) = ui.allocate_exact_size(vec2(label_w, 20.0), Sense::hover());
        ui.painter().galley(pos2(r.right() - galley.size().x - 2.0, r.center().y - galley.size().y / 2.0), galley, app.pal.text);
        let id = egui::Id::unique("cmdline");
        let fnt = font(app);
        let edit = egui::TextEdit::singleline(&mut app.cmdline)
            .id(id)
            .desired_width(ui.available_width() - 22.0)
            .font(fnt)
            .margin(egui::Margin::symmetric(4, 2));
        let resp = ui.add(edit);
        if resp.clicked() {
            app.cmd_active = true;
        }
        let keys_free = app.panel_keys_active();
        if keys_free && !resp.has_focus() {
            resp.request_focus();
        }
        let (dr, dresp) = ui.allocate_exact_size(vec2(18.0, 20.0), Sense::click());
        if dresp.hovered() {
            ui.painter().rect_filled(dr, 2.0, app.pal.hover);
        }
        ui.painter().text(dr.center(), Align2::CENTER_CENTER, "\u{2304}", font(app), app.pal.text);
        if dresp.clicked() {
            app.pending.push(Cmd::CmdHistory);
        }
    });
}

// ---- middle button bar ------------------------------------------------------------------

fn middle_bar(app: &mut App, ui: &mut Ui, rect: Rect, full: Rect) {
    let pal = app.pal.clone();
    let resp = ui.interact(rect, egui::Id::unique("divider"), Sense::click_and_drag());
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    if resp.dragged() {
        if let Some(p) = resp.interact_pointer_pos() {
            let avail = full.width() - rect.width();
            app.cfg.divider = ((p.x - full.left() - rect.width() / 2.0) / avail).clamp(0.1, 0.9);
        }
    }
    if resp.double_clicked() {
        app.cfg.divider = 0.5;
    }
    if !app.cfg.show_middle_bar {
        return;
    }
    let mut y = rect.top() + DRIVEBAR_H + INFO_H + HEADER_H + COLHDR_H + 8.0;
    let buttons: [Option<(Icon, &str, Cmd)>; 7] = [
        Some((Icon::View, "View (F3)", Cmd::View)),
        Some((Icon::Edit, "Edit (F4)", Cmd::Edit)),
        Some((Icon::Copy, "Copy (F5)", Cmd::Copy)),
        Some((Icon::Move, "Move/rename (F6)", Cmd::Move)),
        None,
        Some((Icon::Pack, "Pack (Alt+F5)", Cmd::Pack)),
        Some((Icon::MkDir, "New folder (F7)", Cmd::MkDir)),
    ];
    for b in buttons {
        let Some((icon, tip, cmd)) = b else {
            y += 32.0;
            continue;
        };
        let r = Rect::from_center_size(pos2(rect.center().x, y + 12.0), vec2(28.0, 28.0));
        let br = ui.interact(r, egui::Id::unique(("midbtn", tip)), Sense::click());
        if br.hovered() {
            ui.painter().rect(r, 2.0, pal.hover, Stroke::new(1.0, pal.button_border), StrokeKind::Inside);
        }
        paint_icons::paint(ui.painter(), r.shrink(5.0), icon, pal.text);
        if br.on_hover_text(tip).clicked() {
            app.pending.push(cmd);
        }
        y += 32.0;
    }
}

// ---- one panel half ----------------------------------------------------------------------

fn draw_half(app: &mut App, ui: &mut Ui, rect: Rect, side: Side) {
    app.panel_rects[side.index()] = rect;
    let mut y = rect.top();
    if app.cfg.show_drive_buttons {
        drive_bar(app, ui, Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), DRIVEBAR_H)), side);
        y += DRIVEBAR_H;
    }
    info_line(app, ui, Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), INFO_H)), side);
    y += INFO_H;
    if app.panels[side.index()].tabs.len() > 1 {
        tab_bar(app, ui, Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), TABS_H)), side);
        y += TABS_H;
    }
    path_header(app, ui, Rect::from_min_size(pos2(rect.left() + 2.0, y), vec2(rect.width() - 4.0, HEADER_H)), side);
    y += HEADER_H + 1.0;
    let status_h = if app.cfg.show_status { STATUS_H } else { 0.0 };
    let body = Rect::from_min_max(pos2(rect.left() + 2.0, y), pos2(rect.right() - 2.0, rect.bottom() - status_h - 2.0));
    let pal = app.pal.clone();
    ui.painter().rect_filled(body, 0.0, pal.list_bg);
    ui.painter().rect_stroke(body, 0.0, Stroke::new(1.0, pal.border), StrokeKind::Outside);
    if app.quick_view && side != app.active {
        quick_view(app, ui, body);
    } else {
        let view = app.tab(side).view;
        let list_top = if view == ViewMode::Full {
            column_header(app, ui, Rect::from_min_size(body.min, vec2(body.width(), COLHDR_H)), side);
            body.top() + COLHDR_H
        } else {
            body.top()
        };
        file_list(app, ui, Rect::from_min_max(pos2(body.left(), list_top), body.max), side);
    }
    if app.cfg.show_status {
        status_line(app, ui, Rect::from_min_max(pos2(rect.left() + 2.0, rect.bottom() - STATUS_H), pos2(rect.right() - 2.0, rect.bottom() - 1.0)), side);
    }
}

fn drive_bar(app: &mut App, ui: &mut Ui, rect: Rect, side: Side) {
    let pal = app.pal.clone();
    ui.painter().rect_filled(rect, 0.0, pal.bar);
    let current = app.tab(side).loc.drive_letter();
    let mut x = rect.left() + 2.0;
    let drives = app.drives.clone();
    for d in &drives {
        let label = d.letter.to_ascii_lowercase().to_string();
        let w = 40.0;
        let r = Rect::from_min_size(pos2(x, rect.top() + 2.0), vec2(w, rect.height() - 4.0));
        let resp = ui.interact(r, egui::Id::unique(("drive", side.index(), d.letter)), Sense::click());
        let on = current == Some(d.letter);
        if on {
            ui.painter().rect_filled(r, 2.0, pal.pressed);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 2.0, pal.hover);
        }
        let icon_r = Rect::from_min_size(pos2(r.left() + 3.0, r.center().y - 8.0), vec2(16.0, 16.0));
        match app.icons.for_drive(d.letter) {
            Some(tex) => {
                ui.painter().image(tex, icon_r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => paint_icons::paint(ui.painter(), icon_r, Icon::File, pal.text),
        }
        ui.painter().text(pos2(icon_r.right() + 5.0, r.center().y), Align2::LEFT_CENTER, label, font(app), pal.text);
        if resp.clicked() {
            app.pending.push(Cmd::GotoDrive(side, d.letter));
        }
        x += w + 1.0;
    }
    // Network drives button.
    let r = Rect::from_min_size(pos2(x, rect.top() + 2.0), vec2(34.0, rect.height() - 4.0));
    let resp = ui.interact(r, egui::Id::unique(("netdrive", side.index())), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, 2.0, pal.hover);
    }
    paint_icons::paint(ui.painter(), Rect::from_min_size(pos2(r.left() + 2.0, r.center().y - 8.0), vec2(16.0, 16.0)), Icon::Network, pal.text);
    ui.painter().text(pos2(r.left() + 21.0, r.center().y), Align2::LEFT_CENTER, "\\", font(app), pal.text);
    if resp.on_hover_text("Network drives").clicked() {
        app.active = side;
        app.pending.push(Cmd::NetworkDrives);
    }
}

fn info_line(app: &mut App, ui: &mut Ui, rect: Rect, side: Side) {
    let pal = app.pal.clone();
    let loc = app.tab(side).loc.clone();
    let letter = loc.drive_letter();
    // Drive combo.
    let combo = Rect::from_min_size(pos2(rect.left() + 2.0, rect.top() + 2.0), vec2(52.0, rect.height() - 4.0));
    let resp = ui.interact(combo, egui::Id::unique(("drivecombo", side.index())), Sense::click());
    ui.painter().rect(combo, 2.0, if resp.hovered() { pal.hover } else { pal.input_bg }, Stroke::new(1.0, pal.border), StrokeKind::Inside);
    if let Some(l) = letter {
        let icon_r = Rect::from_min_size(pos2(combo.left() + 3.0, combo.center().y - 8.0), vec2(16.0, 16.0));
        if let Some(tex) = app.icons.for_drive(l) {
            ui.painter().image(tex, icon_r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        ui.painter().text(pos2(icon_r.right() + 4.0, combo.center().y), Align2::LEFT_CENTER, l.to_ascii_lowercase(), font(app), pal.text);
        app.request_drive_info(l);
    }
    ui.painter().text(pos2(combo.right() - 9.0, combo.center().y), Align2::CENTER_CENTER, "\u{2304}", font(app), pal.text_dim);
    if resp.clicked() {
        app.pending.push(Cmd::DriveMenu(side));
    }
    // Volume label and free space.
    let text = match (&loc, letter) {
        (Location::Results { title, .. }, _) => title.clone(),
        (Location::Archive { archive, .. }, _) => {
            let size = std::fs::metadata(archive).map(|m| m.len()).unwrap_or(0);
            format!("[archive]  {} k packed", format::thousands(size / 1024))
        }
        (_, Some(l)) => match app.drive_extra.get(&l) {
            Some(x) => {
                let label = x.label.clone().unwrap_or_default();
                let label = if label.is_empty() { "_none_".into() } else { label };
                match x.space {
                    Some((free, total)) => format!("[{label}]  {} of {} free", format::kilo(free), format::kilo(total)),
                    None => format!("[{label}]"),
                }
            }
            None => String::new(),
        },
        _ => loc.display(),
    };
    let text_rect = Rect::from_min_max(pos2(combo.right() + 6.0, rect.top()), pos2(rect.right() - 60.0, rect.bottom()));
    ui.painter().with_clip_rect(text_rect).text(pos2(text_rect.left(), rect.center().y), Align2::LEFT_CENTER, text, font(app), pal.text);
    // Root and parent buttons.
    for (i, (label, cmd)) in [("\\", Cmd::Root), ("..", Cmd::Parent)].into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.right() - 54.0 + i as f32 * 26.0, rect.top() + 2.0), vec2(24.0, rect.height() - 4.0));
        let b = ui.interact(r, egui::Id::unique(("rootparent", side.index(), i)), Sense::click());
        if b.hovered() {
            ui.painter().rect(r, 2.0, pal.hover, Stroke::new(1.0, pal.button_border), StrokeKind::Inside);
        }
        ui.painter().text(r.center(), Align2::CENTER_CENTER, label, font(app), pal.text);
        if b.clicked() {
            app.active = side;
            app.pending.push(cmd);
        }
    }
    if i32::from(rect.width() > 0.0) > 0 {
        ui.painter().vline(rect.right() - 58.0, rect.y_range().shrink(4.0), Stroke::new(1.0, pal.col_sep));
    }
}

fn tab_bar(app: &mut App, ui: &mut Ui, rect: Rect, side: Side) {
    let pal = app.pal.clone();
    let panel_active = app.panels[side.index()].active;
    let captions: Vec<(String, bool)> = app.panels[side.index()].tabs.iter().map(|t| (t.caption(), t.locked)).collect();
    let mut x = rect.left() + 2.0;
    for (i, (cap, locked)) in captions.into_iter().enumerate() {
        let text = if locked { format!("\u{1F512}{cap}") } else { cap };
        let galley = ui.painter().layout_no_wrap(text, font(app), pal.text);
        let w = (galley.size().x + 16.0).clamp(40.0, 180.0);
        let r = Rect::from_min_size(pos2(x, rect.top() + 2.0), vec2(w, rect.height() - 2.0));
        let resp = ui.interact(r, egui::Id::unique(("tab", side.index(), i)), Sense::click());
        let fill = if i == panel_active { pal.tab_active } else if resp.hovered() { pal.hover } else { pal.tab_inactive };
        ui.painter().rect(r, egui::CornerRadius { nw: 3, ne: 3, sw: 0, se: 0 }, fill, Stroke::new(1.0, pal.border), StrokeKind::Inside);
        ui.painter().with_clip_rect(r.shrink(2.0)).galley(pos2(r.left() + 8.0, r.center().y - galley.size().y / 2.0), galley, pal.text);
        if resp.clicked() {
            app.panels[side.index()].active = i;
            app.active = side;
        }
        if resp.middle_clicked() || resp.double_clicked() {
            app.panels[side.index()].active = i;
            app.active = side;
            app.pending.push(Cmd::CloseTab);
        }
        resp.context_menu(|ui| {
            if ui.button(if locked { "Unlock tab" } else { "Lock tab" }).clicked() {
                let t = &mut app.panels[side.index()].tabs[i];
                t.locked = !t.locked;
                ui.close();
            }
            if ui.button("Close tab").clicked() {
                app.panels[side.index()].active = i;
                app.active = side;
                app.pending.push(Cmd::CloseTab);
                ui.close();
            }
        });
        x += w + 1.0;
        if x > rect.right() - 20.0 {
            break;
        }
    }
}

fn path_header(app: &mut App, ui: &mut Ui, rect: Rect, side: Side) {
    let pal = app.pal.clone();
    let active = app.active == side;
    ui.painter().rect_filled(rect, 0.0, if active { pal.header_active } else { pal.header_inactive });
    let tab = app.tab(side);
    let filter = match &tab.filter {
        FileFilter::All => "*.*".to_string(),
        FileFilter::Programs => "*.exe *.com *.bat *.cmd ...".to_string(),
        FileFilter::Custom(m) => m.clone(),
    };
    let mut text = format!("{}{}", tab.loc.display(), filter);
    if tab.branch {
        text.push_str("  [branch view]");
    }
    if let Some(q) = &tab.quick_filter {
        text.push_str(&format!("  [filter: {q}]"));
    }
    // Left: tiny triangle like TC (opens history).
    let tri = Rect::from_min_size(rect.min, vec2(14.0, rect.height()));
    let tri_resp = ui.interact(tri, egui::Id::unique(("hdrhist", side.index())), Sense::click());
    ui.painter().text(tri.center(), Align2::CENTER_CENTER, "\u{25BE}", font(app), pal.header_text);
    if tri_resp.clicked() {
        app.active = side;
        app.pending.push(Cmd::HistoryMenu);
    }
    let text_rect = Rect::from_min_max(pos2(tri.right(), rect.top()), pos2(rect.right() - 40.0, rect.bottom()));
    let resp = ui.interact(text_rect, egui::Id::unique(("hdrpath", side.index())), Sense::click());
    let galley = ui.painter().layout_no_wrap(text.clone(), font(app), pal.header_text);
    // Hovering a path component underlines it; clicking jumps there.
    let disp = app.tab(side).loc.display();
    let mut hover_target: Option<String> = None;
    if let (Some(p), Location::Fs(_)) = (resp.hover_pos(), &app.tab(side).loc) {
        let mut acc = String::new();
        for part in disp.split_inclusive('\\') {
            let before = ui.painter().layout_no_wrap(acc.clone(), font(app), pal.header_text).size().x;
            acc.push_str(part);
            let after = ui.painter().layout_no_wrap(acc.clone(), font(app), pal.header_text).size().x;
            if p.x >= text_rect.left() + before && p.x < text_rect.left() + after {
                ui.painter().hline(
                    (text_rect.left() + before)..=(text_rect.left() + after - 4.0),
                    rect.bottom() - 2.0,
                    Stroke::new(1.0, pal.header_text),
                );
                hover_target = Some(acc.clone());
            }
        }
    }
    ui.painter().with_clip_rect(text_rect).galley(pos2(text_rect.left(), rect.center().y - galley.size().y / 2.0), galley, pal.header_text);
    if resp.clicked() {
        app.active = side;
        if let Some(t) = hover_target {
            if t != disp {
                app.pending.push(Cmd::GotoPath(t));
            }
        }
    }
    // Right: hotlist (*) and history (down arrow).
    for (i, (label, cmd, tip)) in [("*", Cmd::Hotlist, "Directory hotlist (Ctrl+D)"), ("\u{25BC}", Cmd::HistoryMenu, "History (Alt+Down)")].into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.right() - 38.0 + i as f32 * 19.0, rect.top()), vec2(18.0, rect.height()));
        let b = ui.interact(r, egui::Id::unique(("hdrbtn", side.index(), i)), Sense::click());
        if b.hovered() {
            ui.painter().rect_filled(r, 0.0, pal.hover.gamma_multiply(0.6));
        }
        ui.painter().text(r.center(), Align2::CENTER_CENTER, label, FontId::proportional(app.cfg.font_size - 1.0), pal.header_text);
        if b.on_hover_text(tip).clicked() {
            app.active = side;
            app.pending.push(cmd);
        }
    }
}

/// Column widths: [name, ext, size, date, attr]. With the extension shown
/// as part of the name (TC's default look) the Ext column stays empty.
fn columns(app: &App, width: f32) -> [f32; 5] {
    let [ext, size, date, attr] = app.cfg.columns;
    let name = (width - ext - size - date - attr).max(60.0);
    [name, ext, size, date, attr]
}

fn column_header(app: &mut App, ui: &mut Ui, rect: Rect, side: Side) {
    let pal = app.pal.clone();
    let list_w = rect.width() - SCROLL_W;
    let cols = columns(app, list_w);
    let labels = ["Name", "Ext", "Size", "Date", "Attr"];
    let sorts = [SortColumn::Name, SortColumn::Ext, SortColumn::Size, SortColumn::Date, SortColumn::Attr];
    let sort = app.tab(side).sort;
    ui.painter().rect_filled(rect, 0.0, pal.col_header_bg);
    let mut x = rect.left();
    for i in 0..5 {
        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(cols[i], rect.height()));
        let resp = ui.interact(r.shrink2(vec2(3.0, 0.0)), egui::Id::unique(("colhdr", side.index(), i)), Sense::click());
        let sorted = sort.column == sorts[i];
        if sorted {
            ui.painter().rect_filled(r, 0.0, pal.col_header_sorted);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 0.0, pal.hover);
        }
        let descending = matches!(sorts[i], SortColumn::Size | SortColumn::Date) != sort.reverse;
        let label = if sorted { format!("{}{}", if descending { "\u{2193}" } else { "\u{2191}" }, labels[i]) } else { labels[i].to_string() };
        let align = if i == 2 { Align2::RIGHT_CENTER } else { Align2::LEFT_CENTER };
        let tx = if i == 2 { r.right() - 4.0 } else { r.left() + 4.0 };
        ui.painter().with_clip_rect(r).text(pos2(tx, r.center().y), align, label, font(app), pal.text);
        ui.painter().vline(r.right() - 0.5, r.y_range(), Stroke::new(1.0, pal.col_sep));
        if resp.clicked() {
            app.active = side;
            app.pending.push(Cmd::SortBy(sorts[i]));
        }
        // Resize handle on the left edge of Ext..Attr (resizing the column to its left).
        if i > 0 {
            let handle = Rect::from_center_size(pos2(r.left(), r.center().y), vec2(6.0, r.height()));
            let h = ui.interact(handle, egui::Id::unique(("colsize", side.index(), i)), Sense::drag());
            if h.hovered() || h.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeColumn);
            }
            if h.dragged() {
                let d = h.drag_delta().x;
                // Moving the boundary right grows the left column and shrinks this one.
                let ci = i - 1;
                if ci == 0 {
                    app.cfg.columns[0] = (app.cfg.columns[0] - d).clamp(20.0, 400.0);
                } else {
                    app.cfg.columns[ci - 1] = (app.cfg.columns[ci - 1] + d).clamp(20.0, 400.0);
                    app.cfg.columns[ci] = (app.cfg.columns[ci] - d).clamp(20.0, 400.0);
                }
            }
        }
        x += cols[i];
    }
    ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, pal.border));
}

fn entry_name(app: &App, e: &FileEntry) -> String {
    let name = e.name_shown();
    if e.is_dir && app.cfg.dir_brackets { format!("[{name}]") } else { name.to_string() }
}

fn draw_icon(app: &mut App, ui: &Ui, r: Rect, e: &FileEntry, full: Option<std::path::PathBuf>) {
    if e.is_parent {
        paint_icons::paint(ui.painter(), r, Icon::ParentDir, app.pal.text);
        return;
    }
    match app.icons.for_entry(e, full.as_deref()) {
        Some(tex) => {
            let tint = if e.is_hidden() { Color32::from_white_alpha(140) } else { Color32::WHITE };
            ui.painter().image(tex, r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
        }
        None => paint_icons::paint(ui.painter(), r, if e.is_dir { Icon::Folder } else { Icon::File }, app.pal.text),
    }
}

fn file_list(app: &mut App, ui: &mut Ui, rect: Rect, side: Side) {
    app.list_rects[side.index()] = rect;
    let pal = app.pal.clone();
    let rh = row_h(app);
    let view = app.tab(side).view;
    let list_rect = Rect::from_min_max(rect.min, pos2(rect.right() - if view == ViewMode::Full { SCROLL_W } else { 0.0 }, rect.bottom()));
    let rows = ((list_rect.height() - if view == ViewMode::Brief { SCROLL_W } else { 0.0 }) / rh).floor().max(1.0) as usize;
    let n = app.tab(side).entries.len();
    // Brief view lays entries out in columns of `rows` entries.
    let col_w = 190.0f32.min(list_rect.width());
    let ncols_vis = ((list_rect.width() / col_w).floor() as usize).max(1);
    {
        let tab = app.tab_mut(side);
        tab.page_rows = rows;
        if tab.follow_cursor {
            match view {
                ViewMode::Full => {
                    if tab.cursor < tab.top {
                        tab.top = tab.cursor;
                    } else if tab.cursor >= tab.top + rows {
                        tab.top = tab.cursor + 1 - rows;
                    }
                }
                ViewMode::Brief => {
                    let col = tab.cursor / rows;
                    let first = tab.top / rows;
                    if col < first {
                        tab.top = col * rows;
                    } else if col >= first + ncols_vis {
                        tab.top = (col + 1 - ncols_vis) * rows;
                    }
                }
            }
            tab.follow_cursor = false;
        }
        let max_top = match view {
            ViewMode::Full => n.saturating_sub(rows),
            ViewMode::Brief => (n.saturating_sub(1) / rows).saturating_sub(ncols_vis - 1) * rows,
        };
        tab.top = tab.top.min(max_top);
    }

    let resp = ui.interact(list_rect, egui::Id::unique(("list", side.index())), Sense::click_and_drag());
    // Mouse wheel scrolling.
    if resp.hovered() {
        let lines = wheel_lines(ui, rh, rows);
        if lines != 0 {
            let tab = app.tab_mut(side);
            let step = lines.unsigned_abs() * if view == ViewMode::Brief { rows } else { 1 };
            tab.top = if lines > 0 { tab.top.saturating_sub(step) } else { tab.top + step };
            let max_top = match view {
                ViewMode::Full => n.saturating_sub(rows),
                ViewMode::Brief => (n.saturating_sub(1) / rows).saturating_sub(ncols_vis - 1) * rows,
            };
            tab.top = tab.top.min(max_top);
        }
    }

    let index_at = |p: Pos2, top: usize| -> Option<usize> {
        if !list_rect.contains(p) {
            return None;
        }
        let row = ((p.y - list_rect.top()) / rh).floor() as usize;
        let i = match view {
            ViewMode::Full => top + row,
            ViewMode::Brief => {
                if row >= rows {
                    return None;
                }
                let col = ((p.x - list_rect.left()) / col_w).floor() as usize;
                top + col * rows + row
            }
        };
        (i < n).then_some(i)
    };

    let top = app.tab(side).top;
    // Clicks.
    if resp.clicked() || resp.secondary_clicked() || resp.double_clicked() || resp.drag_started() {
        app.active = side;
        app.quick = None;
        if app.rename.as_ref().is_some_and(|r| r.side == side) {
            app.finish_rename(true);
        }
    }
    if let Some(p) = resp.interact_pointer_pos() {
        let hit = index_at(p, top);
        let mods = ui.input(|i| i.modifiers);
        if resp.clicked() {
            if let Some(i) = hit {
                let tab = app.tab_mut(side);
                if mods.ctrl {
                    tab.toggle(i);
                    tab.anchor = Some(i);
                } else if mods.shift {
                    let a = tab.anchor.unwrap_or(tab.cursor);
                    tab.select_range(a, i, true);
                } else {
                    tab.anchor = Some(i);
                }
                tab.cursor = i;
            }
        }
        if resp.double_clicked() {
            if let Some(i) = hit {
                app.tab_mut(side).cursor = i;
                app.pending.push(Cmd::Open);
            }
        }
        if resp.secondary_clicked() {
            if let Some(i) = hit {
                app.tab_mut(side).cursor = i;
            }
            app.context_at = Some(p);
            app.pending.push(Cmd::ContextMenu);
        }
        if resp.drag_started() {
            if let Some(i) = hit {
                let tab = app.tab(side);
                let e = &tab.entries[i];
                if !e.is_parent {
                    let names = if e.selected {
                        tab.entries.iter().filter(|e| e.selected).map(|e| e.name.clone()).collect()
                    } else {
                        vec![e.name.clone()]
                    };
                    app.drag = Some(DragState { side, names, start: p, active: false });
                }
            }
        }
    }

    // Rows.
    let painter = ui.painter().with_clip_rect(list_rect);
    let active_panel = app.active == side;
    let renaming = app.rename.as_ref().filter(|r| r.side == side).map(|r| r.index);
    let cols = columns(app, list_rect.width());
    let fnt = font(app);
    let loc = app.tab(side).loc.clone();
    let visible: Vec<usize> = match view {
        ViewMode::Full => (top..(top + rows + 1).min(n)).collect(),
        ViewMode::Brief => (top..(top + rows * (ncols_vis + 1)).min(n)).collect(),
    };
    let cursor = app.tab(side).cursor;
    for i in visible {
        let e = app.tab(side).entries[i].clone();
        let (row_rect, name_w) = match view {
            ViewMode::Full => {
                let y = list_rect.top() + (i - top) as f32 * rh;
                (Rect::from_min_size(pos2(list_rect.left(), y), vec2(list_rect.width(), rh)), cols[0] + cols[1])
            }
            ViewMode::Brief => {
                let rel = i - top;
                let (col, row) = (rel / rows, rel % rows);
                let r = Rect::from_min_size(pos2(list_rect.left() + col as f32 * col_w, list_rect.top() + row as f32 * rh), vec2(col_w, rh));
                (r, col_w)
            }
        };
        if i == cursor && active_panel {
            painter.rect_filled(row_rect.shrink2(vec2(1.0, 0.0)), 0.0, pal.cursor_fill);
            painter.rect_stroke(row_rect.shrink2(vec2(1.0, 0.0)), 0.0, Stroke::new(1.0, pal.cursor_border), StrokeKind::Inside);
        } else if app.drag.as_ref().is_some_and(|d| d.active) && e.is_dir && !e.is_parent && ui.rect_contains_pointer(row_rect) {
            painter.rect_stroke(row_rect.shrink(1.0), 0.0, Stroke::new(1.0, pal.accent), StrokeKind::Inside);
        }
        let color = if e.selected { pal.marked } else if e.is_hidden() { pal.text_dim } else { pal.text };
        let icon_r = Rect::from_min_size(pos2(row_rect.left() + 3.0, row_rect.center().y - 8.0), vec2(16.0, 16.0));
        draw_icon(app, ui, icon_r, &e, loc.child_path(&e));
        let name_x = icon_r.right() + 4.0;
        if renaming == Some(i) {
            continue;
        }
        let name = entry_name(app, &e);
        let name_clip = Rect::from_min_max(pos2(name_x, row_rect.top()), pos2(row_rect.left() + name_w - 4.0, row_rect.bottom()));
        painter.with_clip_rect(name_clip.intersect(list_rect)).text(pos2(name_x, row_rect.center().y), Align2::LEFT_CENTER, name, fnt.clone(), color);
        if view == ViewMode::Full {
            let mut x = list_rect.left() + cols[0] + cols[1];
            let size_text = if e.is_dir {
                match e.dir_size {
                    Some(s) => format::thousands(s),
                    None => "<DIR>".into(),
                }
            } else {
                format::thousands(e.size)
            };
            painter.text(pos2(x + cols[2] - 4.0, row_rect.center().y), Align2::RIGHT_CENTER, size_text, fnt.clone(), color);
            x += cols[2];
            if let Some(t) = e.modified {
                painter.with_clip_rect(Rect::from_min_size(pos2(x, row_rect.top()), vec2(cols[3], rh))).text(pos2(x + 4.0, row_rect.center().y), Align2::LEFT_CENTER, format::datetime(t), fnt.clone(), color);
            }
            x += cols[3];
            painter.text(pos2(x + 4.0, row_rect.center().y), Align2::LEFT_CENTER, format::attr_string(e.attrs), fnt.clone(), color);
        }
    }
    if let Some(err) = app.tab(side).error.clone() {
        painter.text(list_rect.center(), Align2::CENTER_CENTER, err, fnt.clone(), pal.error);
    } else if n == 0 {
        painter.text(list_rect.center_top() + vec2(0.0, 30.0), Align2::CENTER_CENTER, "(empty)", fnt.clone(), pal.text_dim);
    }

    // Inline rename editor.
    if let Some(r) = app.rename.as_mut().filter(|r| r.side == side) {
        if r.index >= top && (view == ViewMode::Brief || r.index < top + rows) {
            let row_rect = match view {
                ViewMode::Full => Rect::from_min_size(pos2(list_rect.left() + 21.0, list_rect.top() + (r.index - top) as f32 * rh - 1.0), vec2(cols[0] + cols[1] - 22.0, rh + 2.0)),
                ViewMode::Brief => {
                    let rel = r.index - top;
                    Rect::from_min_size(pos2(list_rect.left() + (rel / rows) as f32 * col_w + 21.0, list_rect.top() + (rel % rows) as f32 * rh - 1.0), vec2(col_w - 22.0, rh + 2.0))
                }
            };
            let id = egui::Id::unique("inline-rename");
            if r.init {
                let stem = crate::format::split_ext(&r.text, false).0.chars().count();
                let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
                state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(stem))));
                state.store(ui.ctx(), id);
            }
            let resp = ui.put(row_rect, egui::TextEdit::singleline(&mut r.text).id(id).font(fnt.clone()).margin(egui::Margin::symmetric(2, 0)));
            if r.init {
                resp.request_focus();
                r.init = false;
            }
            if resp.lost_focus() {
                let esc = ui.input(|i| i.key_pressed(Key::Escape));
                app.finish_rename(!esc);
            }
        }
    }

    // Quick search box.
    if active_panel {
        if let Some(q) = &app.quick {
            let r = Rect::from_min_size(pos2(list_rect.left() + 6.0, list_rect.bottom() - rh - 8.0), vec2(220.0, rh + 4.0));
            ui.painter().rect(r, 2.0, pal.input_bg, Stroke::new(1.0, pal.accent), StrokeKind::Inside);
            ui.painter().text(pos2(r.left() + 6.0, r.center().y), Align2::LEFT_CENTER, format!("Search: {}", q.text), fnt.clone(), pal.text);
        }
    }

    // Scroll bar (full view: vertical; brief view: horizontal).
    match view {
        ViewMode::Full => {
            let track = Rect::from_min_max(pos2(list_rect.right(), rect.top()), rect.max);
            scrollbar(app, ui, track, side, n, rows, true);
        }
        ViewMode::Brief => {
            let track = Rect::from_min_max(pos2(rect.left(), rect.bottom() - SCROLL_W), rect.max);
            let total_cols = n.div_ceil(rows.max(1));
            scrollbar(app, ui, track, side, total_cols, ncols_vis, false);
        }
    }
}

fn scrollbar(app: &mut App, ui: &mut Ui, track: Rect, side: Side, total: usize, visible: usize, vertical: bool) {
    let pal = app.pal.clone();
    ui.painter().rect_filled(track, 0.0, pal.scrollbar_track);
    if total <= visible || total == 0 {
        return;
    }
    let rows = app.tab(side).page_rows.max(1);
    let unit = if vertical { 1 } else { rows };
    let pos = app.tab(side).top / unit;
    let len = if vertical { track.height() } else { track.width() };
    let thumb_len = (len * visible as f32 / total as f32).max(20.0);
    let max_pos = total - visible;
    let offset = (len - thumb_len) * pos as f32 / max_pos as f32;
    let thumb = if vertical {
        Rect::from_min_size(pos2(track.left() + 3.0, track.top() + offset), vec2(track.width() - 6.0, thumb_len))
    } else {
        Rect::from_min_size(pos2(track.left() + offset, track.top() + 3.0), vec2(thumb_len, track.height() - 6.0))
    };
    let resp = ui.interact(track, egui::Id::unique(("scroll", side.index(), vertical)), Sense::click_and_drag());
    let hot = resp.hovered() || resp.dragged();
    ui.painter().rect_filled(thumb, 3.0, if hot { pal.scrollbar_thumb.gamma_multiply(1.3) } else { pal.scrollbar_thumb });
    if let Some(p) = resp.interact_pointer_pos() {
        if resp.dragged() || resp.clicked() {
            let along = if vertical { p.y - track.top() } else { p.x - track.left() };
            let frac = ((along - thumb_len / 2.0) / (len - thumb_len)).clamp(0.0, 1.0);
            let new_pos = (frac * max_pos as f32).round() as usize;
            app.tab_mut(side).top = new_pos * unit;
        }
    }
}

fn status_line(app: &mut App, ui: &mut Ui, rect: Rect, side: Side) {
    let pal = app.pal.clone();
    let s = app.tab(side).stats();
    let mut text = format!(
        "{} / {} in {} / {} file(s), {} / {} dir(s)",
        format::kilo(s.sel_bytes),
        format::kilo(s.total_bytes),
        s.sel_files,
        s.total_files,
        s.sel_dirs,
        s.total_dirs
    );
    let bg_jobs = app.jobs.iter().filter(|j| j.background).count();
    if bg_jobs > 0 && side == Side::Right {
        text.push_str(&format!("   [{bg_jobs} background job(s)]"));
    }
    ui.painter().rect(rect, 0.0, pal.face, Stroke::new(1.0, pal.border), StrokeKind::Inside);
    ui.painter().with_clip_rect(rect.shrink(1.0)).text(pos2(rect.left() + 4.0, rect.center().y), Align2::LEFT_CENTER, text, font(app), pal.text);
}

// ---- quick view ---------------------------------------------------------------------------

fn quick_view(app: &mut App, ui: &mut Ui, rect: Rect) {
    let pal = app.pal.clone();
    let side = app.active;
    let tab = app.tab(side);
    let entry = tab.cursor_entry().cloned();
    let path = entry.as_ref().and_then(|e| tab.loc.child_path(e));
    let Some((path, entry)) = path.zip(entry) else {
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, "Quick view: no file under cursor", font(app), pal.text_dim);
        return;
    };
    if app.preview.as_ref().is_none_or(|(p, _)| *p != path) {
        let preview = build_preview(ui.ctx(), &path, &entry);
        app.preview = Some((path.clone(), preview));
    }
    let inner = rect.shrink(6.0);
    match &app.preview {
        Some((_, Preview::Text(t))) => {
            let galley = ui.painter().layout(t.clone(), FontId::monospace(app.cfg.font_size - 0.5), pal.text, inner.width());
            ui.painter().with_clip_rect(inner).galley(inner.min, galley, pal.text);
        }
        Some((_, Preview::Image(tex, [w, h]))) => {
            let scale = (inner.width() / *w as f32).min(inner.height() / *h as f32).min(1.0);
            let size = vec2(*w as f32 * scale, *h as f32 * scale);
            let r = Rect::from_center_size(inner.center(), size);
            ui.painter().image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        Some((_, Preview::Info(t))) => {
            let galley = ui.painter().layout(t.clone(), font(app), pal.text, inner.width());
            ui.painter().galley(inner.min, galley, pal.text);
        }
        None => {}
    }
}

fn build_preview(ctx: &egui::Context, path: &std::path::Path, e: &FileEntry) -> Preview {
    if e.is_dir {
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let (bytes, files, dirs) = if e.is_parent { (0, 0, 0) } else { crate::vfs::dir_size(path, &cancel) };
        return Preview::Info(format!(
            "{}\n\nFolder\n{} bytes in {} file(s) and {} folder(s)",
            path.display(),
            format::thousands(bytes),
            files,
            dirs
        ));
    }
    if crate::viewer::is_image(path) {
        if let Ok(img) = image::open(path) {
            // `thumbnail` would also enlarge small images.
            let img = if img.width() > 1600 || img.height() > 1600 { img.thumbnail(1600, 1600) } else { img }.to_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            let tex = ctx.load_texture("quickview", egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()), egui::TextureOptions::LINEAR);
            return Preview::Image(tex, size);
        }
    }
    match crate::viewer::read_head(path, 64 * 1024) {
        Ok(bytes) if !crate::viewer::looks_binary(&bytes) => Preview::Text(crate::viewer::decode(&bytes)),
        _ => Preview::Info(format!(
            "{}\n\n{}\n{} bytes\nModified {}",
            path.display(),
            crate::win::type_name(&e.name, false),
            format::thousands(e.size),
            e.modified.map(format::datetime_secs).unwrap_or_default()
        )),
    }
}

// ---- drag & drop ---------------------------------------------------------------------------

fn draw_drag_overlay(app: &mut App, ui: &mut Ui) {
    let Some(drag) = &mut app.drag else { return };
    let pointer = ui.input(|i| i.pointer.latest_pos());
    let released = ui.input(|i| i.pointer.any_released());
    if let Some(p) = pointer {
        if !drag.active && p.distance(drag.start) > 6.0 {
            drag.active = true;
        }
        if drag.active {
            let text = if drag.names.len() == 1 { drag.names[0].clone() } else { format!("{} items", drag.names.len()) };
            let shift = ui.input(|i| i.modifiers.shift);
            let label = format!("{} {}", if shift { "Move" } else { "Copy" }, text);
            let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("dragtip")));
            let galley = painter.layout_no_wrap(label, FontId::proportional(app.cfg.font_size), app.pal.text);
            let r = Rect::from_min_size(p + vec2(14.0, 10.0), galley.size() + vec2(10.0, 6.0));
            painter.rect(r, 3.0, app.pal.bar, Stroke::new(1.0, app.pal.border), StrokeKind::Inside);
            painter.galley(r.min + vec2(5.0, 3.0), galley, app.pal.text);
        }
    }
    if !released {
        return;
    }
    let drag = app.drag.take().unwrap();
    let Some(p) = pointer.filter(|_| drag.active) else { return };
    let shift = ui.input(|i| i.modifiers.shift);
    // Drop target: a directory row under the pointer, else the panel's directory.
    let target_side = [Side::Left, Side::Right].into_iter().find(|s| app.list_rects[s.index()].contains(p));
    let Some(ts) = target_side else { return };
    let rh = row_h(app);
    let tab = app.tab(ts);
    let row = tab.top + ((p.y - app.list_rects[ts.index()].top()) / rh).floor() as usize;
    let over_dir = tab
        .entries
        .get(row)
        .filter(|e| e.is_dir && tab.view == ViewMode::Full && !(ts == drag.side && drag.names.contains(&e.name)))
        .cloned();
    let dest_loc = match over_dir {
        Some(e) if e.is_parent => tab.loc.parent().map(|(l, _)| l),
        Some(e) => Some(tab.loc.child_dir(&e.name)),
        None if ts != drag.side => Some(tab.loc.clone()),
        None => None,
    };
    let Some(dest_loc) = dest_loc else { return };
    // Select the dragged entries so the transfer dialog acts on them.
    let src = app.tab_mut(drag.side);
    for e in src.entries.iter_mut() {
        e.selected = drag.names.contains(&e.name);
    }
    app.active = drag.side;
    let sources = app.sources(drag.side);
    let target = match &dest_loc {
        Location::Fs(_) | Location::Archive { .. } => format!("{}*.*", dest_loc.display()),
        _ => return,
    };
    let src_dir = app.tab(drag.side).loc.host_dir();
    app.dialog = Some(crate::dialogs::Dialog::Transfer {
        mv: shift,
        label: format!("{} {} item(s) to:", if shift { "Move" } else { "Copy" }, sources.len()),
        sources,
        target,
        src_dir,
        init: true,
    });
}

fn handle_external_drop(app: &mut App, ui: &mut Ui) {
    let dropped: Vec<std::path::PathBuf> = ui.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect());
    if dropped.is_empty() {
        return;
    }
    let p = ui.input(|i| i.pointer.latest_pos());
    let side = p
        .and_then(|p| [Side::Left, Side::Right].into_iter().find(|s| app.panel_rects[s.index()].contains(p)))
        .unwrap_or(app.active);
    let loc = app.tab(side).loc.clone();
    if !matches!(loc, Location::Fs(_) | Location::Archive { .. }) {
        return;
    }
    let src_dir = dropped[0].parent().map(|p| p.to_path_buf());
    app.dialog = Some(crate::dialogs::Dialog::Transfer {
        mv: false,
        label: format!("Copy {} dropped item(s) to:", dropped.len()),
        sources: dropped.into_iter().map(crate::ops::Source::Fs).collect(),
        target: format!("{}*.*", loc.display()),
        src_dir,
        init: true,
    });
}

// ---- popup menu (hotlist, history, drives, context) --------------------------------------

pub fn draw_popup(app: &mut App, ctx: &egui::Context) {
    let Some(popup) = &mut app.popup else { return };
    let pal = app.pal.clone();
    let fnt = FontId::proportional(app.cfg.font_size);
    let n = popup.items.len();
    // Keyboard navigation.
    let (up, down, enter, esc) = ctx.input_mut(|i| {
        (
            i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
            i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
            i.consume_key(egui::Modifiers::NONE, Key::Enter),
            i.consume_key(egui::Modifiers::NONE, Key::Escape),
        )
    });
    let selectable = |i: usize, items: &[crate::app::PopupItem]| items[i].cmd.is_some();
    if up || down {
        for _ in 0..n {
            popup.sel = if up { (popup.sel + n - 1) % n } else { (popup.sel + 1) % n };
            if selectable(popup.sel, &popup.items) {
                break;
            }
        }
    }
    if !selectable(popup.sel, &popup.items) {
        if let Some(i) = (0..n).find(|&i| selectable(i, &popup.items)) {
            popup.sel = i;
        }
    }
    let mut chosen: Option<Cmd> = None;
    let mut close = esc;
    if enter {
        chosen = popup.items.get(popup.sel).and_then(|i| i.cmd.clone());
        close = true;
    }
    let screen = ctx.content_rect();
    let pos = popup.pos;
    let area = egui::Area::new(egui::Id::unique("mc-popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .constrain_to(screen)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).fill(pal.bar).stroke(Stroke::new(1.0, pal.border)).inner_margin(3.0).show(ui, |ui| {
                ui.set_min_width(240.0);
                let max_h = (screen.height() - 40.0).max(100.0);
                egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
                    for (i, item) in popup.items.iter().enumerate() {
                        if item.cmd.is_none() && item.label.is_empty() {
                            ui.separator();
                            continue;
                        }
                        let h = app_row_height(&fnt);
                        let w = ui.available_width().max(240.0);
                        let (r, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
                        if resp.hovered() && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) {
                            popup.sel = i;
                        }
                        if i == popup.sel {
                            ui.painter().rect_filled(r, 2.0, pal.cursor_fill);
                        }
                        let color = if item.cmd.is_some() { pal.text } else { pal.text_dim };
                        ui.painter().text(pos2(r.left() + 8.0, r.center().y), Align2::LEFT_CENTER, &item.label, fnt.clone(), color);
                        if !item.detail.is_empty() {
                            ui.painter().text(pos2(r.right() - 8.0, r.center().y), Align2::RIGHT_CENTER, &item.detail, fnt.clone(), pal.text_dim);
                        }
                        if resp.clicked() && item.cmd.is_some() {
                            chosen = item.cmd.clone();
                            close = true;
                        }
                    }
                });
            });
        });
    // Click outside closes (ignore the click that opened the popup).
    let clicked_outside = ctx.input(|i| i.pointer.any_pressed())
        && popup.opened.elapsed() > std::time::Duration::from_millis(150)
        && ctx.pointer_interact_pos().is_some_and(|p| !area.response.rect.contains(p));
    if clicked_outside {
        close = true;
    }
    let was_history = app.cmd_history_popup;
    if close {
        app.popup = None;
        app.cmd_history_popup = false;
    }
    if let Some(c) = chosen {
        // Command history entries go back to the command line for editing.
        if let (true, Cmd::Run(line)) = (was_history, &c) {
            app.cmdline = line.clone();
            app.cmd_active = true;
            return;
        }
        app.pending.push(c);
    }
}

/// Mouse wheel movement this frame in list rows (positive = up), using the
/// Windows default of three lines per notch.
pub fn wheel_lines(ui: &Ui, row_h: f32, page: usize) -> isize {
    let dy: f32 = ui.input(|i| {
        i.events
            .iter()
            .map(|e| match e {
                egui::Event::MouseWheel { unit, delta, .. } => match unit {
                    egui::MouseWheelUnit::Line => delta.y * 3.0,
                    egui::MouseWheelUnit::Point => delta.y / row_h,
                    egui::MouseWheelUnit::Page => delta.y * page as f32,
                },
                _ => 0.0,
            })
            .sum()
    });
    if dy == 0.0 { 0 } else { (dy.abs().ceil() as isize).max(1) * dy.signum() as isize }
}

fn app_row_height(f: &FontId) -> f32 {
    (f.size + 8.0).round()
}
