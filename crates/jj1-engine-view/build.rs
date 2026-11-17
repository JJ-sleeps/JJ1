//! Embeds the application icon into the Windows executables.
//!
//! Done by hand with `windres` (GNU toolchain) to avoid another dependency.
//! If it cannot be done the build still succeeds, just without an icon.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/jj1.ico");
    println!("cargo:rerun-if-changed=build.rs");
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    if var("CARGO_CFG_TARGET_OS") != "windows" {
        return;
    }
    if var("CARGO_CFG_TARGET_ENV") != "gnu" {
        println!("cargo:warning=jj1: the app icon is only embedded on the windows-gnu toolchain so far");
        return;
    }
    let out = PathBuf::from(var("OUT_DIR"));
    let rc = out.join("jj1.rc");
    let obj = out.join("jj1-icon.o");
    let ico = Path::new(&var("CARGO_MANIFEST_DIR")).join("assets").join("jj1.ico");
    let script = format!("1 ICON \"{}\"\n", ico.display().to_string().replace('\\', "/"));
    if std::fs::write(&rc, script).is_err() {
        println!("cargo:warning=jj1: could not write the icon resource script");
        return;
    }
    // Use the windres that sits beside the configured linker, so a different
    // (possibly 32-bit) MinGW earlier on PATH is not picked up by accident.
    let windres = std::env::var_os("RUSTC_LINKER")
        .map(PathBuf::from)
        .and_then(|linker| linker.parent().map(|dir| dir.join("windres.exe")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("windres"));
    let done = Command::new(&windres).arg(&rc).args(["-O", "coff", "-o"]).arg(&obj).status();
    match done {
        Ok(s) if s.success() => println!("cargo:rustc-link-arg-bins={}", obj.display()),
        _ => println!("cargo:warning=jj1: windres failed ({}); building without an app icon", windres.display()),
    }
}
