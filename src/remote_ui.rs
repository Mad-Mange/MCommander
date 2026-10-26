//! FTP/FTPS/SFTP in the user interface: connecting (with password, host key
//! and certificate questions), the saved connections dialog (Ctrl+F), new
//! connections by URL (Ctrl+N), disconnecting (Ctrl+Shift+F) and uploading
//! files that were edited after downloading them.

use crate::app::{App, BgMsg};
use crate::dialogs::{ConfirmAction, Dialog, buttons, enter, modal};
use crate::ops::{Dest, OpKind, OpRequest, Source};
use crate::panel::{Panel, Side};
use crate::remote::sites::{self, Site};
use crate::remote::{self, CertInfo, ConnParams, ConnectError, Connection, HostKeyInfo, Protocol, RemoteFs, Secret, Trust, TrustStore};
use crate::vfs::Location;
use egui::{Context, RichText};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// Everything needed to (re)try a connection after a question was answered.
#[derive(Clone, Debug)]
pub struct ConnectRequest {
    pub side: Side,
    pub name: String,
    pub params: ConnParams,
    /// Index of the saved connection this came from.
    pub site: Option<usize>,
    /// Save the typed password in the Credential Manager once login works.
    pub store_password: bool,
    /// Host key / certificate fingerprints accepted for this session only.
    pub accepted: Vec<String>,
    /// Made with Ctrl+N (offer to save it when disconnecting).
    pub adhoc: bool,
}

/// What to do once a server file has been downloaded to a temporary folder.
pub enum After {
    Open(PathBuf),
    View(PathBuf),
    Edit { local: PathBuf, conn: Arc<Connection>, remote_dir: String },
}

/// A downloaded file opened in the editor; changes are offered for upload.
pub struct RemoteEdit {
    pub local: PathBuf,
    pub conn: Arc<Connection>,
    pub remote_dir: String,
    pub modified: Option<SystemTime>,
}

pub struct PasswordState {
    pub req: ConnectRequest,
    pub text: String,
    pub save: bool,
    pub error: Option<String>,
    pub init: bool,
}

pub struct SiteEdit {
    /// Position in the saved list; `None` for a new connection.
    pub index: Option<usize>,
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: String,
    pub user: String,
    pub anonymous: bool,
    pub password: String,
    pub save_password: bool,
    pub dir: String,
    pub passive: bool,
    pub key_file: String,
    pub error: Option<String>,
    pub init: bool,
    /// Credential Manager target of the site as it was before editing.
    old_target: Option<String>,
}

impl SiteEdit {
    pub fn new_site() -> SiteEdit {
        SiteEdit::from_site(None, &Site::new("", ConnParams::new(Protocol::Ftp, "")))
    }

    pub fn from_site(index: Option<usize>, site: &Site) -> SiteEdit {
        let p = &site.params;
        let anonymous = p.protocol.is_ftp() && (p.user.is_empty() || p.user.eq_ignore_ascii_case("anonymous"));
        SiteEdit {
            index,
            name: site.name.clone(),
            protocol: p.protocol,
            host: p.host.clone(),
            port: p.port.to_string(),
            user: if anonymous { String::new() } else { p.user.clone() },
            anonymous: anonymous && !p.host.is_empty(),
            password: String::new(),
            save_password: site.save_password,
            dir: p.remote_dir.clone(),
            passive: p.passive,
            key_file: p.key_file.as_ref().map(|k| k.to_string_lossy().into_owned()).unwrap_or_default(),
            error: None,
            init: true,
            old_target: (index.is_some() && site.save_password).then(|| Site::credential_target(p)),
        }
    }

