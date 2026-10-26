//! SFTP client (russh + russh-sftp). The async libraries run on a shared
//! runtime; every call here blocks the calling worker thread.

use super::trust::{HostKeyCheck, ssh_fingerprint};
use super::{ConnParams, ConnectError, HostKeyInfo, Progress, RemoteEntry, RemoteFs, TIMEOUT, Trust};
use russh::client;
use russh::keys::{PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh_sftp::client::SftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{FileAttributes, StatusCode};
use std::io::{self, Read, Write};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Checks the server's host key against the trust store.
struct Client {
    host: String,
    port: u16,
    trust: Trust,
    rejected: Arc<Mutex<Option<HostKeyInfo>>>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let key = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            PublicKeyOrCertificate::Certificate(c) => PublicKey::new(c.public_key().clone(), ""),
        };
        let fingerprint = ssh_fingerprint(&key);
        let check = self.trust.store.check_host_key(&self.host, self.port, &key);
        if check == HostKeyCheck::Known || self.trust.accepts(&fingerprint) {
            return Ok(true);
        }
        *self.rejected.lock().unwrap() = Some(HostKeyInfo {
            host: self.host.clone(),
            port: self.port,
            algorithm: key.algorithm().as_str().to_string(),
            fingerprint,
            changed: check == HostKeyCheck::Changed,
            openssh: key.to_openssh().unwrap_or_default(),
        });
        Ok(false)
    }
}

pub struct SftpFs {
    handle: client::Handle<Client>,
    sftp: SftpSession,
    home: String,
}

impl SftpFs {
    pub fn connect(p: &ConnParams, trust: &Trust) -> Result<SftpFs, ConnectError> {
        let addr = super::resolve(&p.host, p.port)?;
        let rejected = Arc::new(Mutex::new(None));
        let handler = Client { host: p.host.clone(), port: p.port, trust: trust.clone(), rejected: rejected.clone() };
        let other = |what: &str, e: &dyn std::fmt::Display| ConnectError::Other(format!("{}:{}: {what}{e}", p.host, p.port));
        super::runtime().block_on(async {
            let config = Arc::new(client::Config {
                keepalive_interval: Some(std::time::Duration::from_secs(30)),
                ..Default::default()
            });
            let stream = tokio::time::timeout(TIMEOUT, tokio::net::TcpStream::connect(addr))
                .await
                .map_err(|e| other("", &e))?
                .map_err(|e| other("", &e))?;
            let _ = stream.set_nodelay(true);
            let connected = tokio::time::timeout(TIMEOUT, client::connect_stream(config, stream, handler)).await.map_err(|e| other("", &e))?;
            let mut handle = match connected {
                Ok(h) => h,
                Err(e) => {
                    return Err(match rejected.lock().unwrap().take() {
                        Some(info) => ConnectError::HostKey(info),
                        None => other("", &e),
                    });
                }
            };
            let user = if p.user.is_empty() { std::env::var("USERNAME").unwrap_or_else(|_| "root".into()) } else { p.user.clone() };
            if !authenticate(&mut handle, &user, p).await.map_err(|e| other("", &e))? {
                return Err(ConnectError::Auth(format!("{user}@{}: access denied", p.host)));
            }
            let channel = handle.channel_open_session().await.map_err(|e| other("", &e))?;
            channel.request_subsystem(true, "sftp").await.map_err(|e| other("", &e))?;
            let sftp = SftpSession::new(channel.into_stream()).await.map_err(|e| other("SFTP: ", &e))?;
            sftp.set_timeout(TIMEOUT.as_secs());
            let home = sftp.canonicalize(".").await.unwrap_or_else(|_| "/".into());
            let mut fs = SftpFs { handle, sftp, home: super::normalize(&home, "/") };
            if !p.remote_dir.is_empty() {
                let dir = super::normalize(&p.remote_dir, &fs.home);
                let meta = fs.sftp.metadata(dir.clone()).await.map_err(|e| ConnectError::Other(format!("{dir}: {e}")))?;
                if !meta.is_dir() {
                    return Err(ConnectError::Other(format!("{dir}: not a directory")));
                }
                fs.home = dir;
            }
            Ok(fs)
        })
    }
}

/// Public key (given or default `~/.ssh` keys), then password, then
/// keyboard-interactive with the password as the answer.
async fn authenticate(handle: &mut client::Handle<Client>, user: &str, p: &ConnParams) -> Result<bool, russh::Error> {
    let password = p.password.as_ref().map(|s| s.0.clone());
    let mut keys = Vec::new();
    if let Some(path) = &p.key_file {
        keys.push(path.clone());
    } else if let Some(home) = std::env::var_os("USERPROFILE") {
        let ssh = std::path::PathBuf::from(home).join(".ssh");
        keys.extend(["id_ed25519", "id_ecdsa", "id_rsa"].iter().map(|n| ssh.join(n)).filter(|k| k.is_file()));
    }
    for path in keys {
        // An encrypted key uses the password field as its passphrase.
        let Ok(key) = russh::keys::load_secret_key(&path, password.as_deref()) else { continue };
        let hash = handle.best_supported_rsa_hash().await.ok().flatten().flatten();
        let res = handle.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await?;
        if res.success() {
            return Ok(true);
        }
    }
    let Some(password) = password else { return Ok(false) };
    if handle.authenticate_password(user, password.clone()).await?.success() {
        return Ok(true);
    }
    use client::KeyboardInteractiveAuthResponse as K;
    let mut resp = handle.authenticate_keyboard_interactive_start(user, None).await?;
    for _ in 0..5 {
        match resp {
            K::Success => return Ok(true),
            K::Failure { .. } => return Ok(false),
            K::InfoRequest { prompts, .. } => {
                let answers = prompts.iter().map(|_| password.clone()).collect();
                resp = handle.authenticate_keyboard_interactive_respond(answers).await?;
            }
        }
    }
    Ok(false)
}

