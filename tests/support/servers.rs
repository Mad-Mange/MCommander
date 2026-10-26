//! Local FTP, FTPS and SFTP servers backed by a directory, for the tests and
//! for `cargo run --example test_servers` (manual testing of the UI).

#![allow(dead_code)]

use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::{PrivateKey, PublicKey};
use russh::server::{Auth, ChannelOpenHandle, Msg, Server as _, Session};
use russh::{Channel, ChannelId};
use russh_sftp::protocol::{Attrs, Data, File, FileAttributes, Handle, Name, OpenFlags, Status, StatusCode, Version};
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap())
}

fn wait_for(port: u16) {
    for _ in 0..200 {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    panic!("test server on port {port} did not start");
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// A self-signed certificate for `localhost` / `127.0.0.1`, written as PEM files.
pub struct TlsFiles {
    pub cert: PathBuf,
    pub key: PathBuf,
    pub der: Vec<u8>,
}

pub fn self_signed(dir: &Path) -> TlsFiles {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string(), "127.0.0.1".to_string()]).unwrap();
    let cert = dir.join("cert.pem");
    let key = dir.join("key.pem");
    std::fs::write(&cert, ck.cert.pem()).unwrap();
    std::fs::write(&key, ck.signing_key.serialize_pem()).unwrap();
    TlsFiles { cert, key, der: ck.cert.der().to_vec() }
}

/// Starts an FTP server (FTPS with explicit TLS when `tls` is given) that
/// accepts any user name and password. Returns its port.
pub fn ftp(root: &Path, tls: Option<&TlsFiles>) -> u16 {
    ftp_on(root, tls, free_port())
}

pub fn ftp_on(root: &Path, tls: Option<&TlsFiles>, port: u16) -> u16 {
    let root = root.to_path_buf();
    let mut builder = libunftp::ServerBuilder::new(Box::new(move || unftp_sbe_fs::Filesystem::new(root.clone()).unwrap()))
        .greeting("MCommander test server")
        .passive_ports(50000..=59999)
        .active_passive_mode(libunftp::options::ActivePassiveMode::ActiveAndPassive);
    if let Some(t) = tls {
        builder = builder.ftps(t.cert.clone(), t.key.clone());
    }
    let server = builder.build().unwrap();
    runtime().spawn(async move {
        let _ = server.listen(format!("127.0.0.1:{port}")).await;
    });
    wait_for(port);
    port
}

pub struct SftpServer {
    pub port: u16,
    pub host_key: PublicKey,
}

/// Starts an SFTP server for `user` / `password` serving `root`.
pub fn sftp(root: &Path, user: &str, password: &str) -> SftpServer {
    sftp_on(root, user, password, 0)
}

pub fn sftp_on(root: &Path, user: &str, password: &str, port: u16) -> SftpServer {
    let key = PrivateKey::new(KeypairData::Ed25519(Ed25519Keypair::from_seed(&[42u8; 32])), "test").unwrap();
    let host_key = key.public_key().clone();
    let config = Arc::new(russh::server::Config {
        keys: vec![key],
        auth_rejection_time: Duration::from_millis(10),
        auth_rejection_time_initial: Some(Duration::ZERO),
        ..Default::default()
    });
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let mut server = SshServer { root: root.to_path_buf(), user: user.into(), password: password.into() };
    runtime().spawn(async move {
        let listener = tokio::net::TcpListener::from_std(listener).unwrap();
        let _ = server.run_on_socket(config, &listener).await;
    });
    wait_for(port);
    SftpServer { port, host_key }
}

#[derive(Clone)]
struct SshServer {
    root: PathBuf,
    user: String,
    password: String,
}

impl russh::server::Server for SshServer {
    type Handler = SshSession;

    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> SshSession {
        SshSession { server: self.clone(), channels: HashMap::new() }
    }
}

struct SshSession {
    server: SshServer,
    channels: HashMap<ChannelId, Channel<Msg>>,
}

impl russh::server::Handler for SshSession {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == self.server.user && password == self.server.password { Auth::Accept } else { Auth::reject() })
    }

    async fn channel_open_session(&mut self, channel: Channel<Msg>, reply: ChannelOpenHandle, _: &mut Session) -> Result<(), Self::Error> {
        self.channels.insert(channel.id(), channel);
        reply.accept().await;
        Ok(())
    }

    async fn channel_eof(&mut self, channel: ChannelId, session: &mut Session) -> Result<(), Self::Error> {
        session.close(channel)?;
        Ok(())
    }

    async fn subsystem_request(&mut self, channel_id: ChannelId, name: &str, session: &mut Session) -> Result<(), Self::Error> {
        match self.channels.remove(&channel_id) {
            Some(channel) if name == "sftp" => {
                session.channel_success(channel_id)?;
                russh_sftp::server::run(channel.into_stream(), SftpDir { root: self.server.root.clone(), handles: HashMap::new(), next: 1 }).await;
            }
            _ => session.channel_failure(channel_id)?,
        }
        Ok(())
    }
}

enum Open {
    File(std::fs::File),
    Dir(Option<Vec<File>>),
}

/// SFTP subsystem serving a local directory as `/`.
struct SftpDir {
    root: PathBuf,
    handles: HashMap<String, Open>,
    next: u64,
}

fn status(id: u32) -> Status {
    Status { id, status_code: StatusCode::Ok, error_message: "Ok".into(), language_tag: "en-US".into() }
}

fn code(e: std::io::Error) -> StatusCode {
    match e.kind() {
        std::io::ErrorKind::NotFound => StatusCode::NoSuchFile,
        std::io::ErrorKind::PermissionDenied => StatusCode::PermissionDenied,
        _ => StatusCode::Failure,
    }
}

