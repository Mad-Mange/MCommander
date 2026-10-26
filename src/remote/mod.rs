//! Remote file systems: FTP, FTPS (explicit and implicit TLS) and SFTP.
//!
//! A [`Connection`] owns one logged-in session that panels list through; file
//! operations open their own extra sessions so browsing stays responsive
//! while a transfer runs. All calls block, so they run on worker threads.

pub mod ftp;
#[cfg(test)]
pub mod live_tests;
pub mod sftp;
pub mod sites;
pub mod trust;

use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

pub use trust::TrustStore;

/// How long to wait for a server before giving up (connect, replies, data).
pub const TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Protocol {
    Ftp,
    /// FTP with explicit TLS (`AUTH TLS` on the normal port).
    Ftps,
    /// FTP with implicit TLS (encrypted from the start, usually port 990).
    FtpsImplicit,
    Sftp,
}

impl Protocol {
    pub const ALL: [Protocol; 4] = [Protocol::Ftp, Protocol::Ftps, Protocol::FtpsImplicit, Protocol::Sftp];

    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Ftp | Protocol::Ftps => 21,
            Protocol::FtpsImplicit => 990,
            Protocol::Sftp => 22,
        }
    }

    /// URL scheme, as Total Commander spells them.
    pub fn scheme(self) -> &'static str {
        match self {
            Protocol::Ftp => "ftp",
            Protocol::Ftps => "ftps",
            Protocol::FtpsImplicit => "ftpis",
            Protocol::Sftp => "sftp",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Protocol::Ftp => "FTP",
            Protocol::Ftps => "FTPS (explicit TLS)",
            Protocol::FtpsImplicit => "FTPS (implicit TLS)",
            Protocol::Sftp => "SFTP (SSH)",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            Protocol::Ftp => "FTP",
            Protocol::Ftps | Protocol::FtpsImplicit => "FTPS",
            Protocol::Sftp => "SFTP",
        }
    }

    pub fn is_ftp(self) -> bool {
        self != Protocol::Sftp
    }
}

/// A password that is never printed by `Debug`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() { "\"\"" } else { "\"***\"" })
    }
}

/// Everything needed to log in to a server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnParams {
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    /// Empty means anonymous (FTP).
    pub user: String,
    pub password: Option<Secret>,
    /// Private key for SFTP public-key login (OpenSSH or PuTTY format).
    pub key_file: Option<PathBuf>,
    /// Passive mode for FTP data connections (active mode otherwise).
    pub passive: bool,
    /// Directory to open after connecting; empty for the login directory.
    pub remote_dir: String,
}

impl ConnParams {
    pub fn new(protocol: Protocol, host: &str) -> ConnParams {
        ConnParams {
            protocol,
            host: host.to_string(),
            port: protocol.default_port(),
            user: String::new(),
            password: None,
            key_file: None,
            passive: true,
            remote_dir: String::new(),
        }
    }

    /// `ftp://user@host:port` (the port only when it isn't the default).
    pub fn url_prefix(&self) -> String {
        let user = if self.user.is_empty() { String::new() } else { format!("{}@", self.user) };
        let host = if self.host.contains(':') { format!("[{}]", self.host) } else { self.host.clone() };
        let port = if self.port == self.protocol.default_port() { String::new() } else { format!(":{}", self.port) };
        format!("{}://{user}{host}{port}", self.protocol.scheme())
    }

    /// Whether a password must be asked for before connecting.
    pub fn needs_password(&self) -> bool {
        let anonymous = self.protocol.is_ftp() && (self.user.is_empty() || self.user.eq_ignore_ascii_case("anonymous"));
        self.password.is_none() && !anonymous && self.key_file.is_none()
    }

    fn login_user(&self) -> &str {
        if self.user.is_empty() { "anonymous" } else { &self.user }
    }

    fn login_password(&self) -> &str {
        match &self.password {
            Some(p) => &p.0,
            None if self.protocol.is_ftp() && self.login_user() == "anonymous" => "mcommander@",
            None => "",
        }
    }
}

