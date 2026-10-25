//! Embeds the application icon, version info and manifest into MCommander.exe.

fn main() {
    println!("cargo:rerun-if-changed=res/mcommander.rc");
    println!("cargo:rerun-if-changed=res/mcommander.ico");
    println!("cargo:rerun-if-changed=res/mcommander.exe.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    embed_resource::compile("res/mcommander.rc", embed_resource::NONE)
        .manifest_required()
        .expect("failed to compile Windows resources (rc.exe from the Windows SDK is required)");
    // The manifest comes from the resource script; don't let link.exe add another.
    println!("cargo:rustc-link-arg-bins=/MANIFEST:NO");
}
