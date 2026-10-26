//! FTP and FTPS client (suppaftp with rustls). Certificates are checked
//! against the Windows root store; a certificate the user accepted before is
//! pinned by its SHA-256 fingerprint.

use super::{CertInfo, ConnParams, ConnectError, Progress, Protocol, RemoteEntry, RemoteFs, TIMEOUT, Trust};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;
use suppaftp::types::FileType;
use suppaftp::{FtpError, Mode, RustlsConnector, RustlsFtpStream, Status};

pub struct FtpFs {
    ftp: RustlsFtpStream,
    mlsd: bool,
    mfmt: bool,
    home: String,
    /// Set after a cancelled transfer: servers answer ABOR inconsistently
    /// (426 and/or 226, before or after closing the data connection), so
    /// the control connection can't be trusted to be in sync any more.
    broken: bool,
}

impl FtpFs {
    pub fn connect(p: &ConnParams, trust: &Trust) -> Result<FtpFs, ConnectError> {
        let addr = super::resolve(&p.host, p.port)?;
        let seen = Arc::new(Mutex::new(None));
        let fail = |e: FtpError| match seen.lock().unwrap().take() {
            Some(cert) => ConnectError::Certificate(cert),
            None => ConnectError::Other(format!("{}:{}: {}", p.host, p.port, describe(&e))),
        };
        let mut ftp = if p.protocol == Protocol::FtpsImplicit {
            RustlsFtpStream::connect_secure_implicit(addr, connector(p, trust, &seen), &p.host).map_err(fail)?
        } else {
            RustlsFtpStream::connect_timeout(addr, TIMEOUT).map_err(fail)?
        };
        {
            let sock = ftp.get_ref();
            let _ = sock.set_read_timeout(Some(TIMEOUT));
            let _ = sock.set_write_timeout(Some(TIMEOUT));
        }
        if p.protocol == Protocol::Ftps {
            ftp = ftp.into_secure(connector(p, trust, &seen), &p.host).map_err(fail)?;
        }
        if p.protocol == Protocol::FtpsImplicit {
            // Most implicit-TLS servers also expect encrypted data connections.
            let _ = ftp.custom_command("PBSZ 0", &[Status::CommandOk]);
            let _ = ftp.custom_command("PROT P", &[Status::CommandOk]);
        }
        // Data connections get the same timeouts as the control connection.
        ftp = ftp.passive_stream_builder(|addr| {
            let s = TcpStream::connect_timeout(&addr, TIMEOUT).map_err(FtpError::ConnectionError)?;
            let _ = s.set_read_timeout(Some(TIMEOUT));
            let _ = s.set_write_timeout(Some(TIMEOUT));
            Ok(s)
        });
        if p.passive {
            // Servers behind NAT often announce their private address in PASV
            // replies; connect to the address we already talk to instead.
            ftp.set_mode(if addr.is_ipv6() { Mode::ExtendedPassive } else { Mode::Passive });
            ftp.set_passive_nat_workaround(true);
        } else {
            ftp = ftp.active_mode(TIMEOUT);
        }
        match ftp.login(p.login_user(), p.login_password()) {
            Ok(()) => {}
            Err(FtpError::UnexpectedResponse(r)) => return Err(ConnectError::Auth(reply_text(&r))),
            Err(e) => return Err(fail(e)),
        }
        ftp.transfer_type(FileType::Binary).map_err(fail)?;
        let feat = ftp.feat().unwrap_or_default();
        let has = |k: &str| feat.keys().any(|f| f.eq_ignore_ascii_case(k));
        if has("UTF8") {
            let _ = ftp.opts("UTF8", Some("ON"));
        }
        let home = ftp.pwd().unwrap_or_else(|_| "/".into());
        let mut fs = FtpFs { mlsd: has("MLSD") || has("MLST"), mfmt: has("MFMT"), home: super::normalize(&home, "/"), ftp, broken: false };
        if !p.remote_dir.is_empty() {
            let dir = super::normalize(&p.remote_dir, &fs.home);
            fs.ftp.cwd(&dir).map_err(|e| ConnectError::Other(format!("{dir}: {}", describe(&e))))?;
            fs.home = dir;
        }
        Ok(fs)
    }

