#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! MCommander - a dual-pane file manager for Windows inspired by Total Commander.

mod app;
mod archive;
mod commands;
mod config;
mod dialogs;
mod format;
mod icons;
mod ini;
mod mask;
mod ops;
mod paint_icons;
mod panel;
mod remote;
mod remote_ui;
mod rename;
mod search;
mod sync;
mod theme;
mod thumbs;
mod tree;
mod ui;
mod vfs;
mod viewer;
mod watch;
mod win;

use app::{Options, Step};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::OnceLock;

/// The egui context, for background threads that need to request repaints.
pub static CTX: OnceLock<egui::Context> = OnceLock::new();

const ICON_PNG: &[u8] = include_bytes!("../res/mcommander-256.png");

fn usage() -> &'static str {
    "MCommander [left_path] [right_path] [options]\n\n\
     /L=path        left panel directory\n\
     /R=path        right panel directory\n\
     /I=file.ini    use this settings file\n\
     --size=WxH     initial window size\n\
     --keys=K1,K2   scripted input for tests (Down, Ctrl+F3, Text:abc, Wait:300,\n\
                    NumPlus, NumMinus, NumStar, CtrlC, CtrlV, Click:x;y, RClick:x;y,\n\
                    RDown:x;y, Move:x;y, RUp:x;y, Shot:file.png, Exit)\n"
}

fn parse_step(token: &str) -> Option<Step> {
    let t = token.trim();
    if let Some(v) = t.strip_prefix("Text:") {
        return Some(Step::Text(v.to_string()));
    }
    if let Some(v) = t.strip_prefix("Wait:") {
        return v.parse().ok().map(Step::Wait);
    }
    let pos = |v: &str| -> Option<egui::Pos2> {
        let (x, y) = v.split_once(';')?;
        Some(egui::pos2(x.trim().parse().ok()?, y.trim().parse().ok()?))
    };
    use egui::PointerButton::{Primary, Secondary};
    for (prefix, button, pressed, click) in [
        ("Click:", Primary, true, true),
        ("RClick:", Secondary, true, true),
        ("RDown:", Secondary, true, false),
        ("RUp:", Secondary, false, false),
    ] {
        if let Some(v) = t.strip_prefix(prefix) {
            return Some(Step::Mouse { pos: pos(v)?, button, pressed, click });
        }
    }
    if let Some(v) = t.strip_prefix("Move:") {
        return pos(v).map(Step::Move);
    }
    if let Some(v) = t.strip_prefix("ShotSearch:") {
        return Some(Step::ShotSearch(PathBuf::from(v)));
    }
    if let Some(v) = t.strip_prefix("ShotChild:") {
        return Some(Step::ShotChild(PathBuf::from(v)));
    }
    if let Some(v) = t.strip_prefix("Shot:") {
        return Some(Step::Shot(PathBuf::from(v)));
    }
    let raw = |vk: u16, ctrl: bool| Some(Step::Raw(win::RawKey { vk, ctrl, shift: false, alt: false }));
    match t {
        "Exit" => return Some(Step::Exit),
        "NumPlus" => return raw(win::VK_NUM_PLUS, false),
        "NumMinus" => return raw(win::VK_NUM_MINUS, false),
        "NumStar" => return raw(win::VK_NUM_STAR, false),
        "NumSlash" => return raw(win::VK_NUM_SLASH, false),
        "CtrlC" => return raw(0x43, true),
        "CtrlX" => return raw(0x58, true),
        "CtrlV" => return raw(0x56, true),
        _ => {}
    }
    let mut mods = egui::Modifiers::NONE;
    let mut name = t;
    loop {
        if let Some(r) = name.strip_prefix("Ctrl+") {
            mods.ctrl = true;
            mods.command = true;
            name = r;
        } else if let Some(r) = name.strip_prefix("Alt+") {
            mods.alt = true;
            name = r;
        } else if let Some(r) = name.strip_prefix("Shift+") {
            mods.shift = true;
            name = r;
        } else {
            break;
        }
    }
    let key = match name {
        "Up" => egui::Key::ArrowUp,
        "Down" => egui::Key::ArrowDown,
        "Left" => egui::Key::ArrowLeft,
        "Right" => egui::Key::ArrowRight,
        "Esc" => egui::Key::Escape,
        other => egui::Key::from_name(other)?,
    };
    Some(Step::Key(key, mods))
}

