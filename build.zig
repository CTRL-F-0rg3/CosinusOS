const std = @import("std");

// ============================================================
//  CosineOS — build.zig
//  Architektura : x86_64
//  Kernel       : src/kernel  (Rust, no_std, x86_64-unknown-none)
//  Userspace    : src/userspace (Rust, no_std)
//  Bootloader   : boot.asm + GRUB (multiboot2)
//  Wyjście      : build/cosinusos.iso  +  build/cosinusos.img
// ============================================================

pub fn build(b: *std.Build) void {
    // --------------------------------------------------------
    // Opcje
    // --------------------------------------------------------
    const debug = b.option(bool, "debug", "Włącz debug (QEMU bez -daemonize)") orelse true;
    const qemu_wait_gdb = b.option(bool, "gdb", "Czekaj na GDB na porcie 1234") orelse false;

    // --------------------------------------------------------
    // Ścieżki
    // --------------------------------------------------------
    const build_dir = "build";
    const iso_dir = "iso/boot";
    const grub_dir = "iso/boot/grub";
    const kernel_elf = build_dir ++ "/kernel.elf";
    const boot_obj = build_dir ++ "/boot.o";
    const userspace_bin = build_dir ++ "/userspace_raw.bin";
    const iso_out = build_dir ++ "/cosinusos.iso";
    const img_out = build_dir ++ "/cosinusos.img";

    // ========================================================
    // KROK 1 — Stwórz katalogi wyjściowe
    // ========================================================
    const mk_dirs = b.addSystemCommand(&.{
        "mkdir",   "-p",
        build_dir, iso_dir,
        grub_dir,
    });

    // ========================================================
    // KROK 2 — Kompiluj boot.asm → boot.o
    // ========================================================
    const asm_boot = b.addSystemCommand(&.{
        "nasm",
        "-f",
        "elf64",
        "boot.asm",
        "-o",
        boot_obj,
    });
    asm_boot.step.dependOn(&mk_dirs.step);

    // ========================================================
    // KROK 3 — Upewnij się że kernel Cargo.toml ma crate-type = staticlib,
    //           potem kompiluj kernel (Rust) → libkernel.a
    // ========================================================
    // Patch Cargo.toml kernela - dodaj [lib] staticlib jeśli nie ma
    const patch_cargo = b.addSystemCommand(&.{
        "sh", "-c",
        "grep -q 'staticlib' src/kernel/Cargo.toml || " ++
            "printf '\\n[lib]\\nname = \"kernel\"\\npath = \"src/main.rs\"\\ncrate-type = [\"staticlib\"]\\n' >> src/kernel/Cargo.toml",
    });
    patch_cargo.step.dependOn(&mk_dirs.step);

    const cargo_kernel = b.addSystemCommand(&.{
        "cargo",                "+nightly",            "build",
        "--release",            "--manifest-path",     "src/kernel/Cargo.toml",
        "--target",             "x86_64-unknown-none", "-Z",
        "build-std=core,alloc", "-Z",                  "build-std-features=compiler-builtins-mem",
    });
    cargo_kernel.step.dependOn(&patch_cargo.step);

    // ========================================================
    // KROK 4 — Linkuj kernel.elf
    //          boot.o + libkernel.a → kernel.elf  (przez linker.ld)
    // ========================================================
    const link_kernel = b.addSystemCommand(&.{
        "ld",
        "-n",
        "-T",
        "linker.ld",
        "-o",
        kernel_elf,
        boot_obj,
        "src/kernel/target/x86_64-unknown-none/release/libkernel.a",
    });
    link_kernel.step.dependOn(&asm_boot.step);
    link_kernel.step.dependOn(&cargo_kernel.step);

    // ========================================================
    // KROK 5 — Kompiluj userspace (Rust) → userspace_raw.bin
    // ========================================================
    const cargo_userspace = b.addSystemCommand(&.{
        "cargo",                "+nightly",            "build",
        "--release",            "--manifest-path",     "src/userspace/Cargo.toml",
        "--target",             "x86_64-unknown-none", "-Z",
        "build-std=core,alloc", "-Z",                  "build-std-features=compiler-builtins-mem",
    });
    cargo_userspace.step.dependOn(&mk_dirs.step);

    // Skopiuj binarny output userspace do build/
    const copy_userspace = b.addSystemCommand(&.{
        "cp",
        "src/userspace/target/x86_64-unknown-none/release/userspace",
        userspace_bin,
    });
    copy_userspace.step.dependOn(&cargo_userspace.step);

    // ========================================================
    // KROK 6 — Przygotuj strukturę ISO
    //          Skopiuj kernel.elf do iso/boot/
    // ========================================================
    const copy_kernel_to_iso = b.addSystemCommand(&.{
        "cp", kernel_elf, iso_dir ++ "/kernel.elf",
    });
    copy_kernel_to_iso.step.dependOn(&link_kernel.step);

    // Skopiuj userspace obok kernela (kernel go wczyta)
    const copy_userspace_to_iso = b.addSystemCommand(&.{
        "cp", userspace_bin, iso_dir ++ "/userspace.bin",
    });
    copy_userspace_to_iso.step.dependOn(&copy_userspace.step);

    // Stwórz grub.cfg jeśli nie istnieje
    const write_grub_cfg = b.addSystemCommand(&.{
        "sh", "-c",
        "[ -f " ++ grub_dir ++ "/grub.cfg ] || cat > " ++ grub_dir ++ "/grub.cfg << 'EOF'\n" ++
            "set timeout=0\n" ++
            "set default=0\n" ++
            "menuentry \"Cosinus OS\" {\n" ++
            "    multiboot2 /boot/kernel.elf\n" ++
            "    module2    /boot/userspace.bin userspace\n" ++
            "    boot\n" ++
            "}\n" ++
            "EOF",
    });
    write_grub_cfg.step.dependOn(&mk_dirs.step);

    // ========================================================
    // KROK 7 — Zbuduj ISO (grub-mkrescue)
    // ========================================================
    const make_iso = b.addSystemCommand(&.{
        "grub-mkrescue",
        "-o",
        iso_out,
        "iso",
    });
    make_iso.step.dependOn(&copy_kernel_to_iso.step);
    make_iso.step.dependOn(&copy_userspace_to_iso.step);
    make_iso.step.dependOn(&write_grub_cfg.step);

    // ========================================================
    // KROK 8 — Zbuduj raw disk image (64 MB)
    //          Format: ISO na początku dysku (hybrydowy obraz)
    // ========================================================
    const make_img = b.addSystemCommand(&.{
        "sh", "-c",
        // Utwórz pusty obraz 64MB, wgraj ISO jako hybrydowy MBR
        "dd if=/dev/zero of=" ++ img_out ++ " bs=1M count=64 2>/dev/null && " ++
            "dd if=" ++ iso_out ++ " of=" ++ img_out ++ " conv=notrunc 2>/dev/null",
    });
    make_img.step.dependOn(&make_iso.step);

    // ========================================================
    // KROK 9 — Uruchom QEMU
    // ========================================================
    var qemu_args = std.ArrayListUnmanaged([]const u8){};
    defer qemu_args.deinit(b.allocator);

    qemu_args.appendSlice(b.allocator, &.{
        "qemu-system-x86_64",
        "-cdrom",
        iso_out,
        "-drive",
        "file=" ++ img_out ++ ",format=raw,index=0,media=disk",
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
    }) catch @panic("OOM");

    if (debug) {
        qemu_args.appendSlice(b.allocator, &.{ "-d", "int,cpu_reset", "-D", build_dir ++ "/qemu.log" }) catch @panic("OOM");
    }

    if (qemu_wait_gdb) {
        qemu_args.appendSlice(b.allocator, &.{ "-s", "-S" }) catch @panic("OOM");
    }

    const run_qemu = b.addSystemCommand(qemu_args.items);
    run_qemu.step.dependOn(&make_img.step);

    // ========================================================
    // Eksponowane kroki (zig build <krok>)
    // ========================================================

    // zig build          → pełny build + QEMU
    const default_step = b.default_step;
    default_step.dependOn(&run_qemu.step);

    // zig build iso      → tylko ISO bez QEMU
    const iso_step = b.step("iso", "Tylko zbuduj ISO bez uruchamiania QEMU");
    iso_step.dependOn(&make_iso.step);

    // zig build img      → ISO + raw disk image
    const img_step = b.step("img", "Zbuduj ISO + raw disk image");
    img_step.dependOn(&make_img.step);

    // zig build kernel   → tylko kernel.elf
    const kernel_step = b.step("kernel", "Tylko skompiluj kernel.elf");
    kernel_step.dependOn(&link_kernel.step);

    // zig build userspace → tylko userspace
    const us_step = b.step("userspace", "Tylko skompiluj userspace");
    us_step.dependOn(&copy_userspace.step);

    // zig build run      → uruchom QEMU (zakłada że ISO już istnieje)
    const run_step = b.step("run", "Uruchom QEMU");
    run_step.dependOn(&run_qemu.step);

    // zig build clean    → usuń katalog build/
    const clean = b.addSystemCommand(&.{ "rm", "-rf", build_dir });
    const clean_step = b.step("clean", "Usuń katalog build/");
    clean_step.dependOn(&clean.step);
}