    fn check(&self) -> io::Result<()> {
        if self.broken {
            return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "connection reset after a cancelled transfer"));
        }
        Ok(())
    }

    fn list_lines(&mut self, dir: &str) -> io::Result<Vec<RemoteEntry>> {
        self.check()?;
        // Changing into the directory first copes with servers that
        // misinterpret paths containing spaces in LIST arguments.
        self.ftp.cwd(dir).map_err(io_err)?;
        if self.mlsd {
            match self.ftp.mlsd(None) {
                Ok(lines) => return Ok(lines.iter().filter_map(|l| parse_mlsx(l)).collect()),
                Err(FtpError::UnexpectedResponse(r)) if (500..=504).contains(&r.status.code()) => self.mlsd = false,
                Err(e) => return Err(io_err(e)),
            }
        }
        let lines = self.ftp.list(None).map_err(io_err)?;
        Ok(lines.iter().filter_map(|l| parse_list_line(l)).collect())
    }
}

impl RemoteFs for FtpFs {
    fn home(&mut self) -> io::Result<String> {
        Ok(self.home.clone())
    }

    fn list(&mut self, dir: &str) -> io::Result<Vec<RemoteEntry>> {
        self.list_lines(dir)
    }

    fn download(&mut self, path: &str, out: &mut dyn Write, progress: Progress) -> io::Result<()> {
        self.check()?;
        let mut stream = self.ftp.retr_as_stream(path).map_err(io_err)?;
        match super::copy_with_progress(&mut stream, out, progress) {
            Ok(()) => stream.finish().map_err(io_err),
            Err(e) => {
                let _ = self.ftp.abort(stream);
                self.broken = true;
                Err(e)
            }
        }
    }

    fn upload(&mut self, path: &str, input: &mut dyn Read, progress: Progress) -> io::Result<()> {
        self.check()?;
        let mut stream = self.ftp.put_with_stream(path).map_err(io_err)?;
        match super::copy_with_progress(input, &mut stream, progress).and_then(|_| stream.flush()) {
            Ok(()) => stream.finish().map_err(io_err),
            Err(e) => {
                let _ = self.ftp.abort(stream);
                self.broken = true;
                Err(e)
            }
        }
    }

    fn mkdir(&mut self, path: &str) -> io::Result<()> {
        self.check()?;
        self.ftp.mkdir(path).map_err(io_err)
    }

    fn remove_file(&mut self, path: &str) -> io::Result<()> {
        self.check()?;
        self.ftp.rm(path).map_err(io_err)
    }

    fn remove_dir(&mut self, path: &str) -> io::Result<()> {
        self.check()?;
        self.ftp.rmdir(path).map_err(io_err)
    }

    fn rename(&mut self, from: &str, to: &str) -> io::Result<()> {
        self.check()?;
        self.ftp.rename(from, to).map_err(io_err)
    }

    fn set_modified(&mut self, path: &str, time: SystemTime) -> io::Result<()> {
        self.check()?;
        if !self.mfmt {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "server cannot set file times"));
        }
        let t: chrono::DateTime<chrono::Utc> = time.into();
        self.ftp.custom_command(format!("MFMT {} {path}", t.format("%Y%m%d%H%M%S")), &[Status::File]).map(|_| ()).map_err(io_err)
    }
}

impl Drop for FtpFs {
    fn drop(&mut self) {
        let _ = self.ftp.quit();
    }
}

fn reply_text(r: &suppaftp::types::Response) -> String {
    let body = String::from_utf8_lossy(&r.body).trim().to_string();
    if body.is_empty() { format!("{}", r.status.code()) } else { body }
}

fn describe(e: &FtpError) -> String {
    match e {
        FtpError::UnexpectedResponse(r) => reply_text(r),
        FtpError::ConnectionError(io) => io.to_string(),
        other => other.to_string(),
    }
}

