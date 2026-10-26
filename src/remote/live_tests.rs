//! End-to-end tests against local FTP, FTPS and SFTP servers.

#[path = "../../tests/support/servers.rs"]
pub mod servers;

use super::*;
use std::time::Duration;

pub fn trust_in(dir: &std::path::Path) -> Trust {
    Trust { store: TrustStore { dir: Some(dir.to_path_buf()), use_openssh: false }, accepted: Vec::new() }
}

fn download(fs: &mut dyn RemoteFs, path: &str) -> Vec<u8> {
    let mut out = Vec::new();
    fs.download(path, &mut out, &mut |_| true).unwrap();
    out
}

/// The same checks for every protocol: list, upload, download, mkdir,
/// rename, stat, times and delete.
fn exercise(fs: &mut dyn RemoteFs, root: &std::path::Path) {
    let home = fs.home().unwrap();
    assert_eq!(home, "/");
    let list = fs.list("/").unwrap();
    let names: Vec<&str> = list.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"hello.txt") && names.contains(&"sub dir"), "{names:?}");
    let hello = list.iter().find(|e| e.name == "hello.txt").unwrap();
    assert_eq!((hello.size, hello.is_dir), (11, false));
    assert!(hello.modified.is_some());
    assert!(list.iter().find(|e| e.name == "sub dir").unwrap().is_dir);
    assert_eq!(download(fs, "/hello.txt"), b"hello world");
    assert_eq!(fs.list("/sub dir").unwrap().len(), 1);

    // Upload a file with a non-ASCII name; progress reports the bytes.
    let data: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let mut seen = 0;
    fs.upload("/sub dir/rä ksmörgås.bin", &mut &data[..], &mut |n| {
        seen = n;
        true
    })
    .unwrap();
    assert_eq!(seen, data.len() as u64);
    assert_eq!(std::fs::read(root.join("sub dir").join("rä ksmörgås.bin")).unwrap(), data);
    assert_eq!(download(fs, "/sub dir/rä ksmörgås.bin"), data);

    fs.mkdir("/new").unwrap();
    assert!(root.join("new").is_dir());
    fs.rename("/hello.txt", "/new/moved.txt").unwrap();
    assert!(root.join("new").join("moved.txt").is_file());
    let moved = fs.list("/new").unwrap();
    assert_eq!((moved.len(), moved[0].name.as_str(), moved[0].size), (1, "moved.txt", 11));
    let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    if fs.set_modified("/new/moved.txt", t).is_ok() {
        let m = std::fs::metadata(root.join("new").join("moved.txt")).unwrap().modified().unwrap();
        assert_eq!(m, t);
    }
    let missing = fs.download("/nope.txt", &mut Vec::new(), &mut |_| true).unwrap_err();
    assert_ne!(missing.kind(), io::ErrorKind::Interrupted);
    fs.remove_file("/new/moved.txt").unwrap();
    fs.remove_dir("/new").unwrap();
    assert!(!root.join("new").exists());

    // A cancelled download stops with Interrupted; the session is either
    // still usable or reports a disconnect (so Connection reconnects).
    let err = fs.download("/sub dir/rä ksmörgås.bin", &mut Vec::new(), &mut |_| false).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::Interrupted);
    if let Err(e) = fs.list("/") {
        assert!(is_disconnect(&e), "{e}");
    }
}

pub fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.txt"), b"hello world").unwrap();
    std::fs::create_dir(dir.path().join("sub dir")).unwrap();
    std::fs::write(dir.path().join("sub dir").join("inner.txt"), b"inner").unwrap();
    dir
}

#[test]
fn ftp_end_to_end() {
    let root = fixture();
    let port = servers::ftp(root.path(), None);
    let mut p = ConnParams::new(Protocol::Ftp, "127.0.0.1");
    p.port = port;
    let state = tempfile::tempdir().unwrap();
    let mut fs = connect(&p, &trust_in(state.path())).unwrap();
    exercise(fs.as_mut(), root.path());

    // Active mode works too.
    p.passive = false;
    let mut fs = connect(&p, &trust_in(state.path())).unwrap();
    assert!(fs.list("/").unwrap().iter().any(|e| e.name == "sub dir"));
}

