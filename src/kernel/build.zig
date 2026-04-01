const std = @import("std");

pub fn build(b: *std.Build) void {
    const build_dir = "../../build";

    // -----------------------------------------------------------------------
    // NASM — original kernel assembly
    // -----------------------------------------------------------------------
    const nasm_boot = b.addSystemCommand(&.{
        "nasm",           "-f", "elf64",
        "../../boot.asm", "-o", build_dir ++ "/boot.o",
    });
    const nasm_tramp = b.addSystemCommand(&.{
        "nasm",          "-f", "elf64",
        "src/tramp.asm", "-o", build_dir ++ "/tramp.o",
    });
    const nasm_eu = b.addSystemCommand(&.{
        "nasm",                    "-f", "elf64",
        "src/enter_userspace.asm", "-o", build_dir ++ "/enter_userspace.o",
    });

    // -----------------------------------------------------------------------
    // NASM — allocator hot paths
    // -----------------------------------------------------------------------
    const nasm_bitmap = b.addSystemCommand(&.{
        "nasm",                             "-f", "elf64",
        "src/allocator/asm/bitmap_ops.asm", "-o", build_dir ++ "/bitmap_ops.o",
    });
    const nasm_slab = b.addSystemCommand(&.{
        "nasm",                               "-f", "elf64",
        "src/allocator/asm/slab_hotpath.asm", "-o", build_dir ++ "/slab_hotpath.o",
    });

    // -----------------------------------------------------------------------
    // Ada — allocator integrity / audit / lifecycle
    //
    // Problem: na wielu systemach `gcc` nie ma frontendu Ada (brak gnat1).
    // Rozwiązanie: używamy `gnat compile` który wywołuje właściwy
    // x86_64-linux-gnu-gcc-XX z Ada frontend.
    //
    // Problem 2: `gnat compile` nie obsługuje -o — zawsze zapisuje .o
    // do bieżącego katalogu jako <basename>.o.
    // Rozwiązanie: cd do katalogu ze źródłem, kompiluj, mv do build/.
    //
    // Ścieżka: build.zig jest w src/kernel/
    //   src/kernel/src/allocator/ada/ -> ../../../../.. -> CosinusOS_/
    //   build_dir = "../../build" -> CosinusOS_/build/
    //   Z src/allocator/ada/ do ../../build/ = ../../../../../build/
    // -----------------------------------------------------------------------
    const ada_flags = "-fno-exceptions -fno-stack-protector -O2 -mno-red-zone -mcmodel=large";
    const ada_out = "../../../../../build";

    const ada_integrity = b.addSystemCommand(&.{
        "sh", "-c",
        "cd src/allocator/ada && " ++
            "gnat compile " ++ ada_flags ++ " integrity_checks.adb && " ++
            "mv integrity_checks.o " ++ ada_out ++ "/ada_integrity.o",
    });
    const ada_audit = b.addSystemCommand(&.{
        "sh", "-c",
        "cd src/allocator/ada && " ++
            "gnat compile " ++ ada_flags ++ " audit_log.adb && " ++
            "mv audit_log.o " ++ ada_out ++ "/ada_audit.o",
    });
    const ada_lifecycle = b.addSystemCommand(&.{
        "sh", "-c",
        "cd src/allocator/ada && " ++
            "gnat compile " ++ ada_flags ++ " lifecycle.adb && " ++
            "mv lifecycle.o " ++ ada_out ++ "/ada_lifecycle.o",
    });

    // GNAT runtime stubs — zwykłe C, kompiluje cc (nie gcc żeby uniknąć
    // problemów z brakującym gnat1 gdy gcc = wrapper)
    const ada_stubs = b.addSystemCommand(&.{
        "cc",                                     "-c", "-O2",                       "-mno-red-zone", "-mcmodel=large",
        "src/allocator/ada/gnat_runtime_stubs.c", "-o", build_dir ++ "/ada_stubs.o",
    });

    // -----------------------------------------------------------------------
    // Rust kernel
    // -----------------------------------------------------------------------
    const cargo_kernel = b.addSystemCommand(&.{
        "cargo",           "+nightly",
        "build",           "--release",
        "--manifest-path", "Cargo.toml",
        "--target",        "x86_64-cosinus.json",
        "-Z",              "json-target-spec",
        "-Z",              "build-std=core,compiler_builtins",
        "-Z",              "build-std-features=compiler-builtins-mem",
        "--target-dir",    build_dir ++ "/kernel_target",
    });
    cargo_kernel.step.dependOn(&nasm_bitmap.step);
    cargo_kernel.step.dependOn(&nasm_slab.step);
    cargo_kernel.step.dependOn(&ada_integrity.step);
    cargo_kernel.step.dependOn(&ada_audit.step);
    cargo_kernel.step.dependOn(&ada_lifecycle.step);
    cargo_kernel.step.dependOn(&ada_stubs.step);

    // -----------------------------------------------------------------------
    // Link
    // -----------------------------------------------------------------------
    const link_kernel = b.addSystemCommand(&.{
        "ld",
        "-T",
        "linker.ld",
        "-nostdlib",
        "-static",
        "-no-pie",
        "--no-warn-rwx-segments",
        "-z",
        "noexecstack",
        "-o",
        build_dir ++ "/kernel.elf",
        build_dir ++ "/boot.o",
        build_dir ++ "/tramp.o",
        build_dir ++ "/enter_userspace.o",
        build_dir ++ "/bitmap_ops.o",
        build_dir ++ "/slab_hotpath.o",
        build_dir ++ "/ada_integrity.o",
        build_dir ++ "/ada_audit.o",
        build_dir ++ "/ada_lifecycle.o",
        build_dir ++ "/ada_stubs.o",
        build_dir ++ "/kernel_target/x86_64-cosinus/release/libkernel.a",
    });
    link_kernel.step.dependOn(&cargo_kernel.step);
    link_kernel.step.dependOn(&nasm_boot.step);
    link_kernel.step.dependOn(&nasm_tramp.step);
    link_kernel.step.dependOn(&nasm_eu.step);

    // -----------------------------------------------------------------------
    // ISO
    // -----------------------------------------------------------------------
    const copy_to_iso = b.addSystemCommand(&.{
        "sh", "-c",
        "mkdir -p ../../iso/boot && cp " ++
            build_dir ++ "/kernel.elf ../../iso/boot/kernel.elf",
    });
    copy_to_iso.step.dependOn(&link_kernel.step);
    b.default_step.dependOn(&copy_to_iso.step);

    // -----------------------------------------------------------------------
    // Clean
    // -----------------------------------------------------------------------
    const clean = b.step("clean", "Clean kernel");
    const clean_cmd = b.addSystemCommand(&.{
        "rm",                              "-rf",
        build_dir ++ "/boot.o",            build_dir ++ "/tramp.o",
        build_dir ++ "/enter_userspace.o", build_dir ++ "/bitmap_ops.o",
        build_dir ++ "/slab_hotpath.o",    build_dir ++ "/ada_integrity.o",
        build_dir ++ "/ada_audit.o",       build_dir ++ "/ada_lifecycle.o",
        build_dir ++ "/ada_stubs.o",       build_dir ++ "/kernel.elf",
        build_dir ++ "/kernel_target",
    });
    clean.dependOn(&clean_cmd.step);
}
