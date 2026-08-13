fn main() {
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let icons = manifest.join("icons");
    let png = std::fs::read(icons.join("icon.png")).expect("failed to read icons/icon.png");

    // Tauri's context generation rejects non-RGBA PNG icons on release runners.
    // PNG IHDR byte 25 is the color type; 6 means truecolor with alpha (RGBA).
    assert!(
        png.len() > 25 && &png[..8] == b"\x89PNG\r\n\x1a\n" && png[25] == 6,
        "icons/icon.png must be an RGBA PNG (PNG color type 6)"
    );

    let windows = tauri_build::WindowsAttributes::new().window_icon_path(icons.join("icon.ico"));
    let attributes = tauri_build::Attributes::new().windows_attributes(windows);
    tauri_build::try_build(attributes).expect("failed to run tauri build script");
}
