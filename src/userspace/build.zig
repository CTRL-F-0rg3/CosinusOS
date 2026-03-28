const std = @import("std");

pub fn build(b: *std.Build) void {
    const build_dir = "../../build";
    const iso_boot_dir = "../../iso/boot";
    const us_target = build_dir ++ "/userspace_target";

    const cargo_init = b.addSystemCommand(&.{
        "cargo",           "+nightly",             "build",        "--release",
        "--manifest-path", "Cargo.toml",           "--target",     "x86_64-unknown-none",
        "-Z",              "build-std=core,alloc", "--target-dir", us_target,
    });

    const fs_target = b.resolveTargetQuery(.{
        .cpu_arch = .x86_64,
        .os_tag = .freestanding,
        .abi = .none,
    });

    const fs_mod = b.createModule(.{
        .root_source_file = b.path("src/filesystem/main.zig"),
        .target = fs_target,
        .optimize = .ReleaseSmall,
    });

    const fs_server = b.addExecutable(.{
        .name = "fs_server",
        .root_module = fs_mod,
    });

    // Ustawienia dla Executable (nie dla Module)
    fs_server.root_module.stack_protector = false;
    fs_server.root_module.red_zone = false;
    fs_server.link_z_max_page_size = 4096;

    // Ignorujemy brak standardowego wejścia, wskazujemy na main
    fs_server.entry = .{ .symbol_name = "main" };

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

    const strip_fs = b.addSystemCommand(&.{
        "objcopy",
        "-O",
        "binary",
        "--only-section=.text",
        "--only-section=.rodata",
        "--only-section=.data",
        "--only-section=.bss",
    });
    strip_fs.addFileArg(fs_server.getEmittedBin());
    strip_fs.addArg(build_dir ++ "/fs_server.bin");
    strip_fs.step.dependOn(&fs_server.step);

    const mkdir_cmd = b.addSystemCommand(&.{ "mkdir", "-p", iso_boot_dir });

    const cp_init = b.addSystemCommand(&.{ "cp", build_dir ++ "/userspace.bin", iso_boot_dir ++ "/userspace.bin" });
    cp_init.step.dependOn(&strip_init.step);
    cp_init.step.dependOn(&mkdir_cmd.step);

    const cp_fs = b.addSystemCommand(&.{ "cp", build_dir ++ "/fs_server.bin", iso_boot_dir ++ "/fs_server.bin" });
    cp_fs.step.dependOn(&strip_fs.step);
    cp_fs.step.dependOn(&mkdir_cmd.step);

    const export_step = b.step("export", "Export binaries to ISO");
    export_step.dependOn(&cp_init.step);
    export_step.dependOn(&cp_fs.step);

    b.default_step.dependOn(export_step);
}
