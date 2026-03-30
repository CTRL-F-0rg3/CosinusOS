const std = @import("std");

pub fn build(b: *std.Build) void {
    const build_dir = "../../build";
    const iso_boot_dir = "../../iso/boot";
    const ds_target = build_dir ++ "/devspace_target";

    // ── 1. Assemble crytic.asm → crytic.o ────────────────────────────────────
    // Must run BEFORE cargo so build.rs can find the object.
    const nasm_crytic = b.addSystemCommand(&.{
        "nasm", "-f",                     "elf64",
        "-o",   build_dir ++ "/crytic.o", "src/drivers/drive/crytic.asm",
    });

    // ── 2. Compile Odin drive.odin → drive_odin.o ────────────────────────────
    // Odin -file flag compiles a single file as its own package.
    // -build-mode:obj = emit .o, do not link.
    // -no-crt = no libc startup code (freestanding).
    // We need the output named drive_odin.o so build.rs finds it.
    const odin_drive = b.addSystemCommand(&.{
        "sh", "-c",
        "odin build src/drivers/drive/drive.odin" ++ " -file" ++ " -target:linux_amd64" ++ " -build-mode:obj" ++ " -out:" ++ build_dir ++ "/drive_odin" // odin appends .o
        ++ " -o:speed" ++ " -no-crt" ++ " -disable-assert" ++ " -no-bounds-check"
            // Remove stdlib imports that don't exist freestanding
        ++ " || true", // don't fail build if odin not installed yet
    });

    // ── 3. Cargo build — needs crytic.o + drive_odin.o ready first ───────────
    // Passes build dir via env so build.rs can locate the .o files.
    const cargo_ds = b.addSystemCommand(&.{
        "sh",                                                                                                                                                                                                    "-c",
        "CARGO_BUILD_DIR=" ++ build_dir ++ " cargo +nightly build --release" ++ " --manifest-path Cargo.toml" ++ " --target x86_64-unknown-none" ++ " -Z build-std=core,alloc" ++ " --target-dir " ++ ds_target,
    });
    cargo_ds.step.dependOn(&nasm_crytic.step);
    cargo_ds.step.dependOn(&odin_drive.step);

    // ── 4. Copy ELF to ISO (cargo + build.rs handles linking via linker.ld) ──
    // The final ELF is produced by cargo's link step (with our linker script
    // and the external .o files passed via build.rs rustc-link-arg).
    const copy = b.addSystemCommand(&.{
        "sh",                                                                                                                                                                                      "-c",
        "mkdir -p " ++ iso_boot_dir ++ " && cp " ++ ds_target ++ "/x86_64-unknown-none/release/devspace " ++ iso_boot_dir ++ "/devspace.elf" ++ " && echo '[DS] devspace.elf copied to iso/boot'",
    });
    copy.step.dependOn(&cargo_ds.step);

    b.default_step.dependOn(&copy.step);

    // ── Clean ─────────────────────────────────────────────────────────────────
    const clean = b.step("clean", "Remove devspace build artifacts");
    const clean_cmd = b.addSystemCommand(&.{
        "rm",                            "-rf",
        build_dir ++ "/crytic.o",        build_dir ++ "/drive_odin.o",
        build_dir ++ "/devspace_target", iso_boot_dir ++ "/devspace.elf",
    });
    clean.dependOn(&clean_cmd.step);

    // ── Check ─────────────────────────────────────────────────────────────────
    const check = b.step("check", "cargo check devspace");
    const cargo_check = b.addSystemCommand(&.{
        "sh",                                                                                                                                                                                          "-c",
        "CARGO_BUILD_DIR=" ++ build_dir ++ " cargo +nightly check" ++ " --manifest-path Cargo.toml" ++ " --target x86_64-unknown-none" ++ " -Z build-std=core,alloc" ++ " --target-dir " ++ ds_target,
    });
    check.dependOn(&cargo_check.step);
}
