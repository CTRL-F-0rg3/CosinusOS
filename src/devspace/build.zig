const std = @import("std");

pub fn build(b: *std.Build) void {
    const build_dir = "../../build";
    const iso_boot_dir = "../../iso/boot";
    const ds_target = build_dir ++ "/devspace_target";

    // ── 1. Compile Odin drive.odin → drive_odin.o ────────────────────────────
    // Odin compiles to an object file that Rust/lld links in.
    // -target:linux_amd64 produces elf64 object compatible with our linker.
    // -build-mode:obj = only compile, do not link.
    const odin_drive = b.addSystemCommand(&.{
        "odin",                         "build",
        "src/drivers/drive/drive.odin",
        "-file", // single file, not package
        "-target:linux_amd64",
        "-build-mode:obj",
        "-out:" ++ build_dir ++ "/drive_odin.o",
        "-o:speed",
        "-no-crt", // no libc, freestanding
        "-disable-assert",
        "-no-bounds-check",
    });

    // ── 2. Assemble crytic.asm → crytic.o ────────────────────────────────────
    const nasm_crytic = b.addSystemCommand(&.{
        "nasm",                         "-f", "elf64",
        "src/drivers/drive/crytic.asm", "-o", build_dir ++ "/crytic.o",
    });

    // ── 3. Embed Forth source files into Rust binary ──────────────────────────
    // drive_def.fs and drive_logic.fs are embedded as &[u8] via include_bytes!
    // in mod.rs — no separate compilation step needed.
    // We just verify the files exist before cargo runs.
    const check_forth = b.addSystemCommand(&.{
        "sh",                                                                                                                             "-c",
        "test -f src/drivers/drive/drive_def.fs && " ++ "test -f src/drivers/drive/drive_logic.fs && " ++ "echo '[DS] Forth sources OK'",
    });

    // ── 4. Compile Rust devspace crate ───────────────────────────────────────
    const cargo_ds = b.addSystemCommand(&.{
        "cargo",           "+nightly",             "build",        "--release",
        "--manifest-path", "Cargo.toml",           "--target",     "x86_64-unknown-none",
        "-Z",              "build-std=core,alloc", "--target-dir", ds_target,
    });
    cargo_ds.step.dependOn(&check_forth.step);

    // ── 5. Link everything into devspace.elf ─────────────────────────────────
    // Objects: crytic.o + drive_odin.o + Rust static lib
    // Linker script sets load address 0x500000, keeps full ELF for kernel loader.
    const link_elf = b.addSystemCommand(&.{
        "sh",                                                                                                                                                                                                                                                                                                                                                          "-c",
        "rust-lld -flavor gnu" ++ " -T src/devspace_linker.ld" ++ " -o " ++ build_dir ++ "/devspace.elf" ++ " " ++ build_dir ++ "/crytic.o" ++ " " ++ build_dir ++ "/drive_odin.o" ++ " --whole-archive" ++ " " ++ ds_target ++ "/x86_64-unknown-none/release/libdevspace.a" ++ " --no-whole-archive" ++ " -z max-page-size=0x1000" ++ " --gc-sections" ++ " -static",
    });
    link_elf.step.dependOn(&nasm_crytic.step);
    link_elf.step.dependOn(&odin_drive.step);
    link_elf.step.dependOn(&cargo_ds.step);

    // ── 6. Copy ELF to ISO ────────────────────────────────────────────────────
    const copy = b.addSystemCommand(&.{
        "sh",                                                                                                                                                        "-c",
        "mkdir -p " ++ iso_boot_dir ++ " && cp " ++ build_dir ++ "/devspace.elf " ++ iso_boot_dir ++ "/devspace.elf" ++ " && echo '[DS] devspace.elf → iso/boot'",
    });
    copy.step.dependOn(&link_elf.step);

    b.default_step.dependOn(&copy.step);

    // ── Clean ─────────────────────────────────────────────────────────────────
    const clean = b.step("clean", "Remove devspace build artifacts");
    const clean_cmd = b.addSystemCommand(&.{
        "rm",                         "-rf",
        build_dir ++ "/devspace.elf", build_dir ++ "/crytic.o",
        build_dir ++ "/drive_odin.o", build_dir ++ "/devspace_target",
    });
    clean.dependOn(&clean_cmd.step);

    // ── Check ─────────────────────────────────────────────────────────────────
    const check = b.step("check", "cargo check devspace");
    const cargo_check = b.addSystemCommand(&.{
        "cargo",               "+nightly",   "check",
        "--manifest-path",     "Cargo.toml", "--target",
        "x86_64-unknown-none", "-Z",         "build-std=core,alloc",
        "--target-dir",        ds_target,
    });
    check.dependOn(&cargo_check.step);
}
