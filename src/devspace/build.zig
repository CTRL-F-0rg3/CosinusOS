const std = @import("std");

pub fn build(b: *std.Build) void {
    const build_dir = "../../build";
    const iso_boot_dir = "../../iso/boot";
    const ds_target = build_dir ++ "/devspace_target";

    // ── 1. Assemble crytic.asm (REP INSW/OUTSW critical transfers) ────────────
    const nasm_crytic = b.addSystemCommand(&.{
        "nasm",                         "-f", "elf64",
        "src/drivers/drive/crytic.asm", "-o", build_dir ++ "/crytic.o",
    });

    // ── 2. Compile Rust devspace (no_std, x86_64-unknown-none) ───────────────
    // Uses a custom target JSON that sets code-model=kernel and disables
    // red-zone (kernel does not preserve it for Ring-1).
    const cargo_ds = b.addSystemCommand(&.{
        "cargo",           "+nightly",             "build",        "--release",
        "--manifest-path", "Cargo.toml",           "--target",     "x86_64-unknown-none",
        "-Z",              "build-std=core,alloc", "--target-dir", ds_target,
    });

    // ── 3. Link final ELF: Rust object + crytic.o ────────────────────────────
    // rust-lld links both objects with our linker script → devspace.elf
    const link_elf = b.addSystemCommand(&.{
        "sh", "-c",
        // Find the Rust-compiled object file and link with crytic.o
        "rust-lld -flavor gnu" ++ " -T linker.ld" ++ " -o " ++ build_dir ++ "/devspace.elf" ++ " " ++ build_dir ++ "/crytic.o"
            // Rust puts the static lib here; extract all objects from it
        ++ " --whole-archive" ++ " " ++ ds_target ++ "/x86_64-unknown-none/release/devspace" ++ " --no-whole-archive" ++ " -z max-page-size=0x1000" ++ " --gc-sections",
    });
    link_elf.step.dependOn(&nasm_crytic.step);
    link_elf.step.dependOn(&cargo_ds.step);

    // ── 4. Copy ELF to ISO boot directory ────────────────────────────────────
    // DevSpace is loaded by GRUB as a multiboot2 module alongside kernel.elf.
    // We keep it as a full ELF so the kernel can parse PT_LOAD segments.
    const copy = b.addSystemCommand(&.{
        "sh",                                                                                                         "-c",
        "mkdir -p " ++ iso_boot_dir ++ " && cp " ++ build_dir ++ "/devspace.elf " ++ iso_boot_dir ++ "/devspace.elf",
    });
    copy.step.dependOn(&link_elf.step);

    b.default_step.dependOn(&copy.step);

    // ── Clean ─────────────────────────────────────────────────────────────────
    const clean = b.step("clean", "Remove devspace build artifacts");
    const clean_cmd = b.addSystemCommand(&.{
        "rm",                            "-rf",
        build_dir ++ "/devspace.elf",    build_dir ++ "/crytic.o",
        build_dir ++ "/devspace_target",
    });
    clean.dependOn(&clean_cmd.step);

    // ── Check — fast cargo check without linking ───────────────────────────────
    const check = b.step("check", "Run cargo check on devspace");
    const cargo_check = b.addSystemCommand(&.{
        "cargo",               "+nightly",   "check",
        "--manifest-path",     "Cargo.toml", "--target",
        "x86_64-unknown-none", "-Z",         "build-std=core,alloc",
        "--target-dir",        ds_target,
    });
    check.dependOn(&cargo_check.step);
}
