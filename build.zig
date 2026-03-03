const std = @import("std");

pub fn build(b: *std.Build) void {
    const debug = b.option(bool, "debug", "QEMU: loguj przerwania") orelse false;
    const qemu_wait_gdb = b.option(bool, "gdb", "QEMU: czekaj na GDB") orelse false;
    const skip_qemu = b.option(bool, "no-run", "Nie uruchamiaj QEMU") orelse false;

    const build_dir = "build";
    const iso_root = "iso";

    const kernel_step = b.step("kernel", "Build kernel");
    const build_kernel = b.addSystemCommand(&.{
        "sh", "-c", "mkdir -p build && cd src/kernel && zig build",
    });
    kernel_step.dependOn(&build_kernel.step);

    const userspace_step = b.step("userspace", "Build userspace");
    const build_userspace = b.addSystemCommand(&.{
        "sh", "-c", "mkdir -p build && cd src/userspace && zig build",
    });
    userspace_step.dependOn(&build_userspace.step);

    const grub_cfg = b.addSystemCommand(&.{
        "sh", "-c",
        "mkdir -p " ++ iso_root ++ "/boot/grub && " ++
            "echo 'set timeout=3' > " ++ iso_root ++ "/boot/grub/grub.cfg && " ++
            "echo 'set default=0' >> " ++ iso_root ++ "/boot/grub/grub.cfg && " ++
            "echo 'menuentry CosinusOS {' >> " ++ iso_root ++ "/boot/grub/grub.cfg && " ++
            "echo '    multiboot2 /boot/kernel.elf' >> " ++ iso_root ++ "/boot/grub/grub.cfg && " ++
            "echo '    module2   /boot/userspace.bin' >> " ++ iso_root ++ "/boot/grub/grub.cfg && " ++
            "echo '}' >> " ++ iso_root ++ "/boot/grub/grub.cfg",
    });
    grub_cfg.step.dependOn(kernel_step);
    grub_cfg.step.dependOn(userspace_step);

    const iso_step = b.step("iso", "Build ISO");
    const grub_mkrescue = b.addSystemCommand(&.{
        "sh", "-c",
        "rm -f " ++ build_dir ++ "/cosinusos.iso && " ++
            "grub-mkrescue -o " ++ build_dir ++ "/cosinusos.iso " ++ iso_root,
    });
    grub_mkrescue.step.dependOn(&grub_cfg.step);
    iso_step.dependOn(&grub_mkrescue.step);

    const diag_step = b.step("diag", "Diagnostyka");
    const diag_cmd = b.addSystemCommand(&.{
        "sh", "-c",
        "echo === BUILD === && " ++
            "ls -lh " ++ build_dir ++ "/kernel.elf " ++
            build_dir ++ "/userspace.bin 2>/dev/null && " ++
            "echo --- grub.cfg --- && " ++
            "cat " ++ iso_root ++ "/boot/grub/grub.cfg 2>/dev/null",
    });
    diag_cmd.step.dependOn(iso_step);
    diag_step.dependOn(&diag_cmd.step);

    const run_step = b.step("run", "Uruchom QEMU");
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
        "-cpu",
        "qemu64",
        "-smp",
        "2",
        "-no-reboot",
    }) catch unreachable;
    if (debug) args.appendSlice(b.allocator, &.{
        "-d",           "int,guest_errors,cpu_reset",
        "-D",           build_dir ++ "/qemu-debug.log",
        "-no-shutdown",
    }) catch unreachable;
    if (qemu_wait_gdb) args.appendSlice(b.allocator, &.{ "-s", "-S" }) catch unreachable;

    const qemu_run = b.addSystemCommand(args.items);
    qemu_run.step.dependOn(diag_step);
    run_step.dependOn(&qemu_run.step);

    if (skip_qemu) {
        b.default_step.dependOn(diag_step);
    } else {
        b.default_step.dependOn(run_step);
    }

    const clean_step = b.step("clean", "Clean wszystko");
    const clean_cmd = b.addSystemCommand(&.{
        "sh", "-c",
        "cd src/kernel && zig build clean ; " ++
            "cd ../userspace && zig build clean ; " ++
            "cd ../.. && rm -rf " ++ build_dir ++ " " ++ iso_root,
    });
    clean_step.dependOn(&clean_cmd.step);
}