    fn to_site(&self) -> Result<Site, String> {
        let host = self.host.trim();
        if host.is_empty() {
            return Err("Enter the host name (or IP address) of the server.".into());
        }
        // "host:port" or a URL typed into the host field.
        let mut p = if host.contains("://") || host.matches(':').count() == 1 {
            let mut p = remote::parse_url(host)?;
            if !host.contains("://") {
                p.protocol = self.protocol;
            }
            p
        } else {
            let mut p = ConnParams::new(self.protocol, host);
            p.port = self.port.trim().parse().map_err(|_| format!("Invalid port: {}", self.port))?;
            p
        };
        p.user = if self.anonymous && p.protocol.is_ftp() { String::new() } else { self.user.trim().to_string() };
        p.remote_dir = self.dir.trim().to_string();
        p.passive = self.passive;
        p.key_file = (p.protocol == Protocol::Sftp && !self.key_file.trim().is_empty()).then(|| PathBuf::from(self.key_file.trim()));
        let name = if self.name.trim().is_empty() { p.host.clone() } else { self.name.trim().to_string() };
        Ok(Site { name, params: p, save_password: self.save_password && !(self.anonymous && self.protocol.is_ftp()) })
    }
}

impl App {
    pub fn trust_store(&self) -> TrustStore {
        TrustStore::beside(&self.cfg.file)
    }

    /// Connects in the background; asks for a password first when needed.
    pub fn start_connect(&mut self, req: ConnectRequest) {
        if req.params.needs_password() {
            let save = req.site.is_some_and(|i| self.cfg.sites.get(i).is_some_and(|s| s.save_password));
            self.dialog = Some(Dialog::Password(Box::new(PasswordState { req, text: String::new(), save, error: None, init: true })));
            return;
        }
        self.connect_attempt += 1;
        let attempt = self.connect_attempt;
        self.connecting = Some(attempt);
        self.dialog = Some(Dialog::Connecting { label: format!("Connecting to {} ...", req.params.url_prefix()) });
        let trust = Trust { store: self.trust_store(), accepted: req.accepted.clone() };
        let tx = self.bg_tx.clone();
        let ctx = crate::CTX.get().cloned().unwrap_or_default();
        std::thread::spawn(move || {
            let result = remote::connect(&req.params, &trust).map(|mut s| {
                let home = s.home().unwrap_or_else(|_| "/".into());
                (s, home)
            });
            let _ = tx.send(BgMsg::Connected { attempt, req: Box::new(req), result });
            ctx.request_repaint();
        });
    }

    pub fn finish_connect(&mut self, attempt: u64, req: ConnectRequest, result: Result<(Box<dyn RemoteFs>, String), ConnectError>) {
        if self.connecting != Some(attempt) {
            return; // Cancelled meanwhile.
        }
        self.connecting = None;
        if matches!(self.dialog, Some(Dialog::Connecting { .. })) {
            self.dialog = None;
        }
        match result {
            Ok((session, home)) => {
                if req.store_password {
                    if let Some(pw) = &req.params.password {
                        if let Err(e) = sites::write_credential(&Site::credential_target(&req.params), &req.params.user, &pw.0) {
                            self.toast(format!("Could not save the password: {e}"));
                        }
                    }
                }
                let trust = Trust { store: self.trust_store(), accepted: req.accepted.clone() };
                let conn = Connection::new(req.params.clone(), req.name.clone(), trust, session);
                if req.adhoc {
                    self.adhoc_conns.insert(conn.id);
                }
                self.active = req.side;
                self.go(req.side, Location::Remote { conn, path: home }, None);
            }
            Err(ConnectError::HostKey(info)) => self.dialog = Some(Dialog::HostKey { req: Box::new(req), info }),
            Err(ConnectError::Certificate(info)) => self.dialog = Some(Dialog::Certificate { req: Box::new(req), info }),
            Err(ConnectError::Auth(msg)) => {
                let save = req.store_password;
                self.dialog = Some(Dialog::Password(Box::new(PasswordState { req, text: String::new(), save, error: Some(msg), init: true })));
            }
            Err(ConnectError::Other(msg)) => self.error(format!("Could not connect to {}\n\n{msg}", req.params.url_prefix())),
        }
    }