fn io_err(e: SftpError) -> io::Error {
    let kind = match &e {
        SftpError::Status(s) => match s.status_code {
            StatusCode::NoSuchFile => io::ErrorKind::NotFound,
            StatusCode::PermissionDenied => io::ErrorKind::PermissionDenied,
            StatusCode::NoConnection | StatusCode::ConnectionLost => io::ErrorKind::ConnectionAborted,
            _ => io::ErrorKind::Other,
        },
        SftpError::Timeout => io::ErrorKind::TimedOut,
        SftpError::IO(_) | SftpError::UnexpectedBehavior(_) => io::ErrorKind::ConnectionAborted,
        _ => io::ErrorKind::Other,
    };
    let text = match &e {
        SftpError::Status(s) if !s.error_message.is_empty() => s.error_message.clone(),
        _ => e.to_string(),
    };
    io::Error::new(kind, text)
}

fn entry(name: String, attrs: &FileAttributes) -> RemoteEntry {
    RemoteEntry {
        name,
        is_dir: attrs.is_dir(),
        is_link: attrs.is_symlink(),
        size: attrs.size.unwrap_or(0),
        modified: attrs.modified().ok(),
        mode: attrs.permissions.map(|m| m & 0o777),
    }
}

impl RemoteFs for SftpFs {
    fn home(&mut self) -> io::Result<String> {
        Ok(self.home.clone())
    }

    fn list(&mut self, dir: &str) -> io::Result<Vec<RemoteEntry>> {
        let sftp = &self.sftp;
        super::runtime().block_on(async {
            let mut out = Vec::new();
            for e in sftp.read_dir(dir).await.map_err(io_err)? {
                let attrs = e.metadata();
                let mut item = entry(e.file_name(), &attrs);
                if item.is_link {
                    // Show links to directories as directories.
                    if let Ok(target) = sftp.metadata(super::join(dir, &item.name)).await {
                        item.is_dir = target.is_dir();
                        if !item.is_dir {
                            item.size = target.size.unwrap_or(0);
                        }
                    }
                }
                out.push(item);
            }
            Ok(out)
        })
    }

    fn download(&mut self, path: &str, out: &mut dyn Write, progress: Progress) -> io::Result<()> {
        let sftp = &self.sftp;
        super::runtime().block_on(async {
            let mut f = sftp.open(path).await.map_err(io_err)?;
            let mut buf = vec![0u8; 256 * 1024];
            let mut done = 0u64;
            loop {
                let n = f.read(&mut buf).await?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
                done += n as u64;
                if !progress(done) {
                    let _ = f.close().await;
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                }
            }
            f.close().await
        })
    }

    fn upload(&mut self, path: &str, input: &mut dyn Read, progress: Progress) -> io::Result<()> {
        let sftp = &self.sftp;
        super::runtime().block_on(async {
            let mut f = sftp.create(path).await.map_err(io_err)?;
            let mut buf = vec![0u8; 256 * 1024];
            let mut done = 0u64;
            loop {
                let n = input.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                f.write_all(&buf[..n]).await?;
                done += n as u64;
                if !progress(done) {
                    let _ = f.close().await;
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "cancelled"));
                }
            }
            f.flush().await?;
            f.close().await
        })
    }

    fn mkdir(&mut self, path: &str) -> io::Result<()> {
        super::runtime().block_on(self.sftp.create_dir(path)).map_err(io_err)
    }

    fn remove_file(&mut self, path: &str) -> io::Result<()> {
        super::runtime().block_on(self.sftp.remove_file(path)).map_err(io_err)
    }

    fn remove_dir(&mut self, path: &str) -> io::Result<()> {
        super::runtime().block_on(self.sftp.remove_dir(path)).map_err(io_err)
    }

    fn rename(&mut self, from: &str, to: &str) -> io::Result<()> {
        super::runtime().block_on(self.sftp.rename(from, to)).map_err(io_err)
    }

    fn set_modified(&mut self, path: &str, time: SystemTime) -> io::Result<()> {
        let secs = time.duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as u32).unwrap_or(0);
        let attrs = FileAttributes { atime: Some(secs), mtime: Some(secs), ..FileAttributes::empty() };
        super::runtime().block_on(self.sftp.set_metadata(path, attrs)).map_err(io_err)
    }
}

impl Drop for SftpFs {
    fn drop(&mut self) {
        let (sftp, handle) = (&self.sftp, &self.handle);
        super::runtime().block_on(async {
            let bye = async {
                let _ = sftp.close().await;
                let _ = handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
            };
            let _ = tokio::time::timeout(std::time::Duration::from_secs(2), bye).await;
        });
    }
}
