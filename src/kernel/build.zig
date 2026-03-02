const std = @import("std");

pub fn build(b: *std.Build) void {
    const build_dir = "../../build";

    const cargo_kernel = b.addSystemCommand(&.{
        "cargo",                       "+nightly",                                 "build",
        "--release",                   "--manifest-path",                          "Cargo.toml",
        "--target",                    "x86_64-cosinus.json",                      "-Z",
        "json-target-spec",            "-Z",                                       "build-std=core,compiler_builtins",
        "-Z",                          "build-std-features=compiler-builtins-mem", "--target-dir",
        build_dir ++ "/kernel_target",
    });

    const nasm_boot = b.addSystemCommand(&.{
        "nasm",           "-f", "elf64",                "-w+all", "-Wno-deprecated",
        "../../boot.asm", "-o", build_dir ++ "/boot.o",
    });

    const link_kernel = b.addSystemCommand(&.{
        "ld",                                                                  "-T",                       "linker.ld",
        "-nostdlib",                                                           "-static",                  "-no-pie",
        "--no-warn-rwx-segments",                                              "-z",                       "noexecstack",
        "-o",                                                                  build_dir ++ "/kernel.elf", build_dir ++ "/boot.o",
        build_dir ++ "/kernel_target/x86_64-unknown-none/release/libkernel.a",
    });
    link_kernel.step.dependOn(&cargo_kernel.step);
    link_kernel.step.dependOn(&nasm_boot.step);

    const copy_to_iso = b.addSystemCommand(&.{
        "sh", "-c",
        "mkdir -p ../../iso/boot && cp " ++
            build_dir ++ "/kernel.elf ../../iso/boot/kernel.elf",
    });
    copy_to_iso.step.dependOn(&link_kernel.step);

    b.default_step.dependOn(&copy_to_iso.step);

    const clean = b.step("clean", "Clean kernel");
    const clean_cmd = b.addSystemCommand(&.{
        "rm",                          "-rf",
        build_dir ++ "/boot.o",        build_dir ++ "/kernel.elf",
        build_dir ++ "/kernel_target",
    });
    clean.dependOn(&clean_cmd.step);
}
