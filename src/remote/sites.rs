//! Saved connections (the Ctrl+F list), their passwords in the Windows
//! Credential Manager, and import from Total Commander's `wcx_ftp.ini`.

use super::{ConnParams, Protocol, Secret};
use crate::ini::Ini;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub struct Site {
    pub name: String,
    /// Connection details; the password is never kept here.
    pub params: ConnParams,
    /// The password is stored in the Credential Manager.
    pub save_password: bool,
}

impl Site {
    pub fn new(name: &str, params: ConnParams) -> Site {
        Site { name: name.to_string(), params: ConnParams { password: None, ..params }, save_password: false }
    }

    /// Credential Manager target; keyed by the server, not the display name,
    /// so renaming a connection keeps its password.
    pub fn credential_target(params: &ConnParams) -> String {
        format!("MCommander:{}", params.url_prefix())
    }

    /// The parameters with the saved password filled in.
    pub fn resolved(&self) -> ConnParams {
        let mut p = self.params.clone();
        if self.save_password {
            p.password = read_credential(&Site::credential_target(&p)).map(Secret);
        }
        p
    }
}

const SECTION_PREFIX: &str = "Site";

pub fn from_ini(ini: &Ini) -> Vec<Site> {
    let mut out = Vec::new();
    for i in 1..1000 {
        let sec_name = format!("{SECTION_PREFIX}{i}");
        let Some(sec) = ini.section(&sec_name) else { break };
        let protocol = match sec.get("protocol").unwrap_or("ftp") {
            "ftps" => Protocol::Ftps,
            "ftpis" => Protocol::FtpsImplicit,
            "sftp" => Protocol::Sftp,
            _ => Protocol::Ftp,
        };
        let mut p = ConnParams::new(protocol, sec.get("host").unwrap_or(""));
        if let Some(port) = sec.get("port").and_then(|v| v.parse().ok()) {
            p.port = port;
        }
        p.user = sec.get("user").unwrap_or("").to_string();
        p.remote_dir = sec.get("dir").unwrap_or("").to_string();
        p.passive = sec.get("passive") != Some("0");
        p.key_file = sec.get("keyfile").filter(|k| !k.is_empty()).map(PathBuf::from);
        out.push(Site { name: sec.get("name").unwrap_or(&p.host).to_string(), params: p, save_password: sec.get("savepw") == Some("1") });
    }
    out
}

pub fn to_ini(ini: &mut Ini, sites: &[Site]) {
    for (i, s) in sites.iter().enumerate() {
        let sec = format!("{SECTION_PREFIX}{}", i + 1);
        let p = &s.params;
        ini.set(&sec, "name", &s.name);
        ini.set(&sec, "protocol", p.protocol.scheme());
        ini.set(&sec, "host", &p.host);
        ini.set(&sec, "port", p.port);
        ini.set(&sec, "user", &p.user);
        ini.set(&sec, "dir", &p.remote_dir);
        ini.set(&sec, "passive", p.passive as i32);
        ini.set(&sec, "keyfile", p.key_file.as_ref().map(|k| k.to_string_lossy().into_owned()).unwrap_or_default());
        ini.set(&sec, "savepw", s.save_password as i32);
    }
}

/// Location of Total Commander's FTP settings.
pub fn find_tc_ftp_ini() -> Option<PathBuf> {
    let main = crate::config::find_tc_ini()?;
    let p = main.with_file_name("wcx_ftp.ini");
    p.is_file().then_some(p)
}

/// Reads the connections of a `wcx_ftp.ini`. Passwords are not imported:
/// TC only scrambles them, and MCommander keeps passwords in the
/// Credential Manager instead.
pub fn import_tc(ini: &Ini) -> Vec<Site> {
    let Some(list) = ini.section("connections") else { return Vec::new() };
    let mut out = Vec::new();
    for (key, name) in &list.entries {
        if key.eq_ignore_ascii_case("default") || name.is_empty() {
            continue;
        }
        let Some(sec) = ini.section(name) else { continue };
        let host = sec.get("host").unwrap_or("").trim();
        if host.is_empty() || host.starts_with("\\\\\\") {
            continue; // File system plugin entries.
        }
        let Ok(mut p) = super::parse_url(host) else { continue };
        let flags = sec.get("SpecialFlags").and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(0);
        if p.protocol == Protocol::Ftp && flags & 1 != 0 {
            p.protocol = Protocol::FtpsImplicit;
            if p.port == Protocol::Ftp.default_port() {
                p.port = Protocol::FtpsImplicit.default_port();
            }
        } else if p.protocol == Protocol::Ftp && flags & 2 != 0 {
            p.protocol = Protocol::Ftps;
        }
        if sec.get("anonymous") != Some("1") {
            p.user = sec.get("username").unwrap_or("").to_string();
        }
        p.remote_dir = sec.get("directory").unwrap_or("").to_string();
        p.passive = sec.get("pasvmode") != Some("0");
        p.password = None;
        out.push(Site::new(name, p));
    }
    out
}

