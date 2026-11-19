use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/blink.manifest");
    println!("cargo:rerun-if-changed=assets/blink.ico");
    println!("cargo:rerun-if-changed=assets/blink-tray.ico");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        // Generated so the version info follows Cargo.toml. Windows shows FileDescription as
        // the app name (notifications, Task Manager); without it, it shows "Blink.exe".
        let assets = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets");
        let asset = |name: &str| assets.join(name).display().to_string().replace('\\', "/");
        let version = std::env::var("CARGO_PKG_VERSION").unwrap();
        let mut parts = version.split(['.', '-', '+']).map(|p| p.parse::<u16>().unwrap_or(0));
        let mut part = || parts.next().unwrap_or(0);
        let numeric = format!("{},{},{},0", part(), part(), part());
        let rc = format!(
            r#"#define RT_MANIFEST 24
1 ICON "{icon}"
2 ICON "{tray}"
1 RT_MANIFEST "{manifest}"

1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "FileDescription", "Blink"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "Blink"
      VALUE "OriginalFilename", "Blink.exe"
      VALUE "ProductName", "Blink"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
            icon = asset("blink.ico"),
            tray = asset("blink-tray.ico"),
            manifest = asset("blink.manifest"),
        );
        let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("blink.rc");
        std::fs::write(&out, rc).expect("failed to write the Windows resource script");
        embed_resource::compile(&out, embed_resource::NONE)
            .manifest_optional()
            .expect("failed to compile Windows resources");
    }
}