fn clean(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for p in path.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    format!("/{}", parts.join("/"))
}

impl SftpDir {
    fn real(&self, path: &str) -> PathBuf {
        let rel = clean(path);
        self.root.join(rel.trim_start_matches('/'))
    }

    fn add(&mut self, open: Open) -> String {
        let h = self.next.to_string();
        self.next += 1;
        self.handles.insert(h.clone(), open);
        h
    }
}

impl russh_sftp::server::Handler for SftpDir {
    type Error = StatusCode;

    fn unimplemented(&self) -> Self::Error {
        StatusCode::OpUnsupported
    }

    async fn init(&mut self, _version: u32, _ext: HashMap<String, String>) -> Result<Version, Self::Error> {
        Ok(Version::new())
    }

    async fn realpath(&mut self, id: u32, path: String) -> Result<Name, Self::Error> {
        Ok(Name { id, files: vec![File::dummy(clean(&path))] })
    }

    async fn opendir(&mut self, id: u32, path: String) -> Result<Handle, Self::Error> {
        let mut files = Vec::new();
        for e in std::fs::read_dir(self.real(&path)).map_err(code)?.flatten() {
            let Ok(meta) = e.metadata() else { continue };
            files.push(File::new(e.file_name().to_string_lossy().into_owned(), FileAttributes::from(&meta)));
        }
        Ok(Handle { id, handle: self.add(Open::Dir(Some(files))) })
    }

    async fn readdir(&mut self, id: u32, handle: String) -> Result<Name, Self::Error> {
        match self.handles.get_mut(&handle) {
            Some(Open::Dir(files)) => match files.take() {
                Some(files) if !files.is_empty() => Ok(Name { id, files }),
                _ => Err(StatusCode::Eof),
            },
            _ => Err(StatusCode::Failure),
        }
    }

    async fn open(&mut self, id: u32, filename: String, pflags: OpenFlags, _attrs: FileAttributes) -> Result<Handle, Self::Error> {
        let f = std::fs::OpenOptions::new()
            .read(pflags.contains(OpenFlags::READ))
            .write(pflags.contains(OpenFlags::WRITE))
            .create(pflags.contains(OpenFlags::CREATE))
            .truncate(pflags.contains(OpenFlags::TRUNCATE))
            .open(self.real(&filename))
            .map_err(code)?;
        Ok(Handle { id, handle: self.add(Open::File(f)) })
    }

    async fn read(&mut self, id: u32, handle: String, offset: u64, len: u32) -> Result<Data, Self::Error> {
        let Some(Open::File(f)) = self.handles.get_mut(&handle) else { return Err(StatusCode::Failure) };
        f.seek(SeekFrom::Start(offset)).map_err(code)?;
        let mut buf = vec![0u8; len as usize];
        let n = f.read(&mut buf).map_err(code)?;
        if n == 0 {
            return Err(StatusCode::Eof);
        }
        buf.truncate(n);
        Ok(Data { id, data: buf })
    }

    async fn write(&mut self, id: u32, handle: String, offset: u64, data: Vec<u8>) -> Result<Status, Self::Error> {
        let Some(Open::File(f)) = self.handles.get_mut(&handle) else { return Err(StatusCode::Failure) };
        f.seek(SeekFrom::Start(offset)).map_err(code)?;
        f.write_all(&data).map_err(code)?;
        Ok(status(id))
    }

    async fn close(&mut self, id: u32, handle: String) -> Result<Status, Self::Error> {
        self.handles.remove(&handle);
        Ok(status(id))
    }

    async fn stat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        let meta = std::fs::metadata(self.real(&path)).map_err(code)?;
        Ok(Attrs { id, attrs: FileAttributes::from(&meta) })
    }

    async fn lstat(&mut self, id: u32, path: String) -> Result<Attrs, Self::Error> {
        self.stat(id, path).await
    }

    async fn fstat(&mut self, id: u32, handle: String) -> Result<Attrs, Self::Error> {
        let Some(Open::File(f)) = self.handles.get(&handle) else { return Err(StatusCode::Failure) };
        Ok(Attrs { id, attrs: FileAttributes::from(&f.metadata().map_err(code)?) })
    }

    async fn setstat(&mut self, id: u32, path: String, attrs: FileAttributes) -> Result<Status, Self::Error> {
        if let Some(m) = attrs.mtime {
            let f = std::fs::OpenOptions::new().write(true).open(self.real(&path)).map_err(code)?;
            f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(m as u64)).map_err(code)?;
        }
        Ok(status(id))
    }

    async fn mkdir(&mut self, id: u32, path: String, _attrs: FileAttributes) -> Result<Status, Self::Error> {
        std::fs::create_dir(self.real(&path)).map_err(code)?;
        Ok(status(id))
    }

    async fn rmdir(&mut self, id: u32, path: String) -> Result<Status, Self::Error> {
        std::fs::remove_dir(self.real(&path)).map_err(code)?;
        Ok(status(id))
    }

    async fn remove(&mut self, id: u32, filename: String) -> Result<Status, Self::Error> {
        std::fs::remove_file(self.real(&filename)).map_err(code)?;
        Ok(status(id))
    }

    async fn rename(&mut self, id: u32, oldpath: String, newpath: String) -> Result<Status, Self::Error> {
        let to = self.real(&newpath);
        if to.exists() {
            return Err(StatusCode::Failure);
        }
        std::fs::rename(self.real(&oldpath), to).map_err(code)?;
        Ok(status(id))
    }
}