/// Maps FTP errors onto I/O errors, keeping the kinds that mean "not found"
/// or "disconnected" recognizable.
fn io_err(e: FtpError) -> io::Error {
    match e {
        FtpError::ConnectionError(io) => io,
        FtpError::UnexpectedResponse(r) => {
            let kind = match r.status.code() {
                421 | 426 => io::ErrorKind::ConnectionAborted,
                550 => io::ErrorKind::NotFound,
                530 => io::ErrorKind::PermissionDenied,
                _ => io::ErrorKind::Other,
            };
            let text = reply_text(&r);
            // 550 is also "permission denied" or "not a directory": keep the text.
            io::Error::new(kind, text)
        }
        other => io::Error::other(other.to_string()),
    }
}

// ---- listings ---------------------------------------------------------------------

/// Parses one MLSD line (RFC 3659): `type=file;size=12;modify=20240102030405; name`.
pub fn parse_mlsx(line: &str) -> Option<RemoteEntry> {
    let line = line.trim_end_matches(['\r', '\n']);
    let (facts, name) = match line.find("; ") {
        Some(i) => (&line[..i], &line[i + 2..]),
        None => ("", line.strip_prefix(' ')?),
    };
    let mut e = RemoteEntry { name: name.to_string(), is_dir: false, is_link: false, size: 0, modified: None, mode: None };
    for fact in facts.split(';') {
        let Some((k, v)) = fact.split_once('=') else { continue };
        match k.to_ascii_lowercase().as_str() {
            "type" => {
                let v = v.to_ascii_lowercase();
                match v.as_str() {
                    "cdir" | "pdir" => return None,
                    "dir" => e.is_dir = true,
                    "file" => {}
                    // "OS.unix=slink:/target", "OS.unix=symlink", "link": usually directories.
                    t if t.contains("link") => {
                        e.is_link = true;
                        e.is_dir = true;
                    }
                    _ => {}
                }
            }
            "size" | "sizd" => e.size = v.parse().unwrap_or(0),
            "modify" => e.modified = parse_mdtm(v),
            "unix.mode" => e.mode = u32::from_str_radix(v.trim_start_matches('0'), 8).ok().or(Some(0)),
            _ => {}
        }
    }
    (!e.name.is_empty() && e.name != "." && e.name != "..").then_some(e)
}

/// `YYYYMMDDHHMMSS[.sss]` in UTC.
fn parse_mdtm(v: &str) -> Option<SystemTime> {
    let digits = v.split('.').next()?;
    let dt = chrono::NaiveDateTime::parse_from_str(digits, "%Y%m%d%H%M%S").ok()?;
    Some(dt.and_utc().into())
}

/// Parses a LIST line in Unix (`ls -l`) or DOS/IIS format.
pub fn parse_list_line(line: &str) -> Option<RemoteEntry> {
    use suppaftp::list::ListParser;
    let line = line.trim_end_matches(['\r', '\n']);
    if line.is_empty() || line.starts_with("total ") {
        return None;
    }
    let f = ListParser::parse_posix(line).or_else(|_| ListParser::parse_dos(line)).ok()?;
    let mode = line
        .as_bytes()
        .first()
        .filter(|c| matches!(c, b'-' | b'd' | b'l'))
        .map(|_| unix_mode(&line[1..10.min(line.len())]));
    let e = RemoteEntry {
        name: f.name().to_string(),
        is_dir: f.is_directory() || f.is_symlink(),
        is_link: f.is_symlink(),
        size: f.size() as u64,
        modified: Some(f.modified()),
        mode,
    };
    (e.name != "." && e.name != "..").then_some(e)
}

fn unix_mode(perms: &str) -> u32 {
    perms.chars().take(9).enumerate().fold(0, |m, (i, c)| if c == '-' || c == 'S' || c == 'T' { m } else { m | 1 << (8 - i) })
}

// ---- TLS ----------------------------------------------------------------------------