    pub fn connect_site(&mut self, index: usize) {
        let Some(site) = self.cfg.sites.get(index) else { return };
        let req = ConnectRequest {
            side: self.active,
            name: site.name.clone(),
            params: site.resolved(),
            site: Some(index),
            store_password: false,
            accepted: Vec::new(),
            adhoc: false,
        };
        self.start_connect(req);
    }

    pub fn connect_url(&mut self, text: &str) -> Result<(), String> {
        let params = remote::parse_url(text)?;
        crate::config::Config::push_history(&mut self.cfg.ftp_history, text.trim());
        let req = ConnectRequest { side: self.active, name: params.host.clone(), params, site: None, store_password: false, accepted: Vec::new(), adhoc: true };
        self.start_connect(req);
        Ok(())
    }

    /// Ctrl+Shift+F: every tab showing this server returns to the local
    /// directory it came from; the connection is closed.
    pub fn disconnect(&mut self, side: Side) {
        let Some(conn) = self.tab(side).loc.remote().map(|(c, _)| c.clone()) else {
            self.toast("The active panel is not connected to a server");
            return;
        };
        let lo = self.list_opts();
        let home = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into());
        for s in [Side::Left, Side::Right] {
            let active = self.panels[s.index()].active;
            for i in 0..self.panels[s.index()].tabs.len() {
                let tab = &self.panels[s.index()].tabs[i];
                let on_server = tab.loc.remote().is_some_and(|(c, _)| c.id == conn.id) || tab.loading.as_ref().and_then(|l| l.remote()).is_some_and(|(c, _)| c.id == conn.id);
                if !on_server {
                    continue;
                }
                let back = Panel::last_local(tab).unwrap_or_else(|| Location::fs(&home));
                if i == active {
                    self.go(s, back, None);
                } else {
                    let tab = &mut self.panels[s.index()].tabs[i];
                    tab.loading = None;
                    let _ = tab.go(back, None, lo, &mut self.zips);
                }
            }
            // Drop the server from the histories so it isn't revisited.
            for tab in &mut self.panels[s.index()].tabs {
                let pos = tab.hist_pos.min(tab.history.len().saturating_sub(1));
                let current = tab.history.get(pos).cloned();
                tab.history.retain(|l| l.remote().is_none_or(|(c, _)| c.id != conn.id));
                if tab.history.is_empty() {
                    tab.history.push(tab.loc.clone());
                }
                tab.hist_pos = current.and_then(|c| tab.history.iter().position(|l| *l == c)).unwrap_or(tab.history.len() - 1);
            }
        }
        conn.close();
        self.remote_edits.retain(|e| e.conn.id != conn.id);
        self.toast(format!("Disconnected from {}", conn.url_prefix()));
        if self.adhoc_conns.remove(&conn.id) {
            let site = Site::new(&conn.name, conn.params.clone());
            self.dialog = Some(Dialog::Confirm {
                title: "Disconnect".into(),
                text: format!("Save the connection to {} in the connection list (Ctrl+F)?", conn.url_prefix()),
                action: ConfirmAction::SaveSite(Box::new(site)),
            });
        }
    }

    /// Downloads the entry at `index` of a server panel to a temporary
    /// folder, then opens, views or edits it.
    pub fn fetch_then(&mut self, side: Side, index: usize, what: &str) {
        let tab = self.tab(side);
        let Some((conn, dir)) = tab.loc.remote().map(|(c, d)| (c.clone(), d.to_string())) else { return };
        let Some(e) = tab.entries.get(index).filter(|e| !e.is_dir).cloned() else { return };
        let tmp = std::env::temp_dir()
            .join("MCommander")
            .join("remote")
            .join(format!("{:x}", crate::app::fxhash(&format!("{}{}", conn.url_prefix(), dir))));
        let local = tmp.join(&e.name);
        let after = match what {
            "view" => After::View(local),
            "edit" => After::Edit { local, conn: conn.clone(), remote_dir: dir.clone() },
            _ => After::Open(local),
        };
        let path = remote::join(&dir, &e.name);
        let source = Source::Remote { conn, path, is_dir: false, is_link: e.is_link(), size: e.size, modified: e.modified };
        let id = self.start_job(OpRequest { kind: OpKind::CopyReplace, sources: vec![source], dest: Some(Dest::Dir(tmp)), mask: "*.*".into() });
        self.job_after.insert(id, after);
    }

    pub fn run_after(&mut self, after: After) {
        match after {
            After::Open(p) => self.shell_open(&p),
            After::View(p) => self.open_viewer(p),
            After::Edit { local, conn, remote_dir } => {
                let editor = self.cfg.editor.clone();
                match crate::win::spawn_detached(&editor, &format!("\"{}\"", local.display()), local.parent()) {
                    Ok(()) => {
                        let modified = std::fs::metadata(&local).and_then(|m| m.modified()).ok();
                        self.remote_edits.retain(|e| e.local != local);
                        self.remote_edits.push(RemoteEdit { local, conn, remote_dir, modified });
                    }
                    Err(e) => self.error(format!("Could not start editor \"{editor}\": {e}")),
                }
            }
        }
    }

    /// Offers to upload server files that were changed in the editor.
    pub fn check_remote_edits(&mut self) {
        if self.remote_edits.is_empty() || self.dialog.is_some() || self.edits_checked.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.edits_checked = Instant::now();
        for e in &mut self.remote_edits {
            let now = std::fs::metadata(&e.local).and_then(|m| m.modified()).ok();
            if now.is_some() && now != e.modified {
                e.modified = now;
                let name = e.local.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.dialog = Some(Dialog::Confirm {
                    title: "Upload changed file".into(),
                    text: format!("\"{name}\" was changed in the editor.\n\nUpload it to {}{}?", e.conn.url_prefix(), remote::join(&e.remote_dir, &name)),
                    action: ConfirmAction::Upload(e.local.clone(), e.conn.clone(), e.remote_dir.clone()),
                });
                break;
            }
        }
    }

    pub fn upload_edited(&mut self, local: PathBuf, conn: Arc<Connection>, dir: String) {
        self.start_job(OpRequest { kind: OpKind::CopyReplace, sources: vec![Source::Fs(local)], dest: Some(Dest::Remote { conn, dir }), mask: "*.*".into() });
    }

    /// Runs a quick server command (rename, make directory) off the UI
    /// thread; the panel is refreshed and `focus` selected afterwards.
    pub fn remote_op(&mut self, side: Side, focus: Option<String>, op: impl FnOnce(&mut dyn RemoteFs) -> std::io::Result<()> + Send + 'static) {
        let Some((conn, _)) = self.tab(side).loc.remote().map(|(c, d)| (c.clone(), d.to_string())) else { return };
        let tx = self.bg_tx.clone();
        let ctx = crate::CTX.get().cloned().unwrap_or_default();
        std::thread::spawn(move || {
            let mut op = Some(op);
            let result = conn.with(|fs| match op.take() {
                Some(f) => f(fs),
                None => Ok(()),
            });
            let _ = tx.send(BgMsg::RemoteDone { side, focus, error: result.err().map(|e| e.to_string()) });
            ctx.request_repaint();
        });
    }

    pub fn import_tc_sites(&mut self) -> usize {
        let Some(path) = sites::find_tc_ftp_ini() else { return 0 };
        let Ok(ini) = crate::ini::Ini::load(&path) else { return 0 };
        let mut added = 0;
        for s in sites::import_tc(&ini) {
            if !self.cfg.sites.iter().any(|x| x.name == s.name) {
                self.cfg.sites.push(s);
                added += 1;
            }
        }
        added
    }
}

