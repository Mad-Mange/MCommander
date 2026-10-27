//! Embeds the application icon, version info and manifest into MCommander.exe.

fn main() {
    println!("cargo:rerun-if-changed=res/mcommander.rc");
    println!("cargo:rerun-if-changed=res/mcommander.ico");
    println!("cargo:rerun-if-changed=res/mcommander.exe.manifest");
    println!("cargo:rerun-if-changed=Cargo.toml");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    // The version resource shows the crate version ("0.5.0" -> 0,5,0,0).
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let mut parts: Vec<u32> = version.split(['.', '-', '+']).take(3).map(|p| p.parse().unwrap_or(0)).collect();
    parts.resize(4, 0);
    let numeric = parts.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let macros = [format!("MC_VERSION={numeric}"), format!("MC_VERSION_STR=\"{version}\"")];
    embed_resource::compile("res/mcommander.rc", &macros)
        .manifest_required()
        .expect("failed to compile Windows resources (rc.exe from the Windows SDK is required)");
    // The manifest comes from the resource script; don't let link.exe add another.
    println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
}
