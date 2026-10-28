// Embeds the icon and version info into redline.exe on Windows.
fn main() {
    println!("cargo:rerun-if-changed=redline.rc");
    println!("cargo:rerun-if-changed=assets/redline.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // The .rc sits at the crate root so "assets/redline.ico" resolves whether the
        // resource compiler searches from the .rc's folder or the working directory.
        // A real compile failure stops the build; a missing compiler (cross `cargo check`) doesn't.
        embed_resource::compile("redline.rc", embed_resource::NONE).manifest_optional().expect("compiling redline.rc");
    }
}