/// Parses what the user typed into "new connection": a host name, `host:port`,
/// `user@host` or a URL like `sftp://user@host:2222/path`.
pub fn parse_url(text: &str) -> Result<ConnParams, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Enter a host name or URL.".into());
    }
    let (scheme, rest) = match text.split_once("://") {
        Some((s, r)) => (Some(s.to_ascii_lowercase()), r),
        None => (None, text),
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let (userinfo, hostport) = match authority.rfind('@') {
        Some(i) => (Some(&authority[..i]), &authority[i + 1..]),
        None => (None, authority),
    };
    let (host, port) = if let Some(inner) = hostport.strip_prefix('[') {
        // [IPv6]:port
        let end = inner.find(']').ok_or("Missing ] in IPv6 address.")?;
        let port = inner[end + 1..].strip_prefix(':').map(str::to_string);
        (inner[..end].to_string(), port)
    } else {
        match hostport.rsplit_once(':') {
            Some((h, p)) if !h.contains(':') => (h.to_string(), Some(p.to_string())),
            _ => (hostport.to_string(), None),
        }
    };
    if host.is_empty() {
        return Err("The host name is missing.".into());
    }
    let port: Option<u16> = match port {
        Some(p) if !p.is_empty() => Some(p.parse().map_err(|_| format!("Invalid port: {p}"))?),
        _ => None,
    };
    let protocol = match scheme.as_deref() {
        None | Some("ftp") => Protocol::Ftp,
        Some("ftps") if port == Some(990) => Protocol::FtpsImplicit,
        Some("ftps") | Some("ftpes") => Protocol::Ftps,
        Some("ftpis") => Protocol::FtpsImplicit,
        Some("sftp") | Some("ssh") | Some("scp") => Protocol::Sftp,
        Some(other) => return Err(format!("Unsupported protocol: {other}://")),
    };
    let mut p = ConnParams::new(protocol, &host);
    if let Some(port) = port {
        p.port = port;
    }
    if let Some(ui) = userinfo {
        match ui.split_once(':') {
            Some((u, pw)) => {
                p.user = percent_decode(u);
                p.password = Some(Secret(percent_decode(pw)));
            }
            None => p.user = percent_decode(ui),
        }
    }
    p.remote_dir = percent_decode(path);
    Ok(p)
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// One directory entry on a server.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteEntry {
    pub name: String,
    pub is_dir: bool,
    pub is_link: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// Unix permission bits, when the server reports them.
    pub mode: Option<u32>,
}

/// Called with the bytes transferred so far; returning false cancels.
pub type Progress<'a> = &'a mut dyn FnMut(u64) -> bool;

/// Copies `from` to `to`, reporting progress; a `false` from `progress`
/// stops with [`io::ErrorKind::Interrupted`].
pub fn copy_with_progress(from: &mut dyn Read, to: &mut dyn Write, progress: Progress) -> io::Result<()> {
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted && !e.to_string().contains("cancel") => continue,
            Err(e) => return Err(e),
        };
        to.write_all(&buf[..n])?;
        done += n as u64;
        if !progress(done) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
        }
    }
}

/// A logged-in session. Paths are absolute and use `/`.
pub trait RemoteFs: Send {
    /// The directory the server put us in after login.
    fn home(&mut self) -> io::Result<String>;
    fn list(&mut self, dir: &str) -> io::Result<Vec<RemoteEntry>>;
    fn download(&mut self, path: &str, out: &mut dyn Write, progress: Progress) -> io::Result<()>;
    fn upload(&mut self, path: &str, input: &mut dyn Read, progress: Progress) -> io::Result<()>;
    fn mkdir(&mut self, path: &str) -> io::Result<()>;
    fn remove_file(&mut self, path: &str) -> io::Result<()>;
    fn remove_dir(&mut self, path: &str) -> io::Result<()>;
    fn rename(&mut self, from: &str, to: &str) -> io::Result<()>;
    /// Best effort: servers without a way to set times return an error.
    fn set_modified(&mut self, path: &str, time: SystemTime) -> io::Result<()>;
}

