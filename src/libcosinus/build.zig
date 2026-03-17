const std = @import("std");
pub fn build(b: *std.Build) void {
    const build_dir = "../../build";
    // Buduj libcosinus jako staticlib
    const cargo = b.addSystemCommand(&.{
        "cargo", "+nightly", "build",
        "--release",
        "--manifest-path", "Cargo.toml",
        "--target",        "x86_64-cosinus.json",
        "-Z", "json-target-spec",
        "-Z", "build-std=core,compiler_builtins",
        "-Z", "build-std-features=compiler-builtins-mem",
        "--target-dir", build_dir ++ "/libcosinus_target",
    });
    // Skopiuj libcosinus.a do build/
    const copy = b.addSystemCommand(&.{
        "sh", "-c",
        "cp " ++ build_dir ++ "/libcosinus_target/x86_64-cosinus/release/libcosinus.a " ++
        build_dir ++ "/libcosinus.a",
    });
    copy.step.dependOn(&cargo.step);
    b.default_step.dependOn(&copy.step);
}
