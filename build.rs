// Embeds the application icon, version, and Windows 10/11 manifest into the
// Windows executable. Does nothing for other targets.
fn main() {
    println!("cargo:rerun-if-changed=windows/wise-softphone.ico");
    println!("cargo:rerun-if-changed=windows/softphone.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("windows/wise-softphone.ico")
        .set_manifest_file("windows/softphone.manifest")
        .set("ProductName", "Wise Softphone")
        .set("FileDescription", "Wise Softphone")
        .set("InternalName", "softphone")
        .set("OriginalFilename", "softphone.exe");
    resource
        .compile()
        .expect("Could not embed Windows resources");
}
