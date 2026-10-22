fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    println!("cargo:rerun-if-changed=app.manifest");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon.ico")
            .set("ProductName", "PlexDio")
            .set("FileDescription", "PlexDio - lightweight Plex music player")
            .set("CompanyName", "PlexDio")
            .set("LegalCopyright", "MIT")
            .set_manifest_file("app.manifest");
        if let Err(e) = res.compile() {
            panic!("failed to compile Windows resources: {e}");
        }
    }
}
