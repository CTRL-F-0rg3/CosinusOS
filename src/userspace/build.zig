const std = @import("std");

pub fn build(b: *std.Build) void {
    const build_dir = "../../build";
    const iso_boot_dir = "../../iso/boot";
    const us_target = build_dir ++ "/userspace_target";

    // ── 1. Cargo — kompiluje userspace jako ELF (nie flat binary) ────────────
    // Linker script (linker.ld) ustawia ENTRY(_start) i bazę na 0x400000.
    // Wynik to poprawny ELF64 ET_EXEC — kernel loader zobaczy magic 0x7F ELF
    // i wejdzie w load_elf64, entry point z nagłówka ELF.
    const cargo_init = b.addSystemCommand(&.{
        "cargo",           "+nightly",
        "build",           "--release",
        "--manifest-path", "Cargo.toml",
        "--target",        "x86_64-unknown-none",
        "-Z",              "build-std=core,alloc",
        "--target-dir",    us_target,
    });

    // ── 2. Filesystem server (Zig) ────────────────────────────────────────────
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
    fs_server.root_module.stack_protector = false;
    fs_server.root_module.red_zone = false;
    fs_server.link_z_max_page_size = 4096;
    fs_server.entry = .{ .symbol_name = "main" };

    // ── 3. Katalog docelowy ───────────────────────────────────────────────────
    const mkdir_cmd = b.addSystemCommand(&.{ "mkdir", "-p", iso_boot_dir });

    // ── 4. Kopiuj userspace ELF bezpośrednio — BEZ objcopy -O binary ─────────
    // Wcześniej robiliśmy objcopy --only-section, co dawało flat binary gdzie
    // _start nie musiał być na początku pliku (Rust przy opt-level=z przestawia
    // funkcje). Teraz kopiujemy ELF, kernel load_elf64 czyta entry point
    // bezpośrednio z nagłówka — niezależnie od kolejności funkcji w .text.
    const cp_init = b.addSystemCommand(&.{
        "cp",
        us_target ++ "/x86_64-unknown-none/release/userspace",
        iso_boot_dir ++ "/userspace.bin",
    });
    cp_init.step.dependOn(&cargo_init.step);
    cp_init.step.dependOn(&mkdir_cmd.step);

    // ── 5. Filesystem server — nadal flat binary (strip sekcje) ──────────────
    // fs_server jest ładowany inaczej niż userspace init, zostawiamy jak było.
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

    const cp_fs = b.addSystemCommand(&.{
        "cp",
        build_dir ++ "/fs_server.bin",
        iso_boot_dir ++ "/fs_server.bin",
    });
    cp_fs.step.dependOn(&strip_fs.step);
    cp_fs.step.dependOn(&mkdir_cmd.step);

    // ── 6. Default ────────────────────────────────────────────────────────────
    const export_step = b.step("export", "Export binaries to ISO");
    export_step.dependOn(&cp_init.step);
    export_step.dependOn(&cp_fs.step);
    b.default_step.dependOn(export_step);

    // ── 7. Clean ──────────────────────────────────────────────────────────────
    const clean = b.step("clean", "Remove userspace build artifacts");
    const clean_cmd = b.addSystemCommand(&.{
        "rm",                          "-rf",
        us_target,                     iso_boot_dir ++ "/userspace.bin",
        build_dir ++ "/fs_server.bin", iso_boot_dir ++ "/fs_server.bin",
    });
    clean.dependOn(&clean_cmd.step);
}