// ---- dialogs ------------------------------------------------------------------------------

/// Draws the FTP dialogs; returns whether the dialog stays open.
pub fn draw_dialog(app: &mut App, ctx: &Context, dialog: &mut Dialog) -> bool {
    let pal = app.pal.clone();
    match dialog {
        Dialog::Sites { sel } => {
            let n = app.cfg.sites.len();
            let (up, down) = ctx.input(|i| (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown)));
            if up && *sel > 0 {
                *sel -= 1;
            }
            if down && *sel + 1 < n {
                *sel += 1;
            }
            let mut action = None;
            let (r, esc) = modal(ctx, "dlg-sites", "Connect to server", &pal, 620.0, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(440.0);
                        egui::Frame::NONE.fill(pal.input_bg).stroke(egui::Stroke::new(1.0, pal.border)).show(ui, |ui| {
                            ui.set_min_size(egui::vec2(436.0, 260.0));
                            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                                if n == 0 {
                                    ui.label(RichText::new("No saved connections yet. Click \"New connection...\".").weak());
                                }
                                ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                                    for (i, s) in app.cfg.sites.iter().enumerate() {
                                        let text = format!("{}   ({})", s.name, s.params.url_prefix());
                                        let r = ui.selectable_label(*sel == i, text);
                                        if r.clicked() {
                                            *sel = i;
                                        }
                                        if r.double_clicked() {
                                            action = Some("connect");
                                        }
                                    }
                                });
                            });
                        });
                    });
                    ui.vertical(|ui| {
                        let wide = egui::vec2(140.0, 24.0);
                        for (label, act, enabled) in [
                            ("Connect", "connect", n > 0),
                            ("New connection...", "new", true),
                            ("Duplicate", "dup", n > 0),
                            ("Edit...", "edit", n > 0),
                            ("Delete", "delete", n > 0),
                            ("Import from TC", "import", true),
                        ] {
                            if ui.add_enabled(enabled, egui::Button::new(label).min_size(wide)).clicked() {
                                action = Some(act);
                            }
                        }
                    });
                });
                ui.label(RichText::new("Passwords are kept in the Windows Credential Manager. Ctrl+N connects to a URL directly.").weak().small());
                if enter(ui) && n > 0 {
                    action = Some("connect");
                }
                buttons(ui, &["Close"])
            });
            let sel = (*sel).min(n.saturating_sub(1));
            match action {
                Some("connect") if n > 0 => {
                    app.connect_site(sel);
                    false
                }
                Some("new") => {
                    app.dialog = Some(Dialog::SiteEdit(Box::new(SiteEdit::new_site())));
                    false
                }
                Some("dup") => {
                    let mut s = app.cfg.sites[sel].clone();
                    s.name = format!("{} (copy)", s.name);
                    s.save_password = false;
                    app.cfg.sites.insert(sel + 1, s);
                    app.save_config();
                    app.dialog = Some(Dialog::Sites { sel: sel + 1 });
                    false
                }
                Some("edit") => {
                    let edit = SiteEdit::from_site(Some(sel), &app.cfg.sites[sel]);
                    app.dialog = Some(Dialog::SiteEdit(Box::new(edit)));
                    false
                }
                Some("delete") => {
                    let s = app.cfg.sites.remove(sel);
                    if s.save_password {
                        sites::delete_credential(&Site::credential_target(&s.params));
                    }
                    app.save_config();
                    app.dialog = Some(Dialog::Sites { sel: sel.saturating_sub(1) });
                    false
                }
                Some("import") => {
                    let added = app.import_tc_sites();
                    app.save_config();
                    app.toast(if added == 0 { "No new connections found in Total Commander's wcx_ftp.ini".to_string() } else { format!("Imported {added} connection(s) from Total Commander") });
                    true
                }
                _ => !(r.is_some() || esc),
            }
        }
        Dialog::SiteEdit(st) => {
            let first = std::mem::take(&mut st.init);
            let (r, esc) = modal(ctx, "dlg-site-edit", "Connection details", &pal, 520.0, |ui| site_form(ui, st, first));
            match (r, esc) {
                (Some(0), _) => match st.to_site() {
                    Ok(site) => {
                        save_site(app, st, site);
                        false
                    }
                    Err(e) => {
                        st.error = Some(e);
                        true
                    }
                },
                (Some(_), _) | (_, true) => {
                    app.dialog = Some(Dialog::Sites { sel: st.index.unwrap_or(0) });
                    false
                }
                _ => true,
            }
        }
        Dialog::QuickConnect { text, error, init } => {
            let first = std::mem::take(init);
            let history = app.cfg.ftp_history.clone();
            let (r, esc) = modal(ctx, "dlg-quick-connect", "New connection", &pal, 520.0, |ui| {
                ui.label("Server name or URL:");
                let id = egui::Id::unique("dlg-quick-url");
                crate::dialogs::focus_select_modal(ui, id, first, 0, text.chars().count());
                ui.add(egui::TextEdit::singleline(text).id(id).desired_width(f32::INFINITY));
                ui.label(RichText::new("Examples: ftp.example.com   sftp://me@nas:22/home/me   ftps://user@host   ftpis://host:990").weak().small());
                if !history.is_empty() {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Recent:").weak());
                        for h in history.iter().take(8) {
                            if ui.small_button(h).clicked() {
                                *text = h.clone();
                            }
                        }
                    });
                }
                if let Some(e) = error.as_ref() {
                    ui.colored_label(pal.error, e);
                }
                let b = buttons(ui, &["Connect", "Cancel"]);
                if enter(ui) { Some(0) } else { b }
            });
            match (r, esc) {
                (Some(0), _) => {
                    let t = text.clone();
                    match app.connect_url(&t) {
                        Ok(()) => false,
                        Err(e) => {
                            *error = Some(e);
                            true
                        }
                    }
                }
                (Some(_), _) | (_, true) => false,
                _ => true,
            }
        }
        Dialog::Password(st) => {
            let first = std::mem::take(&mut st.init);
            let can_save = st.req.site.is_some();
            let (r, esc) = modal(ctx, "dlg-password", "Password", &pal, 440.0, |ui| {
                if let Some(e) = &st.error {
                    ui.colored_label(pal.error, format!("Login failed: {e}"));
                    ui.add_space(4.0);
                }
                let who = if st.req.params.user.is_empty() { st.req.params.url_prefix() } else { format!("{}@{}", st.req.params.user, st.req.params.host) };
                ui.label(format!("Password for {who}:"));
                let id = egui::Id::unique("dlg-password-text");
                crate::dialogs::focus_select_modal(ui, id, first, 0, 0);
                ui.add(egui::TextEdit::singleline(&mut st.text).id(id).password(true).desired_width(f32::INFINITY));
                if can_save {
                    ui.checkbox(&mut st.save, "Save password (Windows Credential Manager)");
                }
                let b = buttons(ui, &["OK", "Cancel"]);
                if enter(ui) { Some(0) } else { b }
            });
            match (r, esc) {
                (Some(0), _) => {
                    let mut req = st.req.clone();
                    req.params.password = Some(Secret(std::mem::take(&mut st.text)));
                    req.store_password = can_save && st.save;
                    if let (Some(i), true) = (req.site, can_save) {
                        if let Some(site) = app.cfg.sites.get_mut(i) {
                            if site.save_password != st.save {
                                site.save_password = st.save;
                                if !st.save {
                                    sites::delete_credential(&Site::credential_target(&site.params));
                                }
                                app.save_config();
                            }
                        }
                    }
                    app.start_connect(req);
                    false
                }
                (Some(_), _) | (_, true) => false,
                _ => true,
            }
        }
        Dialog::HostKey { req, info } => {
            let (r, esc) = modal(ctx, "dlg-hostkey", if info.changed { "WARNING: host key changed" } else { "Unknown host key" }, &pal, 560.0, |ui| host_key_text(ui, &pal, info));
            match (r, esc) {
                (Some(0), _) => {
                    let store = app.trust_store();
                    if let Err(e) = store.save_host_key(&info.host, info.port, &info.openssh) {
                        app.toast(format!("Could not save the host key: {e}"));
                    }
                    let mut req = (**req).clone();
                    req.accepted.push(info.fingerprint.clone());
                    app.start_connect(req);
                    false
                }
                (Some(1), _) => {
                    let mut req = (**req).clone();
                    req.accepted.push(info.fingerprint.clone());
                    app.start_connect(req);
                    false
                }
                (Some(_), _) | (_, true) => false,
                _ => true,
            }
        }
        Dialog::Certificate { req, info } => {
            let (r, esc) = modal(ctx, "dlg-cert", "Untrusted certificate", &pal, 560.0, |ui| cert_text(ui, &pal, info));
            match (r, esc) {
                (Some(0), _) => {
                    let store = app.trust_store();
                    if let Err(e) = store.save_cert(&info.host, info.port, &info.fingerprint) {
                        app.toast(format!("Could not save the certificate: {e}"));
                    }
                    let mut req = (**req).clone();
                    req.accepted.push(info.fingerprint.clone());
                    app.start_connect(req);
                    false
                }
                (Some(1), _) => {
                    let mut req = (**req).clone();
                    req.accepted.push(info.fingerprint.clone());
                    app.start_connect(req);
                    false
                }
                (Some(_), _) | (_, true) => false,
                _ => true,
            }
        }
        Dialog::Connecting { label } => {
            let (r, esc) = modal(ctx, "dlg-connecting", "Connecting", &pal, 420.0, |ui| {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(label.as_str());
                });
                buttons(ui, &["Cancel"])
            });
            ctx.request_repaint_after(Duration::from_millis(100));
            if r.is_some() || esc {
                app.connecting = None;
                return false;
            }
            true
        }
        _ => false,
    }
}

