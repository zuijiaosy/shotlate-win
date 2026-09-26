//! Windows resources: icon, manifest (Per-Monitor V2 DPI, Common Controls 6) and version info.
//! The resource script is generated so its version always matches Cargo.toml, which is also what
//! WinSparkle compares with the appcast.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=res");
    println!("cargo:rerun-if-changed=Cargo.toml");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let rc = write_rc();
    compile(&rc);
}

fn write_rc() -> PathBuf {
    let dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("res");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts: Vec<u32> = version.split(['.', '-']).filter_map(|p| p.parse().ok()).collect();
    parts.resize(4, 0);
    let commas = parts.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    // Forward slashes work for both rc.exe and windres, and survive the escaping rules of both.
    let path = |f: &str| dir.join(f).display().to_string().replace('\\', "/");
    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "{icon}"
1 24 "{manifest}"

1 VERSIONINFO
FILEVERSION {commas}
PRODUCTVERSION {commas}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "080404b0"
    BEGIN
      VALUE "CompanyName", "Shotlate"
      VALUE "FileDescription", "Shotlate"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "Shotlate"
      VALUE "LegalCopyright", "MIT License"
      VALUE "OriginalFilename", "Shotlate.exe"
      VALUE "ProductName", "Shotlate"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x804, 1200
  END
END
"#,
        icon = path("shotlate.ico"),
        manifest = path("shotlate.manifest"),
    );
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("shotlate.rc");
    std::fs::write(&out, rc).unwrap();
    out
}

#[cfg(windows)]
fn compile(rc: &PathBuf) {
    embed_resource::compile(rc, embed_resource::NONE).manifest_optional().unwrap();
}

/// Cross builds from macOS or Linux: the mingw windres turns the script into a COFF object linked into the exe.
#[cfg(not(windows))]
fn compile(rc: &PathBuf) {
    let target = std::env::var("TARGET").unwrap_or_default();
    let windres = if target.starts_with("aarch64") { "aarch64-w64-mingw32-windres" } else { "x86_64-w64-mingw32-windres" };
    let obj = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("shotlate-res.o");
    let status = std::process::Command::new(windres).arg("--input").arg(rc).args(["--output-format=coff", "--output"]).arg(&obj).status();
    match status {
        Ok(s) if s.success() => println!("cargo:rustc-link-arg-bins={}", obj.display()),
        _ => println!("cargo:warning={windres} not found; the exe has no icon, manifest or version info"),
    }
}
