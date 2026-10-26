//! Local servers for trying MCommander's FTP/SFTP support:
//!
//! ```text
//! cargo run --example test_servers [folder]
//! ```
//!
//! Serves `folder` (default: a demo folder in %TEMP%) on 127.0.0.1 as
//! FTP (port 2121, any login), FTPS with explicit TLS and a self-signed
//! certificate (port 2122, any login) and SFTP (port 2222, user `demo`,
//! password `demo`). Stop with Ctrl+C.

#[path = "../tests/support/servers.rs"]
mod servers;

use std::path::PathBuf;

fn main() {
    let root = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join("mcommander-servers"));
    if !root.exists() {
        std::fs::create_dir_all(root.join("Documents")).unwrap();
        std::fs::create_dir_all(root.join("Photos")).unwrap();
        std::fs::write(root.join("readme.txt"), "Welcome to the MCommander test server.\r\n").unwrap();
        std::fs::write(root.join("Documents").join("notes.md"), "# Notes\r\n\r\n- try F5, F6, F7 and F8\r\n").unwrap();
        std::fs::write(root.join("Photos").join("holiday.bin"), vec![0u8; 2_500_000]).unwrap();
    }
    let certs = std::env::temp_dir().join("mcommander-servers-tls");
    std::fs::create_dir_all(&certs).unwrap();
    let tls = servers::self_signed(&certs);
    servers::ftp_on(&root, None, 2121);
    servers::ftp_on(&root, Some(&tls), 2122);
    let sftp = servers::sftp_on(&root, "demo", "demo", 2222);
    println!("Serving {}", root.display());
    println!("  ftp://127.0.0.1:2121    (any user/password)");
    println!("  ftps://127.0.0.1:2122   (explicit TLS, self-signed certificate)");
    println!("  sftp://demo@127.0.0.1:2222   (password: demo)");
    println!("  SFTP host key: {}", sftp.host_key.fingerprint(russh::keys::HashAlg::Sha256));
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