/// Why a connection attempt failed.
#[derive(Clone, Debug)]
pub enum ConnectError {
    /// SFTP: the server's host key isn't trusted yet (or changed).
    HostKey(HostKeyInfo),
    /// FTPS: the certificate isn't trusted by Windows and wasn't accepted before.
    Certificate(CertInfo),
    /// Login was refused.
    Auth(String),
    Other(String),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::HostKey(k) if k.changed => write!(f, "The host key of {} has CHANGED ({})", k.host, k.fingerprint),
            ConnectError::HostKey(k) => write!(f, "Unknown host key for {} ({})", k.host, k.fingerprint),
            ConnectError::Certificate(c) => write!(f, "Untrusted certificate for {}: {}", c.host, c.reason),
            ConnectError::Auth(m) => write!(f, "Login failed: {m}"),
            ConnectError::Other(m) => f.write_str(m),
        }
    }
}

impl From<ConnectError> for io::Error {
    fn from(e: ConnectError) -> io::Error {
        let kind = match e {
            ConnectError::Auth(_) => io::ErrorKind::PermissionDenied,
            _ => io::ErrorKind::ConnectionRefused,
        };
        io::Error::new(kind, e.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKeyInfo {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    /// `SHA256:...` as printed by OpenSSH.
    pub fingerprint: String,
    /// A different key is on record for this host.
    pub changed: bool,
    /// The key in OpenSSH public key format (to save it).
    pub openssh: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertInfo {
    pub host: String,
    pub port: u16,
    /// SHA-256 of the server certificate, hex with colons.
    pub fingerprint: String,
    pub reason: String,
}

/// Trust decisions for one connection attempt: the persistent store plus
/// fingerprints the user accepted for this session only.
#[derive(Clone, Debug, Default)]
pub struct Trust {
    pub store: TrustStore,
    pub accepted: Vec<String>,
}

impl Trust {
    pub fn accepts(&self, fingerprint: &str) -> bool {
        self.accepted.iter().any(|a| a == fingerprint)
    }
}

/// Opens and logs in a new session.
pub fn connect(params: &ConnParams, trust: &Trust) -> Result<Box<dyn RemoteFs>, ConnectError> {
    if params.protocol.is_ftp() {
        Ok(Box::new(ftp::FtpFs::connect(params, trust)?))
    } else {
        Ok(Box::new(sftp::SftpFs::connect(params, trust)?))
    }
}

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

/// A server connection shared by the panels (and tabs) that show it.
pub struct Connection {
    pub id: u64,
    pub params: ConnParams,
    /// Saved connection name, or the host for ad-hoc connections.
    pub name: String,
    trust: Mutex<Trust>,
    session: Mutex<Option<Box<dyn RemoteFs>>>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Connection#{}({})", self.id, self.params.url_prefix())
    }
}

impl PartialEq for Connection {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Connection {}

impl std::hash::Hash for Connection {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

impl Connection {
    pub fn new(params: ConnParams, name: String, trust: Trust, session: Box<dyn RemoteFs>) -> Arc<Connection> {
        Arc::new(Connection {
            id: NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed),
            params,
            name,
            trust: Mutex::new(trust),
            session: Mutex::new(Some(session)),
        })
    }

    pub fn url_prefix(&self) -> String {
        self.params.url_prefix()
    }

    pub(crate) fn trust(&self) -> Trust {
        self.trust.lock().unwrap().clone()
    }

    /// Runs `f` on the shared session, reconnecting once when the server
    /// dropped the connection (idle timeouts are common on FTP).
    pub fn with<T>(&self, mut f: impl FnMut(&mut dyn RemoteFs) -> io::Result<T>) -> io::Result<T> {
        let mut guard = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(connect(&self.params, &self.trust())?);
        }
        match f(guard.as_mut().unwrap().as_mut()) {
            Err(e) if is_disconnect(&e) => {
                *guard = None;
                let mut fresh = connect(&self.params, &self.trust())?;
                let r = f(fresh.as_mut());
                *guard = Some(fresh);
                r
            }
            r => r,
        }
    }

    /// A session for a file operation: a new connection so the panels stay
    /// usable, or the shared one when the server allows only one login.
    pub fn borrow_session(&self) -> io::Result<Box<dyn RemoteFs>> {
        match connect(&self.params, &self.trust()) {
            Ok(s) => Ok(s),
            Err(e) => self.session.lock().unwrap_or_else(|e| e.into_inner()).take().ok_or_else(|| io::Error::from(e)),
        }
    }

    /// Returns a borrowed session; it becomes the shared one if that is gone.
    pub fn give_back(&self, session: Box<dyn RemoteFs>) {
        let mut guard = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(session);
        }
    }

    /// Logs out; later calls reconnect on demand.
    pub fn close(&self) {
        let mut guard = self.session.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None;
    }
}

/// The session of a file operation: reconnects once when the server dropped
/// the connection, and is handed back to the [`Connection`] when done.
pub struct JobSession {
    conn: Arc<Connection>,
    fs: Option<Box<dyn RemoteFs>>,
}

impl JobSession {
    pub fn open(conn: &Arc<Connection>) -> io::Result<JobSession> {
        Ok(JobSession { conn: conn.clone(), fs: Some(conn.borrow_session()?) })
    }

    pub fn call<T>(&mut self, mut f: impl FnMut(&mut dyn RemoteFs) -> io::Result<T>) -> io::Result<T> {
        if self.fs.is_none() {
            self.fs = Some(connect(&self.conn.params, &self.conn.trust())?);
        }
        match f(self.fs.as_mut().unwrap().as_mut()) {
            Err(e) if is_disconnect(&e) => {
                self.fs = None;
                let mut fresh = connect(&self.conn.params, &self.conn.trust())?;
                let r = f(fresh.as_mut());
                self.fs = Some(fresh);
                r
            }
            r => r,
        }
    }
}

impl Drop for JobSession {
    fn drop(&mut self) {
        if let Some(fs) = self.fs.take() {
            self.conn.give_back(fs);
        }
    }
}

/// Errors that mean the session is gone (worth one reconnect).
pub fn is_disconnect(e: &io::Error) -> bool {
    use io::ErrorKind::*;
    matches!(e.kind(), ConnectionAborted | ConnectionReset | BrokenPipe | NotConnected | UnexpectedEof | TimedOut)
        || e.to_string().contains("421")
}

// ---- remote paths ------------------------------------------------------------------

/// `dir` + `name` with exactly one `/` between them.
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() || dir == "/" {
        format!("/{name}")
    } else {
        format!("{}/{name}", dir.trim_end_matches('/'))
    }
}

/// The parent directory and the last component of an absolute path.
pub fn split(path: &str) -> Option<(String, String)> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let i = trimmed.rfind('/')?;
    let parent = if i == 0 { "/".to_string() } else { trimmed[..i].to_string() };
    Some((parent, trimmed[i + 1..].to_string()))
}