fn parse_args() -> Result<(Options, Option<[f32; 2]>), String> {
    let mut opts = Options { config: config::default_config_path(), left: None, right: None, steps: VecDeque::new() };
    let mut size = None;
    let mut positional = Vec::new();
    for arg in std::env::args().skip(1) {
        let lower = arg.to_lowercase();
        if lower == "/?" || lower == "--help" || lower == "-h" {
            return Err(usage().to_string());
        } else if lower.starts_with("/l=") {
            opts.left = Some(arg[3..].trim_matches('"').to_string());
        } else if lower.starts_with("/r=") {
            opts.right = Some(arg[3..].trim_matches('"').to_string());
        } else if lower.starts_with("/i=") {
            opts.config = PathBuf::from(arg[3..].trim_matches('"'));
        } else if let Some(v) = arg.strip_prefix("--size=") {
            let (w, h) = v.split_once('x').ok_or("bad --size")?;
            size = Some([w.parse().map_err(|_| "bad --size")?, h.parse().map_err(|_| "bad --size")?]);
        } else if let Some(v) = arg.strip_prefix("--keys=") {
            for tok in v.split(',').filter(|t| !t.trim().is_empty()) {
                opts.steps.push_back(parse_step(tok).ok_or_else(|| format!("unknown key step: {tok}"))?);
            }
        } else if arg.starts_with("--") || arg.starts_with('/') && arg.len() <= 3 {
            // Unknown switches (e.g. TC's /O, /T) are ignored.
        } else {
            positional.push(arg);
        }
    }
    let mut pos = positional.into_iter();
    if opts.left.is_none() {
        opts.left = pos.next();
    }
    if opts.right.is_none() {
        opts.right = pos.next();
    }
    Ok((opts, size))
}

fn window_icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(ICON_PNG).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width, height })
}

fn main() -> eframe::Result {
    let (opts, size) = match parse_args() {
        Ok(v) => v,
        Err(msg) => {
            let _ = win::shell_execute("open", "cmd.exe", &format!("/K echo {}", msg.lines().next().unwrap_or("")), None);
            eprintln!("{msg}");
            return Ok(());
        }
    };
    let cfg = config::Config::load(opts.config.clone());
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(format!("MCommander (x64) {}", env!("CARGO_PKG_VERSION")))
        .with_app_id("MCommander")
        .with_min_inner_size([640.0, 420.0]);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let mut centered = true;
    if let Some(s) = size {
        viewport = viewport.with_inner_size(s);
    } else if let Some([x, y, w, h]) = cfg.window {
        viewport = viewport.with_position([x, y]).with_inner_size([w, h]);
        centered = false;
    } else {
        viewport = viewport.with_inner_size([1100.0, 820.0]);
    }
    if cfg.maximized && size.is_none() {
        viewport = viewport.with_maximized(true);
    }
    let native = eframe::NativeOptions { viewport, centered, ..Default::default() };
    eframe::run_native(
        "MCommander",
        native,
        Box::new(move |cc| {
            let _ = CTX.set(cc.egui_ctx.clone());
            Ok(Box::new(app::App::new(cc, cfg, opts)))
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_steps() {
        assert!(matches!(parse_step("Down"), Some(Step::Key(egui::Key::ArrowDown, m)) if m.is_none()));
        assert!(matches!(parse_step("Ctrl+F3"), Some(Step::Key(egui::Key::F3, m)) if m.ctrl));
        assert!(matches!(parse_step("Alt+Shift+F9"), Some(Step::Key(egui::Key::F9, m)) if m.alt && m.shift));
        assert!(matches!(parse_step("Text:abc"), Some(Step::Text(t)) if t == "abc"));
        assert!(matches!(parse_step("NumPlus"), Some(Step::Raw(_))));
        assert!(parse_step("Bogus").is_none());
    }
}
