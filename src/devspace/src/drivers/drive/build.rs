// devspace/build.rs — Cargo build script
//
// Links external objects and sets linker script.
// crytic.o (NASM) is optional — inline Rust ASM fallback is used if missing.
// drive_odin.o (Odin) is optional — Odin drive functions called via FFI when present.

use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let build_dir = manifest_dir.join("../../build");

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/drivers/drive/crytic.asm");
    println!("cargo:rerun-if-changed=src/drivers/drive/drive.odin");
    println!("cargo:rerun-if-changed=src/drivers/drive/drive_def.fs");
    println!("cargo:rerun-if-changed=src/drivers/drive/drive_logic.fs");

    // Custom linker script (optional — only if file exists)
    let linker_ld = manifest_dir.join("linker.ld");
    if linker_ld.exists() {
        println!("cargo:rustc-link-arg=-T{}", linker_ld.display());
    }

    // crytic.o — optional, inline ASM fallback used if missing
    let crytic = build_dir.join("crytic.o");
    if crytic.exists() {
        println!("cargo:rustc-link-arg={}", crytic.display());
    }

    // drive_odin.o — optional
    let odin_obj = build_dir.join("drive_odin.o");
    if odin_obj.exists() {
        println!("cargo:rustc-link-arg={}", odin_obj.display());
    }
}