fn main() {
    // Tauri's context generator requires platform app icons even for a cdylib.
    // Generate tiny neutral icons so the library carries no application branding.
    const ICON: &[u8] = &[
        0, 0, 1, 0, 1, 0, 1, 1, 0, 0, 1, 0, 32, 0, 68, 0, 0, 0, 22, 0, 0, 0, 137, 80, 78, 71, 13,
        10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 4, 0, 0, 0, 181, 28,
        12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 100, 248, 15, 0, 1, 5, 1, 1, 39, 24, 227,
        102, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let icons = manifest.join("icons");
    let ico = icons.join("icon.ico");
    std::fs::create_dir_all(&icons).unwrap();
    std::fs::write(&ico, ICON).unwrap();
    // The ICO directory and entry occupy the first 22 bytes; its image is a
    // complete 1x1 PNG also accepted by the Linux and macOS context generator.
    std::fs::write(icons.join("icon.png"), &ICON[22..]).unwrap();
    let windows = tauri_build::WindowsAttributes::new().window_icon_path(ico);
    let attributes = tauri_build::Attributes::new().windows_attributes(windows);
    tauri_build::try_build(attributes).expect("failed to run tauri build script");
}
