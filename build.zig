const std = @import("std");

pub fn build(b: *std.Build) void {
    const debug = b.option(bool, "debug", "Włącz debug logging w QEMU") orelse false;
    const qemu_wait_gdb = b.option(bool, "gdb", "Czekaj na GDB (port 1234)") orelse false;
    const skip_qemu = b.option(bool, "no-run", "Nie uruchamiaj QEMU") orelse false;

    const build_dir = "build";
    const iso_root = "iso";

    // ── Kernel (Rust) ──────────────────────────────────────────────────────
    const kernel_step = b.step("kernel", "Build kernel only");

    const check_cargo = b.addSystemCommand(&.{
        "sh", "-c",
        "grep -q '\\[lib\\]' src/kernel/Cargo.toml && " ++
            "grep -q 'crate-type.*staticlib' src/kernel/Cargo.toml || " ++
            "(echo '[ERROR] kernel/Cargo.toml: brak [lib] staticlib' && exit 1)",
    });

    const cargo_kernel = b.addSystemCommand(&.{
        "cargo",           "+nightly",                    "build",    "--release",
        "--manifest-path", "src/kernel/Cargo.toml",       "--target", "x86_64-unknown-none",
        "-Z",              "build-std=core,alloc",        "-Z",       "build-std-features=compiler-builtins-mem",
        "--target-dir",    build_dir ++ "/kernel_target",
    });
    cargo_kernel.step.dependOn(&check_cargo.step);
    kernel_step.dependOn(&cargo_kernel.step);

    // ── Bootloader (NASM) ─────────────────────────────────────────────────
    const boot_step = b.step("boot", "Build bootloader only");

    const nasm_boot = b.addSystemCommand(&.{
        "nasm", "-f", "elf64",
        "-w+all", // włącz wszystkie ostrzeżenia
        "-Wno-deprecated", // wycisz deprecated ABS
        "boot.asm",
        "-o",
        build_dir ++ "/boot.o",
    });
    boot_step.dependOn(&nasm_boot.step);

    // ── Linkowanie kernel.elf ─────────────────────────────────────────────
    const link_step = b.step("link", "Link kernel.elf");

    const linker_cmd = b.addSystemCommand(&.{
        "ld",
        "-T",
        "linker.ld",
        "-o",
        build_dir ++ "/kernel.elf",
        "--nmagic",
        build_dir ++ "/boot.o",
        build_dir ++ "/kernel_target/x86_64-unknown-none/release/libkernel.a",
    });
    linker_cmd.step.dependOn(kernel_step);
    linker_cmd.step.dependOn(boot_step);
    link_step.dependOn(&linker_cmd.step);

    // ── Userspace (Rust) ──────────────────────────────────────────────────
    const userspace_step = b.step("userspace", "Build userspace only");

    const cargo_us = b.addSystemCommand(&.{
        "cargo",           "+nightly",                 "build",        "--release",
        "--manifest-path", "src/userspace/Cargo.toml", "--target",     "x86_64-unknown-none",
        "-Z",              "build-std=core,alloc",     "--target-dir", build_dir ++ "/userspace_target",
    });

    const copy_us_bin = b.addSystemCommand(&.{
        "cp",
        build_dir ++ "/userspace_target/x86_64-unknown-none/release/userspace",
        build_dir ++ "/userspace.bin",
    });
    copy_us_bin.step.dependOn(&cargo_us.step);
    userspace_step.dependOn(&copy_us_bin.step);

    // ── Przygotowanie ISO ─────────────────────────────────────────────────
    const prepare_iso = b.addSystemCommand(&.{
        "sh", "-c",
        "mkdir -p iso/boot/grub && " ++
            "cp " ++ build_dir ++ "/kernel.elf    iso/boot/ && " ++
            "cp " ++ build_dir ++ "/userspace.bin iso/boot/ && " ++
            "printf 'set timeout=3\\nset default=0\\nmenuentry CosinusOS {\\n" ++
            "    multiboot2 /boot/kernel.elf\\n" ++
            "    module2   /boot/userspace.bin\\n}\\n' > iso/boot/grub/grub.cfg",
    });
    prepare_iso.step.dependOn(link_step);
    prepare_iso.step.dependOn(userspace_step);

    // ── ISO ───────────────────────────────────────────────────────────────
    const iso_step = b.step("iso", "Build ISO");

    const grub_mkrescue = b.addSystemCommand(&.{
        "sh", "-c",
        "rm -f " ++ build_dir ++ "/cosinusos.iso && " ++
            "grub-mkrescue -o " ++ build_dir ++ "/cosinusos.iso " ++ iso_root,
    });
    grub_mkrescue.step.dependOn(&prepare_iso.step);
    iso_step.dependOn(&grub_mkrescue.step);

    // ── Diagnostyka ───────────────────────────────────────────────────────
    const diag_step = b.step("diag", "Diagnostyka builda");

    const diag_cmd = b.addSystemCommand(&.{
        "sh", "-c",
        "echo '=== BUILD ===' && " ++
            "ls -lh " ++ build_dir ++ "/kernel.elf " ++
            build_dir ++ "/userspace.bin 2>/dev/null || echo 'BRAK PLIKU' && " ++
            "echo '--- grub.cfg ---' && " ++
            "cat iso/boot/grub/grub.cfg 2>/dev/null || echo 'brak grub.cfg'",
    });
    diag_cmd.step.dependOn(iso_step);
    diag_step.dependOn(&diag_cmd.step);

    // ── QEMU ──────────────────────────────────────────────────────────────
    const run_step = b.step("run", "Uruchom w QEMU");

    // Bazowe argumenty QEMU (zawsze)
    const qemu_base = &[_][]const u8{
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
    };
    // Argumenty debug
    const qemu_debug_args = &[_][]const u8{
        "-d",           "int,guest_errors,cpu_reset",
        "-D",           build_dir ++ "/qemu-debug.log",
        "-no-shutdown",
    };
    // Argumenty GDB
    const qemu_gdb_args = &[_][]const u8{ "-s", "-S" };

    // Złóż listę argumentów przez alokator
    var args_list = std.ArrayListUnmanaged([]const u8){};
    args_list.appendSlice(b.allocator, qemu_base) catch unreachable;
    if (debug) args_list.appendSlice(b.allocator, qemu_debug_args) catch unreachable;
    if (qemu_wait_gdb) args_list.appendSlice(b.allocator, qemu_gdb_args) catch unreachable;

    const qemu_run = b.addSystemCommand(args_list.items);
    qemu_run.step.dependOn(diag_step);
    run_step.dependOn(&qemu_run.step);

    // ── Default + Clean ───────────────────────────────────────────────────
    if (skip_qemu) {
        b.default_step.dependOn(diag_step);
    } else {
        b.default_step.dependOn(run_step);
    }

    const clean_step = b.step("clean", "Usuń pliki builda");
    const clean_cmd = b.addSystemCommand(&.{ "rm", "-rf", build_dir, iso_root });
    clean_step.dependOn(&clean_cmd.step);
}