/// Makes a server path absolute and canonical (`/a/b`, `/` for the root).
pub fn normalize(path: &str, base: &str) -> String {
    let joined = if path.starts_with('/') { path.to_string() } else { format!("{base}/{path}") };
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    format!("/{}", parts.join("/"))
}

/// `rwxr-xr-x` for Unix permission bits.
pub fn mode_string(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    for shift in [6, 3, 0] {
        let bits = (mode >> shift) & 7;
        s.push(if bits & 4 != 0 { 'r' } else { '-' });
        s.push(if bits & 2 != 0 { 'w' } else { '-' });
        s.push(if bits & 1 != 0 { 'x' } else { '-' });
    }
    s
}

/// Converts listing entries to panel entries.
pub fn to_file_entries(list: Vec<RemoteEntry>, show_hidden: bool) -> Vec<crate::vfs::FileEntry> {
    use crate::format::{ATTR_DIRECTORY, ATTR_HIDDEN, ATTR_REPARSE};
    list.into_iter()
        .filter(|e| e.name != "." && e.name != ".." && !e.name.is_empty())
        .filter(|e| show_hidden || !e.name.starts_with('.'))
        .map(|e| {
            let mut attrs = 0;
            if e.is_dir {
                attrs |= ATTR_DIRECTORY;
            }
            if e.is_link {
                attrs |= ATTR_REPARSE;
            }
            if e.name.starts_with('.') {
                attrs |= ATTR_HIDDEN;
            }
            crate::vfs::FileEntry {
                name: e.name,
                is_dir: e.is_dir,
                is_parent: false,
                size: if e.is_dir { 0 } else { e.size },
                modified: e.modified,
                attrs,
                selected: false,
                dir_size: None,
                full_path: None,
                display: None,
                mode: e.mode,
            }
        })
        .collect()
}