fn connector(p: &ConnParams, trust: &Trust, seen: &Arc<Mutex<Option<CertInfo>>>) -> RustlsConnector {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = PinningVerifier { host: p.host.clone(), port: p.port, trust: trust.clone(), seen: seen.clone(), provider: provider.clone() };
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("TLS versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    RustlsConnector::from(Arc::new(config))
}

fn root_store() -> Arc<rustls::RootCertStore> {
    static ROOTS: OnceLock<Arc<rustls::RootCertStore>> = OnceLock::new();
    ROOTS
        .get_or_init(|| {
            let mut store = rustls::RootCertStore::empty();
            let found = rustls_native_certs::load_native_certs();
            let _ = store.add_parsable_certificates(found.certs);
            Arc::new(store)
        })
        .clone()
}

/// Windows' trusted roots first; otherwise a fingerprint the user accepted.
#[derive(Debug)]
struct PinningVerifier {
    host: String,
    port: u16,
    trust: Trust,
    seen: Arc<Mutex<Option<CertInfo>>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for PinningVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = super::trust::cert_fingerprint(end_entity.as_ref());
        if self.trust.accepts(&fp) || self.trust.store.cert_trusted(&self.host, self.port, &fp) {
            return Ok(ServerCertVerified::assertion());
        }
        let result = rustls::client::WebPkiServerVerifier::builder_with_provider(root_store(), self.provider.clone())
            .build()
            .map_err(|e| rustls::Error::General(e.to_string()))
            .and_then(|v| v.verify_server_cert(end_entity, intermediates, server_name, ocsp, now));
        if let Err(e) = &result {
            *self.seen.lock().unwrap() = Some(CertInfo { host: self.host.clone(), port: self.port, fingerprint: fp, reason: cert_reason(e) });
        }
        result
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

fn cert_reason(e: &rustls::Error) -> String {
    use rustls::CertificateError as C;
    match e {
        rustls::Error::InvalidCertificate(C::UnknownIssuer) => "it is not issued by a trusted authority (self-signed?)".into(),
        rustls::Error::InvalidCertificate(C::NotValidForName) | rustls::Error::InvalidCertificate(C::NotValidForNameContext { .. }) => {
            "it was issued for a different host name".into()
        }
        rustls::Error::InvalidCertificate(C::Expired) | rustls::Error::InvalidCertificate(C::ExpiredContext { .. }) => "it has expired".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mlsd_lines() {
        let e = parse_mlsx("type=file;size=1234;modify=20240102030405;UNIX.mode=0644; my file.txt").unwrap();
        assert_eq!((e.name.as_str(), e.size, e.is_dir, e.mode), ("my file.txt", 1234, false, Some(0o644)));
        let t: chrono::DateTime<chrono::Utc> = e.modified.unwrap().into();
        assert_eq!(t.format("%Y-%m-%d %H:%M:%S").to_string(), "2024-01-02 03:04:05");
        let d = parse_mlsx("type=dir;modify=20240102030405.123; sub;dir").unwrap();
        assert!(d.is_dir);
        assert_eq!(d.name, "sub;dir");
        assert!(parse_mlsx("type=cdir; .").is_none());
        assert!(parse_mlsx("type=pdir; ..").is_none());
        let l = parse_mlsx("type=OS.unix=slink:/srv/pub;modify=20240102030405; pub").unwrap();
        assert!(l.is_link && l.is_dir);
    }

    #[test]
    fn parses_list_lines() {
        let e = parse_list_line("-rw-r--r--    1 1000     1000         5120 Mar 14  2023 report.pdf").unwrap();
        assert_eq!((e.name.as_str(), e.size, e.is_dir, e.mode), ("report.pdf", 5120, false, Some(0o644)));
        let d = parse_list_line("drwxr-xr-x    2 ftp      ftp          4096 Jan  5 10:42 My Photos").unwrap();
        assert!(d.is_dir);
        assert_eq!(d.name, "My Photos");
        let l = parse_list_line("lrwxrwxrwx    1 0        0              10 Jan  5 10:42 www -> /var/www").unwrap();
        assert_eq!(l.name, "www");
        assert!(l.is_link);
        let w = parse_list_line("10-19-20  03:19PM       <DIR>          pub").unwrap();
        assert!(w.is_dir);
        let f = parse_list_line("04-08-14  03:09PM                  403 readme.txt").unwrap();
        assert_eq!((f.name.as_str(), f.size), ("readme.txt", 403));
        assert!(parse_list_line("total 12").is_none());
    }
}