fn site_form(ui: &mut egui::Ui, st: &mut SiteEdit, first: bool) -> Option<usize> {
    let mut clicked = None;
    egui::Grid::new("site-form").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label("Session name:");
        let id = egui::Id::unique("site-name");
        crate::dialogs::focus_select_modal(ui, id, first, 0, st.name.chars().count());
        ui.add(egui::TextEdit::singleline(&mut st.name).id(id).hint_text("e.g. Home NAS").desired_width(360.0));
        ui.end_row();
        ui.label("Protocol:");
        let before = st.protocol;
        egui::ComboBox::from_id_salt("site-proto").selected_text(st.protocol.label()).width(360.0).show_ui(ui, |ui| {
            for p in Protocol::ALL {
                ui.selectable_value(&mut st.protocol, p, p.label());
            }
        });
        if st.protocol != before && st.port.trim() == before.default_port().to_string() {
            st.port = st.protocol.default_port().to_string();
        }
        ui.end_row();
        ui.label("Host name:");
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut st.host).hint_text("server or IP address").desired_width(270.0));
            ui.label("Port:");
            ui.add(egui::TextEdit::singleline(&mut st.port).desired_width(50.0));
        });
        ui.end_row();
        let ftp = st.protocol.is_ftp();
        if ftp {
            ui.label("");
            ui.checkbox(&mut st.anonymous, "Anonymous login");
            ui.end_row();
        }
        let named = !(ftp && st.anonymous);
        ui.label("User name:");
        ui.add_enabled(named, egui::TextEdit::singleline(&mut st.user).desired_width(360.0));
        ui.end_row();
        ui.label("Password:");
        ui.vertical(|ui| {
            ui.add_enabled(named, egui::TextEdit::singleline(&mut st.password).password(true).hint_text(if st.old_target.is_some() { "(saved - leave empty to keep)" } else { "" }).desired_width(360.0));
            ui.add_enabled(named, egui::Checkbox::new(&mut st.save_password, "Save password (Windows Credential Manager)"));
        });
        ui.end_row();
        ui.label("Remote dir:");
        ui.add(egui::TextEdit::singleline(&mut st.dir).hint_text("/ (optional)").desired_width(360.0));
        ui.end_row();
        if ftp {
            ui.label("");
            ui.checkbox(&mut st.passive, "Passive mode for transfers (like a web browser)");
            ui.end_row();
        } else {
            ui.label("Private key:");
            ui.add(egui::TextEdit::singleline(&mut st.key_file).hint_text("optional; default: keys in %USERPROFILE%\\.ssh").desired_width(360.0));
            ui.end_row();
        }
    });
    if let Some(e) = &st.error {
        ui.colored_label(egui::Color32::from_rgb(200, 40, 40), e);
    }
    if let Some(b) = buttons(ui, &["OK", "Cancel"]) {
        clicked = Some(b);
    }
    if enter(ui) {
        clicked = Some(0);
    }
    clicked
}

