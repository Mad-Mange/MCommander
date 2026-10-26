//! Remembered trust decisions: SSH host keys (OpenSSH `known_hosts` format)
//! and pinned certificates of FTPS servers that Windows doesn't trust
//! (self-signed certificates on home NAS boxes and the like).

use russh::keys::{HashAlg, PublicKey};
use std::io;
use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct TrustStore {
    /// Folder for `known_hosts` and `trusted_certs`; `None` remembers nothing.
    pub dir: Option<PathBuf>,
    /// Also accept host keys from the user's `~/.ssh/known_hosts` (read only).
    pub use_openssh: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum HostKeyCheck {
    Known,
    Unknown,
    /// A different key of the same type is on record.
    Changed,
}

impl TrustStore {
    /// Stores next to the settings file.
    pub fn beside(config_file: &std::path::Path) -> TrustStore {
        TrustStore { dir: config_file.parent().map(PathBuf::from), use_openssh: true }
    }

    fn file(&self, name: &str) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(name))
    }

    fn openssh_file() -> Option<PathBuf> {
        std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join(".ssh").join("known_hosts"))
    }

    pub fn check_host_key(&self, host: &str, port: u16, key: &PublicKey) -> HostKeyCheck {
        let mut files: Vec<PathBuf> = self.file("known_hosts").into_iter().collect();
        if self.use_openssh {
            files.extend(Self::openssh_file());
        }
        let mut changed = false;
        for f in files {
            match russh::keys::check_known_hosts_path(host, port, key, &f) {
                Ok(true) => return HostKeyCheck::Known,
                Ok(false) => {}
                Err(russh::keys::Error::KeyChanged { .. }) => changed = true,
                Err(_) => {}
            }
        }
        if changed { HostKeyCheck::Changed } else { HostKeyCheck::Unknown }
    }

    /// Records `key` for the host, replacing what our own file had for it.
    pub fn save_host_key(&self, host: &str, port: u16, openssh: &str) -> io::Result<()> {
        let Some(path) = self.file("known_hosts") else { return Ok(()) };
        let key = PublicKey::from_openssh(openssh).map_err(io::Error::other)?;
        let pattern = if port == 22 { host.to_string() } else { format!("[{host}]:{port}") };
        if let Ok(text) = std::fs::read_to_string(&path) {
            let kept: Vec<&str> = text
                .lines()
                .filter(|l| l.split(' ').next().is_none_or(|hosts| !hosts.split(',').any(|h| h == pattern)))
                .collect();
            std::fs::write(&path, if kept.is_empty() { String::new() } else { kept.join("\n") + "\n" })?;
        }
        russh::keys::known_hosts::learn_known_hosts_path(host, port, &key, &path).map_err(io::Error::other)
    }

    pub fn cert_trusted(&self, host: &str, port: u16, fingerprint: &str) -> bool {
        let Some(path) = self.file("trusted_certs") else { return false };
        let want = format!("{host}:{port} {fingerprint}");
        std::fs::read_to_string(path).is_ok_and(|t| t.lines().any(|l| l.trim() == want))
    }

    pub fn save_cert(&self, host: &str, port: u16, fingerprint: &str) -> io::Result<()> {
        let Some(path) = self.file("trusted_certs") else { return Ok(()) };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let prefix = format!("{host}:{port} ");
        let mut lines: Vec<String> = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.starts_with(&prefix))
            .map(str::to_string)
            .collect();
        lines.push(format!("{prefix}{fingerprint}"));
        std::fs::write(&path, lines.join("\n") + "\n")
    }
}

/// `SHA256:...` like `ssh-keygen -l`.
pub fn ssh_fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// SHA-256 of a DER certificate as `AB:CD:...`.
pub fn cert_fingerprint(der: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    #[test]
    fn remembers_host_keys_and_certificates() {
        let dir = tempfile::tempdir().unwrap();
        let store = TrustStore { dir: Some(dir.path().to_path_buf()), use_openssh: false };
        let (a, b) = (test_key(1), test_key(2));
        assert_eq!(store.check_host_key("nas", 2222, &a), HostKeyCheck::Unknown);
        store.save_host_key("nas", 2222, &a.to_openssh().unwrap()).unwrap();
        assert_eq!(store.check_host_key("nas", 2222, &a), HostKeyCheck::Known);
        assert_eq!(store.check_host_key("nas", 2222, &b), HostKeyCheck::Changed);
        assert_eq!(store.check_host_key("nas", 22, &a), HostKeyCheck::Unknown);
        // Accepting the new key replaces the old one.
        store.save_host_key("nas", 2222, &b.to_openssh().unwrap()).unwrap();
        assert_eq!(store.check_host_key("nas", 2222, &b), HostKeyCheck::Known);
        assert_eq!(store.check_host_key("nas", 2222, &a), HostKeyCheck::Changed);

        assert!(!store.cert_trusted("h", 21, "AA:BB"));
        store.save_cert("h", 21, "AA:BB").unwrap();
        assert!(store.cert_trusted("h", 21, "AA:BB"));
        store.save_cert("h", 21, "CC:DD").unwrap();
        assert!(!store.cert_trusted("h", 21, "AA:BB"));
        assert!(store.cert_trusted("h", 21, "CC:DD"));
        assert_eq!(cert_fingerprint(b"x").len(), 32 * 3 - 1);
    }

    pub fn test_key(seed: u8) -> PublicKey {
        use russh::keys::ssh_key::public::{Ed25519PublicKey, KeyData};
        PublicKey::new(KeyData::Ed25519(Ed25519PublicKey([seed; 32])), "")
    }
}
