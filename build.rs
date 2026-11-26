//! Embeds the Cronista Windows application icon and version metadata into `cronista.exe`.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/cronista.ico");
    println!("cargo:rerun-if-changed=build.rs");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico_path = manifest_dir.join("assets").join("cronista.ico");
    if !ico_path.is_file() {
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let rc_path = out_dir.join("cronista_res.rc");
    let res_obj = out_dir.join("cronista_res.o");

    let ico_escaped = ico_path.to_string_lossy().replace('\\', "\\\\");
    let rc_content = format!(
        r#"1 ICON "{ico_escaped}"

1 VERSIONINFO
FILEVERSION 1,0,0,0
PRODUCTVERSION 1,0,0,0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "Ignacio Chiappero\0"
            VALUE "FileDescription", "Cronista - Screen & Audio Recorder with Local Live Transcription\0"
            VALUE "FileVersion", "1.0.0.0\0"
            VALUE "InternalName", "Cronista\0"
            VALUE "LegalCopyright", "Copyright 2026 Ignacio Chiappero (Apache-2.0)\0"
            VALUE "OriginalFilename", "Cronista.exe\0"
            VALUE "ProductName", "Cronista\0"
            VALUE "ProductVersion", "1.0.0\0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    );

    if fs::write(&rc_path, rc_content).is_err() {
        return;
    }

    let status = Command::new("windres")
        .arg(&rc_path)
        .arg("-O")
        .arg("coff")
        .arg("-o")
        .arg(&res_obj)
        .status();

    if let Ok(st) = status {
        if st.success() && res_obj.is_file() {
            println!("cargo:rustc-link-arg-bin=cronista={}", res_obj.display());
        }
    }
}