fn save_site(app: &mut App, st: &SiteEdit, site: Site) {
    let target = Site::credential_target(&site.params);
    if site.save_password {
        if !st.password.is_empty() {
            if let Err(e) = sites::write_credential(&target, &site.params.user, &st.password) {
                app.toast(format!("Could not save the password: {e}"));
            }
        } else if let Some(old) = st.old_target.as_ref().filter(|o| **o != target) {
            // The server or user changed: move the saved password along.
            if let Some(pw) = sites::read_credential(old) {
                let _ = sites::write_credential(&target, &site.params.user, &pw);
            }
        }
    }
    if let Some(old) = &st.old_target {
        if *old != target || !site.save_password {
            sites::delete_credential(old);
        }
    }
    let index = match st.index {
        Some(i) if i < app.cfg.sites.len() => {
            app.cfg.sites[i] = site;
            i
        }
        _ => {
            app.cfg.sites.push(site);
            app.cfg.sites.len() - 1
        }
    };
    app.save_config();
    app.dialog = Some(Dialog::Sites { sel: index });
}

fn host_key_text(ui: &mut egui::Ui, pal: &crate::theme::Palette, info: &HostKeyInfo) -> Option<usize> {
    if info.changed {
        ui.colored_label(
            pal.error,
            "The host key of this server is different from the one saved before. Someone may be intercepting the connection (man-in-the-middle attack), or the server was reinstalled. Only continue if you know why the key changed.",
        );
    } else {
        ui.label("The server's host key is not known yet. Compare the fingerprint with the one shown on the server (e.g. ssh-keygen -lf) before trusting it.");
    }
    ui.add_space(6.0);
    egui::Grid::new("hostkey").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
        ui.label("Server:");
        ui.label(format!("{}:{}", info.host, info.port));
        ui.end_row();
        ui.label("Key type:");
        ui.label(&info.algorithm);
        ui.end_row();
        ui.label("Fingerprint:");
        ui.label(RichText::new(&info.fingerprint).monospace());
        ui.end_row();
    });
    let labels: [&str; 3] = if info.changed { ["Replace saved key", "Connect once", "Cancel"] } else { ["Trust and save", "Connect once", "Cancel"] };
    buttons(ui, &labels)
}

fn cert_text(ui: &mut egui::Ui, pal: &crate::theme::Palette, info: &CertInfo) -> Option<usize> {
    ui.colored_label(pal.error, format!("Windows does not trust the server's certificate: {}.", info.reason));
    ui.label("This is normal for self-signed certificates (home NAS, routers). Only continue if you trust this server.");
    ui.add_space(6.0);
    egui::Grid::new("cert").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
        ui.label("Server:");
        ui.label(format!("{}:{}", info.host, info.port));
        ui.end_row();
        ui.label("SHA-256:");
        // Two lines of 16 bytes each, so the dialog keeps its size.
        let half = info.fingerprint.len().min(48);
        let fp = format!("{}\n{}", info.fingerprint[..half].trim_end_matches(':'), &info.fingerprint[half..]);
        ui.label(RichText::new(fp).monospace());
        ui.end_row();
    });
    buttons(ui, &["Always trust", "Connect once", "Cancel"])
}
