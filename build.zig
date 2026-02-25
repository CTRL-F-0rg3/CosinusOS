const std = @import("std");

pub fn build(b: *std.Build) void {
    // ============================================================
    // OPCJE BUILD
    // ============================================================
    const debug = b.option(bool, "debug", "Włącz debug logging w QEMU") orelse false;
    const qemu_wait_gdb = b.option(bool, "gdb", "Czekaj na GDB (port 1234)") orelse false;
    const skip_qemu = b.option(bool, "no-run", "Nie uruchamiaj QEMU po buildzie") orelse false;

    // ============================================================
    // ŚCIEŻKI
    // ============================================================
    const build_dir = "build";
    const iso_root = "iso";

    // ============================================================
    // KERNEL (Rust)
    // ============================================================
    const kernel_step = b.step("kernel", "Build kernel only");

    const cargo_kernel = b.addSystemCommand(&.{
        "cargo",           "+nightly",                    "build",    "--release",
        "--manifest-path", "src/kernel/Cargo.toml",       "--target", "x86_64-unknown-none",
        "-Z",              "build-std=core,alloc",        "-Z",       "build-std-features=compiler-builtins-mem",
        "--target-dir",    build_dir ++ "/kernel_target",
    });

    const check_cargo = b.addSystemCommand(&.{
        "sh", "-c",
        "grep -q '\\[lib\\]' src/kernel/Cargo.toml && " ++
            "grep -q 'crate-type.*staticlib' src/kernel/Cargo.toml || " ++
            "(echo '[ERROR] kernel/Cargo.toml musi mieć [lib] z crate-type = [\"staticlib\"]' && exit 1)",
    });

    kernel_step.dependOn(&check_cargo.step);
    kernel_step.dependOn(&cargo_kernel.step);

    // ============================================================
    // BOOTLOADER (NASM)
    // ============================================================
    const boot_step = b.step("boot", "Build bootloader only");

    const nasm_boot = b.addSystemCommand(&.{
        "nasm",     "-f", "elf64",                "-g", "-F", "dwarf",
        "boot.asm", "-o", build_dir ++ "/boot.o",
    });
    boot_step.dependOn(&nasm_boot.step);

    // ============================================================
    // LINK KERNEL
    // ============================================================
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

    // ============================================================
    // USERSPACE (Rust)
    // ============================================================
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

    userspace_step.dependOn(&cargo_us.step);
    userspace_step.dependOn(&copy_us_bin.step);

    // ============================================================
    // PRZYGOTOWANIE ISO + grub.cfg
    // ============================================================
    const prepare_iso = b.addSystemCommand(&.{ "sh", "-c", "mkdir -p iso/boot/grub && " ++
        "cp " ++ build_dir ++ "/kernel.elf   iso/boot/ && " ++
        "cp " ++ build_dir ++ "/userspace.bin iso/boot/ && " ++
        "cat > iso/boot/grub/grub.cfg << 'EOF'\n" ++
        "set timeout=3\n" ++
        "set default=0\n" ++
        "menuentry 'CosinusOS' {\n" ++
        "    multiboot2 /boot/kernel.elf debug=1 earlycon=serial\n" ++
        "    module2   /boot/userspace.bin\n" ++
        "}\n" ++
        "EOF" });

    prepare_iso.step.dependOn(link_step);
    prepare_iso.step.dependOn(userspace_step);

    // ============================================================
    // TWORZENIE ISO
    // ============================================================
    const iso_step = b.step("iso", "Build ISO");

    const grub_mkrescue = b.addSystemCommand(&.{
        "grub-mkrescue",
        "-o",
        build_dir ++ "/cosinusos.iso",
        iso_root,
    });

    grub_mkrescue.step.dependOn(&prepare_iso.step);
    iso_step.dependOn(&grub_mkrescue.step);

    // ============================================================
    // DIAGNOSTYKA
    // ============================================================
    const diag_step = b.step("diag", "Diagnostyka builda");

    const diag_cmd = b.addSystemCommand(&.{ "sh", "-c", "echo '=== DIAGNOSTYKA ===' && " ++
        "ls -lh " ++ build_dir ++ "/kernel.elf " ++ build_dir ++ "/userspace.bin 2>/dev/null || echo 'brak któregoś pliku!' && " ++
        "cat iso/boot/grub/grub.cfg 2>/dev/null || echo 'brak grub.cfg' && " ++
        "echo '--- pierwsze 256 bajtów ISO ---' && " ++
        "hexdump -C " ++ build_dir ++ "/cosinusos.iso | head -n 16" });

    diag_cmd.step.dependOn(iso_step);
    diag_step.dependOn(&diag_cmd.step);

    // ============================================================
    // QEMU – wersja na Zig 0.15+ (Managed ArrayList)
    // ============================================================
    const run_step = b.step("run", "Uruchom w QEMU");

    var qemu_args = std.array_list.Managed([]const u8).init(b.allocator);
    defer qemu_args.deinit();

    qemu_args.appendSlice(&.{
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
    }) catch unreachable;

    if (debug) {
        qemu_args.appendSlice(&.{
            "-d",         "int,guest_errors,unimp",
            "-D",         build_dir ++ "/qemu-debug.log",
            "-no-reboot", "-no-shutdown",
        }) catch unreachable;
    }

    if (qemu_wait_gdb) {
        qemu_args.appendSlice(&.{ "-s", "-S" }) catch unreachable;
    }

    const qemu_run = b.addSystemCommand(qemu_args.items);
    run_step.dependOn(diag_step);
    run_step.dependOn(&qemu_run.step);

    // ============================================================
    // DEFAULT + CLEAN
    // ============================================================
    if (skip_qemu) {
        b.default_step.dependOn(diag_step);
    } else {
        b.default_step.dependOn(run_step);
    }

    const clean_step = b.step("clean", "Usuń pliki builda");
    const clean_cmd = b.addSystemCommand(&.{ "rm", "-rf", build_dir, iso_root });
    clean_step.dependOn(&clean_cmd.step);
}
