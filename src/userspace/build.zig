const std = @import("std");

pub fn build(b: *std.Build) void {
    const build_dir = "../../build";
    const iso_boot_dir = "../../iso/boot";
    const us_target = build_dir ++ "/userspace_target";

    // ── 1. Compile Rust init process (no_std, x86_64-unknown-none) ───────────
    const cargo_init = b.addSystemCommand(&.{
        "cargo",           "+nightly",             "build",        "--release",
        "--manifest-path", "Cargo.toml",           "--target",     "x86_64-unknown-none",
        "-Z",              "build-std=core,alloc", "--target-dir", us_target,
    });

    // ── 2. Compile Zig FS server (freestanding, no libc) ─────────────────────
    const fs_target = b.resolveTargetQuery(.{
        .cpu_arch = .x86_64,
        .os_tag = .freestanding,
        .abi = .none,
    });

    const fs_mod = b.createModule(.{
        .root_source_file = b.path("../filesystem/main.zig"),
        .target = fs_target,
        .optimize = .ReleaseSmall,
    });

    const fs_server = b.addExecutable(.{
        .name = "fs_server",
        .root_module = fs_mod,
    });

    // Disable stack protector and red zone — kernel does not set these up
    fs_server.root_module.stack_protector = false;
    fs_server.root_module.red_zone = false;

    const fs_server_install = b.addInstallArtifact(fs_server, .{
        .dest_dir = .{ .override = .{ .custom = build_dir ++ "/fs_server_target" } },
    });

    // ── 3. Strip init binary: raw sections only, no ELF headers ──────────────
    // Strips everything except .text/.rodata/.data/.bss → flat binary at 0x400000
    const strip_init = b.addSystemCommand(&.{
        "objcopy",
        "-O",
        "binary",
        "--only-section=.text",
        "--only-section=.rodata",
        "--only-section=.data",
        "--only-section=.bss",
        us_target ++ "/x86_64-unknown-none/release/userspace",
        build_dir ++ "/userspace.bin",
    });
    strip_init.step.dependOn(&cargo_init.step);

    // ── 4. Strip FS server binary the same way ────────────────────────────────
    const strip_fs = b.addSystemCommand(&.{
        "objcopy",
        "-O",
        "binary",
        "--only-section=.text",
        "--only-section=.rodata",
        "--only-section=.data",
        "--only-section=.bss",
        build_dir ++ "/fs_server_target/fs_server",
        build_dir ++ "/fs_server.bin",
    });
    strip_fs.step.dependOn(&fs_server_install.step);

    // ── 5. Copy both binaries to ISO boot directory ───────────────────────────
    const copy = b.addSystemCommand(&.{
        "sh",                                                                                                                                                                                              "-c",
        "mkdir -p " ++ iso_boot_dir ++ " && cp " ++ build_dir ++ "/userspace.bin " ++ iso_boot_dir ++ "/userspace.bin" ++ " && cp " ++ build_dir ++ "/fs_server.bin " ++ iso_boot_dir ++ "/fs_server.bin",
    });
    copy.step.dependOn(&strip_init.step);
    copy.step.dependOn(&strip_fs.step);

    b.default_step.dependOn(&copy.step);

    // ── Clean step ────────────────────────────────────────────────────────────
    const clean = b.step("clean", "Remove all userspace build artifacts");
    const clean_cmd = b.addSystemCommand(&.{
        "rm",                             "-rf",
        build_dir ++ "/userspace.bin",    build_dir ++ "/fs_server.bin",
        build_dir ++ "/userspace_target", build_dir ++ "/fs_server_target",
    });
    clean.dependOn(&clean_cmd.step);

    // ── Check step — cargo check only, fast iteration ────────────────────────
    const check = b.step("check", "Run cargo check on Rust userspace");
    const cargo_check = b.addSystemCommand(&.{
        "cargo",               "+nightly",   "check",
        "--manifest-path",     "Cargo.toml", "--target",
        "x86_64-unknown-none", "-Z",         "build-std=core,alloc",
        "--target-dir",        us_target,
    });
    check.dependOn(&cargo_check.step);
}