// ---- Windows Credential Manager -----------------------------------------------------

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn write_credential(target: &str, user: &str, secret: &str) -> std::io::Result<()> {
    use windows::Win32::Security::Credentials::{CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredWriteW};
    let mut target_w = wide(target);
    let mut user_w = wide(user);
    let blob: Vec<u8> = secret.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let cred = CREDENTIALW {
        Flags: CRED_FLAGS(0),
        Type: CRED_TYPE_GENERIC,
        TargetName: windows::core::PWSTR(target_w.as_mut_ptr()),
        CredentialBlobSize: blob.len() as u32,
        CredentialBlob: blob.as_ptr() as *mut u8,
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        UserName: windows::core::PWSTR(user_w.as_mut_ptr()),
        ..Default::default()
    };
    unsafe { CredWriteW(&cred, 0) }.map_err(|e| std::io::Error::other(e.to_string()))
}

pub fn read_credential(target: &str) -> Option<String> {
    use windows::Win32::Security::Credentials::{CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW};
    let target_w = wide(target);
    let mut ptr: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        CredReadW(windows::core::PCWSTR(target_w.as_ptr()), CRED_TYPE_GENERIC, None, &mut ptr).ok()?;
        let cred = &*ptr;
        let bytes = std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize);
        let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let secret = String::from_utf16_lossy(&units);
        CredFree(ptr as *const core::ffi::c_void);
        Some(secret)
    }
}

pub fn delete_credential(target: &str) {
    use windows::Win32::Security::Credentials::{CRED_TYPE_GENERIC, CredDeleteW};
    let target_w = wide(target);
    unsafe {
        let _ = CredDeleteW(windows::core::PCWSTR(target_w.as_ptr()), CRED_TYPE_GENERIC, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sites_round_trip_through_ini() {
        let mut p = super::super::parse_url("sftp://me@nas:2222/data").unwrap();
        p.key_file = Some(PathBuf::from(r"C:\keys\id_ed25519"));
        p.password = Some(Secret("not stored".into()));
        let mut a = Site::new("NAS", p);
        a.save_password = true;
        let b = Site::new("Mirror", super::super::parse_url("ftp.example.org").unwrap());
        let mut ini = Ini::default();
        to_ini(&mut ini, &[a.clone(), b.clone()]);
        let text = ini.to_string();
        assert!(!text.contains("not stored"));
        let back = from_ini(&Ini::parse(&text));
        assert_eq!(back, vec![a, b]);
    }

    #[test]
    fn imports_total_commander_connections() {
        let ini = Ini::parse(
            "[connections]\n1=Work\n2=Home NAS\n3=Plugin\ndefault=Work\n\
             [Work]\nhost=ftpes://ftp.work.example:2121\nusername=me\ndirectory=/pub\npasvmode=1\npassword=0123456789\n\
             [Home NAS]\nhost=192.168.1.20\nanonymous=1\nusername=x\npasvmode=0\nSpecialFlags=1\n\
             [Plugin]\nhost=\\\\\\SFTP\\server\n",
        );
        let sites = import_tc(&ini);
        assert_eq!(sites.len(), 2);
        let w = &sites[0];
        assert_eq!((w.name.as_str(), w.params.protocol, w.params.port), ("Work", Protocol::Ftps, 2121));
        assert_eq!((w.params.user.as_str(), w.params.remote_dir.as_str()), ("me", "/pub"));
        assert!(w.params.password.is_none());
        let h = &sites[1];
        assert_eq!((h.params.protocol, h.params.user.as_str(), h.params.passive), (Protocol::FtpsImplicit, "", false));
    }

    #[test]
    fn credentials_round_trip() {
        let target = format!("MCommander-test:{}", std::process::id());
        write_credential(&target, "user", "pässwörd ✓").unwrap();
        assert_eq!(read_credential(&target).as_deref(), Some("pässwörd ✓"));
        delete_credential(&target);
        assert_eq!(read_credential(&target), None);
    }
}