/// Shared multi-threaded runtime for the async SSH library.
pub(crate) fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("mc-ssh")
            .enable_all()
            .build()
            .expect("tokio runtime")
    })
}

/// Resolves `host:port`, preferring IPv4 like most clients.
pub(crate) fn resolve(host: &str, port: u16) -> Result<std::net::SocketAddr, ConnectError> {
    use std::net::ToSocketAddrs;
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|e| ConnectError::Other(format!("Cannot resolve {host}: {e}")))?
        .collect();
    addrs
        .iter()
        .find(|a| a.is_ipv4())
        .or(addrs.first())
        .copied()
        .ok_or_else(|| ConnectError::Other(format!("Cannot resolve {host}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_urls_and_host_names() {
        let p = parse_url("ftp.example.com").unwrap();
        assert_eq!((p.protocol, p.host.as_str(), p.port), (Protocol::Ftp, "ftp.example.com", 21));
        assert!(!p.needs_password(), "anonymous FTP needs no password");

        let p = parse_url("sftp://me@nas.local:2222/home/me/").unwrap();
        assert_eq!((p.protocol, p.host.as_str(), p.port, p.user.as_str()), (Protocol::Sftp, "nas.local", 2222, "me"));
        assert_eq!(p.remote_dir, "/home/me/");
        assert!(p.needs_password());
        assert_eq!(p.url_prefix(), "sftp://me@nas.local:2222");

        let p = parse_url("ftps://u:p%40ss@host").unwrap();
        assert_eq!((p.protocol, p.password.clone().unwrap().0.as_str()), (Protocol::Ftps, "p@ss"));
        assert_eq!(parse_url("ftps://host:990").unwrap().protocol, Protocol::FtpsImplicit);
        assert_eq!(parse_url("ftpis://host").unwrap().port, 990);
        assert_eq!(parse_url("ftpes://host").unwrap().protocol, Protocol::Ftps);
        let p = parse_url("ftp://[::1]:2121/x").unwrap();
        assert_eq!((p.host.as_str(), p.port), ("::1", 2121));
        assert_eq!(p.url_prefix(), "ftp://[::1]:2121");
        assert!(parse_url("http://x").is_err());
        assert!(parse_url("ftp://host:99999").is_err());
        assert!(parse_url("  ").is_err());
        assert!(format!("{:?}", parse_url("ftp://a:secret@b").unwrap()).contains("***"));
    }

    #[test]
    fn remote_paths() {
        assert_eq!(join("/", "a"), "/a");
        assert_eq!(join("/a/", "b"), "/a/b");
        assert_eq!(split("/a/b"), Some(("/a".into(), "b".into())));
        assert_eq!(split("/a"), Some(("/".into(), "a".into())));
        assert_eq!(split("/"), None);
        assert_eq!(normalize("../c/./d", "/a/b"), "/a/c/d");
        assert_eq!(normalize("/", "/x"), "/");
        assert_eq!(normalize("..", "/"), "/");
        assert_eq!(mode_string(0o754), "rwxr-xr--");
    }
}
