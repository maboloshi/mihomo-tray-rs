use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=app.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest: PathBuf = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("app.manifest");
    if !manifest.exists() {
        return;
    }
    // Embed the application manifest (Common-Controls v6, supportedOS, per-monitor
    // DPI) with plain linker arguments so no extra build dependency is needed.
    println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
    println!("cargo:rustc-link-arg-bins=/MANIFESTUAC:NO");
    println!(
        "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
