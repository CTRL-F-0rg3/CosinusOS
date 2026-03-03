const std = @import("std");

pub fn build(b: *std.Build) void {
    const build_dir = "../../build";

    const cargo_us = b.addSystemCommand(&.{
        "cargo",           "+nightly",             "build",        "--release",
        "--manifest-path", "Cargo.toml",           "--target",     "x86_64-unknown-none",
        "-Z",              "build-std=core,alloc", "--target-dir", build_dir ++ "/userspace_target",
    });

    const copy_bin = b.addSystemCommand(&.{
        "cp",
        build_dir ++ "/userspace_target/x86_64-unknown-none/release/userspace",
        build_dir ++ "/userspace.bin",
    });
    copy_bin.step.dependOn(&cargo_us.step);

    const copy_to_iso = b.addSystemCommand(&.{
        "sh", "-c",
        "mkdir -p ../../iso/boot && cp " ++
            build_dir ++ "/userspace.bin ../../iso/boot/userspace.bin",
    });
    copy_to_iso.step.dependOn(&copy_bin.step);

    b.default_step.dependOn(&copy_to_iso.step);

    const clean = b.step("clean", "Clean userspace");
    const clean_cmd = b.addSystemCommand(&.{
        "rm",                          "-rf",
        build_dir ++ "/userspace.bin", build_dir ++ "/userspace_target",
    });
    clean.dependOn(&clean_cmd.step);
}
