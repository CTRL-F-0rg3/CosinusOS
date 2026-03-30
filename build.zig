const std = @import("std");

pub fn build(b: *std.Build) void {
    const debug = b.option(bool, "debug", "QEMU: log interrupts") orelse false;
    const qemu_wait_gdb = b.option(bool, "gdb", "QEMU: wait for GDB") orelse false;
    const skip_qemu = b.option(bool, "no-run", "Skip QEMU launch") orelse false;

    const build_dir = "build";
    const iso_root = "iso";

    // ── 1. Kernel ─────────────────────────────────────────────────────────────
    const kernel_step = b.step("kernel", "Build kernel");
    const build_kernel = b.addSystemCommand(&.{
        "sh", "-c", "mkdir -p build && cd src/kernel && zig build",
    });
    kernel_step.dependOn(&build_kernel.step);

    // ── 2. Userspace (init process + FS server) ───────────────────────────────
    const userspace_step = b.step("userspace", "Build userspace");
    const build_userspace = b.addSystemCommand(&.{
        "sh", "-c", "mkdir -p build && cd src/userspace && zig build",
    });
    userspace_step.dependOn(&build_userspace.step);

    // ── 3. DevSpace (Ring-1 driver layer) ─────────────────────────────────────
    const devspace_step = b.step("devspace", "Build DevSpace Ring-1 drivers");
    const build_devspace = b.addSystemCommand(&.{
        "sh", "-c", "mkdir -p build && cd src/devspace && zig build",
    });
    devspace_step.dependOn(&build_devspace.step);

    // ── 4. GRUB config ────────────────────────────────────────────────────────
    // Lists all boot modules: kernel + userspace init + devspace driver layer.
    // Kernel reads module2 tags in order:
    //   [0] userspace.bin  — flat binary, init process (Ring-3)
    //   [1] devspace.elf   — ELF, Ring-1 driver layer
    const grub_cfg = b.addSystemCommand(&.{
        "sh",                                                                                                                                                                                                                                                                                                               "-c",
        "mkdir -p " ++ iso_root ++ "/boot/grub && cat > " ++ iso_root ++ "/boot/grub/grub.cfg << 'EOF'\n" ++ "set timeout=3\n" ++ "set default=0\n" ++ "menuentry CosinusOS {\n" ++ "    multiboot2 /boot/kernel.elf\n" ++ "    module2   /boot/userspace.bin\n" ++ "    module2   /boot/devspace.elf\n" ++ "}\n" ++ "EOF",
    });
    grub_cfg.step.dependOn(kernel_step);
    grub_cfg.step.dependOn(userspace_step);
    grub_cfg.step.dependOn(devspace_step);

    // ── 5. ISO ────────────────────────────────────────────────────────────────
    const iso_step = b.step("iso", "Build ISO image");
    const grub_mkrescue = b.addSystemCommand(&.{
        "sh",                                                                                                               "-c",
        "rm -f " ++ build_dir ++ "/cosinusos.iso && " ++ "grub-mkrescue -o " ++ build_dir ++ "/cosinusos.iso " ++ iso_root,
    });
    grub_mkrescue.step.dependOn(&grub_cfg.step);
    iso_step.dependOn(&grub_mkrescue.step);

    // ── 6. Diagnostics ────────────────────────────────────────────────────────
    const diag_step = b.step("diag", "Show build summary");
    const diag_cmd = b.addSystemCommand(&.{
        "sh",                                                                                                                                                                                                                                                       "-c",
        "echo '=== BUILD ===' && " ++ "ls -lh " ++ build_dir ++ "/kernel.elf " ++ build_dir ++ "/userspace.bin " ++ build_dir ++ "/devspace.elf " ++ "2>/dev/null && " ++ "echo '--- grub.cfg ---' && " ++ "cat " ++ iso_root ++ "/boot/grub/grub.cfg 2>/dev/null",
    });
    diag_cmd.step.dependOn(iso_step);
    diag_step.dependOn(&diag_cmd.step);

    // ── 7. QEMU run ───────────────────────────────────────────────────────────
    const run_step = b.step("run", "Launch QEMU");
    var args = std.ArrayListUnmanaged([]const u8){};
    args.appendSlice(b.allocator, &.{
        "qemu-system-x86_64",
        "-cdrom",
        build_dir ++ "/cosinusos.iso",
        "-m",
        "512M",
        "-serial",
        "stdio",
        "-vga",
        "std",
        "-display",
        "sdl",
        "-cpu",
        "qemu64",
        "-smp",
        "2",
        "-no-reboot",
        // ATA disk image — devspace will read/write this
        "-drive",
        "file=" ++ build_dir ++ "/disk.img,format=raw,if=ide,index=0",
    }) catch unreachable;

    if (debug) args.appendSlice(b.allocator, &.{
        "-d",           "int,guest_errors,cpu_reset",
        "-D",           build_dir ++ "/qemu-debug.log",
        "-no-shutdown",
    }) catch unreachable;

    if (qemu_wait_gdb) args.appendSlice(b.allocator, &.{
        "-s", "-S",
    }) catch unreachable;

    const qemu_run = b.addSystemCommand(args.items);
    qemu_run.step.dependOn(diag_step);
    run_step.dependOn(&qemu_run.step);

    // ── 8. Default target ─────────────────────────────────────────────────────
    if (skip_qemu) {
        b.default_step.dependOn(diag_step);
    } else {
        b.default_step.dependOn(run_step);
    }

    // ── 9. Clean ──────────────────────────────────────────────────────────────
    const clean_step = b.step("clean", "Clean everything");
    const clean_cmd = b.addSystemCommand(&.{
        "sh",                                                                                                                                                                                      "-c",
        "cd src/kernel    && zig build clean ; " ++ "cd ../userspace  && zig build clean ; " ++ "cd ../devspace   && zig build clean ; " ++ "cd ../.. && rm -rf " ++ build_dir ++ " " ++ iso_root,
    });
    clean_step.dependOn(&clean_cmd.step);

    // ── 10. Create blank disk image if missing ────────────────────────────────
    // Run once manually: zig build disk
    // Creates a 20MB raw disk image for ATA driver testing.
    const disk_step = b.step("disk", "Create blank 20MB disk image for ATA");
    const disk_cmd = b.addSystemCommand(&.{
        "sh",                                                                                                                                                       "-c",
        "mkdir -p " ++ build_dir ++ " && dd if=/dev/zero of=" ++ build_dir ++ "/disk.img bs=1M count=20 2>/dev/null" ++ " && echo 'Created build/disk.img (20MB)'",
    });
    disk_step.dependOn(&disk_cmd.step);
}