#[test]
fn ftps_asks_about_self_signed_certificates() {
    let root = fixture();
    let certs = tempfile::tempdir().unwrap();
    let tls = servers::self_signed(certs.path());
    let port = servers::ftp(root.path(), Some(&tls));
    let mut p = ConnParams::new(Protocol::Ftps, "127.0.0.1");
    p.port = port;
    p.user = "test".into();
    p.password = Some(Secret("test".into()));
    let state = tempfile::tempdir().unwrap();
    let mut trust = trust_in(state.path());
    let info = match connect(&p, &trust) {
        Err(ConnectError::Certificate(info)) => info,
        Err(e) => panic!("expected a certificate question, got {e}"),
        Ok(_) => panic!("a self-signed certificate must not be trusted silently"),
    };
    assert_eq!(info.fingerprint, trust::cert_fingerprint(&tls.der));
    assert!(info.reason.contains("trusted authority"), "{}", info.reason);

    // Accepted once: data connections use TLS with the same pinned certificate.
    trust.accepted.push(info.fingerprint.clone());
    let mut fs = connect(&p, &trust).unwrap();
    exercise(fs.as_mut(), root.path());

    // Accepted permanently: stored next to the settings.
    let trust = trust_in(state.path());
    trust.store.save_cert("127.0.0.1", port, &info.fingerprint).unwrap();
    assert!(connect(&p, &trust).is_ok());
}

#[test]
fn sftp_end_to_end() {
    let root = fixture();
    let server = servers::sftp(root.path(), "anna", "s3cret");
    let mut p = ConnParams::new(Protocol::Sftp, "127.0.0.1");
    p.port = server.port;
    p.user = "anna".into();
    p.password = Some(Secret("s3cret".into()));
    // Keep the user's own ~/.ssh keys out of the test.
    p.key_file = Some(std::path::PathBuf::from("does-not-exist"));
    let state = tempfile::tempdir().unwrap();
    let trust = trust_in(state.path());

    let info = match connect(&p, &trust) {
        Err(ConnectError::HostKey(info)) => info,
        Err(e) => panic!("expected a host key question, got {e}"),
        Ok(_) => panic!("an unknown host key must not be trusted silently"),
    };
    assert_eq!(info.fingerprint, trust::ssh_fingerprint(&server.host_key));
    assert!(!info.changed);
    trust.store.save_host_key("127.0.0.1", server.port, &info.openssh).unwrap();

    let mut bad = p.clone();
    bad.password = Some(Secret("wrong".into()));
    assert!(matches!(connect(&bad, &trust), Err(ConnectError::Auth(_))));

    let mut fs = connect(&p, &trust).unwrap();
    exercise(fs.as_mut(), root.path());

    // A different key for the same host is reported as changed.
    let other = TrustStore { dir: Some(state.path().to_path_buf()), use_openssh: false };
    other.save_host_key("127.0.0.1", server.port, &trust::tests::test_key(9).to_openssh().unwrap()).unwrap();
    match connect(&p, &trust_in(state.path())) {
        Err(ConnectError::HostKey(info)) => assert!(info.changed),
        Err(e) => panic!("expected a changed host key, got {e}"),
        Ok(_) => panic!("a changed host key must not be accepted"),
    }
}

#[test]
fn connection_reconnects_after_close() {
    let root = fixture();
    let port = servers::ftp(root.path(), None);
    let mut p = ConnParams::new(Protocol::Ftp, "127.0.0.1");
    p.port = port;
    let state = tempfile::tempdir().unwrap();
    let trust = trust_in(state.path());
    let conn = Connection::new(p.clone(), "test".into(), trust.clone(), connect(&p, &trust).unwrap());
    assert_eq!(conn.with(|fs| fs.list("/")).unwrap().len(), 2);
    conn.close();
    assert_eq!(conn.with(|fs| fs.list("/sub dir")).unwrap().len(), 1);
    let mut extra = conn.borrow_session().unwrap();
    assert_eq!(download(extra.as_mut(), "/sub dir/inner.txt"), b"inner");

    // After a cancelled transfer the next call still works.
    let cancelled = conn.with(|fs| fs.download("/hello.txt", &mut Vec::new(), &mut |_| false));
    assert_eq!(cancelled.unwrap_err().kind(), io::ErrorKind::Interrupted);
    assert_eq!(conn.with(|fs| fs.list("/")).unwrap().len(), 2);
}
