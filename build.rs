//! Embeds the Windows version resource, so the exe's Properties → Details show the
//! version from Cargo.toml (FileVersion / ProductVersion are filled in by winresource).

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set("ProductName", "DiskSpaceMap")
        .set("FileDescription", "DiskSpaceMap")
        .set("OriginalFilename", "DiskSpaceMap.exe")
        .set("LegalCopyright", "Copyright (c) 2026 qooxlu");
    res.compile().expect("failed to compile Windows resources");
}
