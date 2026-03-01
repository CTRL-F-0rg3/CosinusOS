//! CosinusOS QEMU Diagnostics (Zig 0.15)
//! zig run diagnose.zig -- --full
//! zig run diagnose.zig -- --log [plik]
//! zig run diagnose.zig -- --sym [elf]

const std = @import("std");
const dp = std.debug.print;

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const alloc = gpa.allocator();

    const args = try std.process.argsAlloc(alloc);
    defer std.process.argsFree(alloc, args);

    const mode = if (args.len > 1) args[1] else "--help";

    if (std.mem.eql(u8, mode, "--run")) {
        try run_qemu(alloc);
    } else if (std.mem.eql(u8, mode, "--log")) {
        const path = if (args.len > 2) args[2] else "build/qemu-debug.log";
        try analyze_log(alloc, path);
    } else if (std.mem.eql(u8, mode, "--sym")) {
        const elf = if (args.len > 2) args[2] else "build/kernel.elf";
        try show_symbols(alloc, elf);
    } else if (std.mem.eql(u8, mode, "--full")) {
        try run_qemu(alloc);
        try analyze_log(alloc, "build/qemu-debug.log");
        try show_symbols(alloc, "build/kernel.elf");
    } else {
        dp("CosinusOS Diagnostic Tool\n", .{});
        dp("  --full   Uruchom QEMU + analiza logu + symbole\n", .{});
        dp("  --run    Uruchom QEMU 5s i zapisz log\n", .{});
        dp("  --log    Analizuj log (build/qemu-debug.log)\n", .{});
        dp("  --sym    Symbole kernela (build/kernel.elf)\n", .{});
    }
}

fn run_qemu(alloc: std.mem.Allocator) !void {
    dp("\n=== Uruchamiam QEMU (5s) ===\n", .{});
    std.fs.cwd().deleteFile("build/qemu-debug.log") catch {};

    const argv = &[_][]const u8{
        "qemu-system-x86_64",         "-cdrom",       "build/cosinusos.iso",
        "-m",                         "256M",         "-serial",
        "stdio",                      "-display",     "none",
        "-no-reboot",                 "-no-shutdown", "-d",
        "int,cpu_reset,guest_errors", "-D",           "build/qemu-debug.log",
    };

    var child = std.process.Child.init(argv, alloc);
    child.stdout_behavior = .Inherit;
    child.stderr_behavior = .Ignore;
    try child.spawn();
    std.Thread.sleep(5 * std.time.ns_per_s);
    _ = child.kill() catch {};
    _ = child.wait() catch {};
    dp("QEMU zatrzymany. Log: build/qemu-debug.log\n", .{});
}

const Exc = struct {
    vec: u8 = 0,
    err: u64 = 0,
    rip: u64 = 0,
    cr2: u64 = 0,
    cr3: u64 = 0,
    rsp: u64 = 0,
    line: usize = 0,
};

const MAX_EXC = 64;

fn analyze_log(alloc: std.mem.Allocator, path: []const u8) !void {
    dp("\n=== Analiza logu: {s} ===\n", .{path});

    const file = std.fs.cwd().openFile(path, .{}) catch |e| {
        dp("Blad otwarcia: {}\n", .{e});
        return;
    };
    defer file.close();

    const content = try file.readToEndAlloc(alloc, 64 * 1024 * 1024);
    defer alloc.free(content);

    var excs: [MAX_EXC]Exc = undefined;
    var nexc: usize = 0;

    var iter = std.mem.splitScalar(u8, content, '\n');
    var lineno: usize = 0;
    var cur = Exc{};
    var in_exc = false;

    while (iter.next()) |line| {
        lineno += 1;

        if (std.mem.indexOf(u8, line, "v=") != null and
            std.mem.indexOf(u8, line, "IP=") != null)
        {
            if (in_exc and nexc < MAX_EXC) {
                excs[nexc] = cur;
                nexc += 1;
            }
            cur = Exc{ .line = lineno };
            in_exc = true;
            cur.vec = phf(u8, line, "v=") orelse 0;
            cur.err = phf(u64, line, "e=") orelse 0;
            cur.rip = phf(u64, line, "IP=0008:") orelse
                phf(u64, line, "pc=") orelse 0;
        }
        if (in_exc) {
            if (phf(u64, line, "CR2=")) |v| cur.cr2 = v;
            if (phf(u64, line, "CR3=")) |v| cur.cr3 = v;
            if (phf(u64, line, "SP=0010:")) |v| cur.rsp = v;
        }
        if (std.mem.indexOf(u8, line, "Triple fault") != null) {
            if (in_exc and nexc < MAX_EXC) {
                excs[nexc] = cur;
                nexc += 1;
            }
            in_exc = false;
            dp("!!! TRIPLE FAULT (linia {}) !!!\n", .{lineno});
        }
    }
    if (in_exc and nexc < MAX_EXC) {
        excs[nexc] = cur;
        nexc += 1;
    }

    dp("Znaleziono {} wyjatkow CPU\n", .{nexc});
    dp("{s}\n", .{"=" ** 60});

    for (excs[0..nexc], 0..) |ex, i| {
        dp("[{d:>3}] #{s:<4} vec={X:0>2} RIP={X:0>16}\n", .{ i + 1, exc_name(ex.vec), ex.vec, ex.rip });
        dp("       err={X:0>16}  CR2={X:0>16}\n", .{ ex.err, ex.cr2 });
        dp("       CR3={X:0>16}  RSP={X:0>16}\n", .{ ex.cr3, ex.rsp });
        diagnose(ex);
        dp("\n", .{});
    }

    dp("=== PODSUMOWANIE ===\n", .{});
    for (excs[0..nexc], 0..) |ex, i| {
        if (ex.vec == 0x08 and i > 0 and excs[i - 1].vec == 0x0E) {
            dp("Lancuch: #PF(RIP={X:0>16}) -> #DF\n", .{excs[i - 1].rip});
            dp("Dostep do CR2={X:0>16}\n", .{excs[i - 1].cr2});
        }
    }
}

fn diagnose(ex: Exc) void {
    switch (ex.vec) {
        0x0E => {
            dp("       #PF: ", .{});
            if (ex.err & 1 != 0) dp("PRESENT ", .{}) else dp("NOT_PRESENT ", .{});
            if (ex.err & 2 != 0) dp("WRITE ", .{});
            if (ex.err & 4 != 0) dp("USER ", .{}) else dp("KERNEL ", .{});
            if (ex.err & 16 != 0) dp("IFETCH ", .{});
            dp("\n", .{});
            if (ex.cr2 == ex.cr3 and ex.cr2 != 0)
                dp("       >>> CR2==CR3! blad page tables zaladowanych jako CR3\n", .{});
            if (ex.rip < 0x1000)
                dp("       >>> RIP={X} bliskie NULL\n", .{ex.rip});
            if (ex.cr2 < 0x1000 and ex.cr2 != 0)
                dp("       >>> CR2={X} bliskie NULL - null ptr deref\n", .{ex.cr2});
            if (ex.rip >= 0x400000 and ex.rip < 0x800000)
                dp("       >>> RIP w zakresie userspace (0x400000+)\n", .{});
        },
        0x08 => {
            dp("       #DF: poprzedni handler crashnal\n", .{});
            if (ex.rip == 0)
                dp("       >>> RIP=0: iretq zaladowalo zly stos\n", .{});
        },
        0x0D => dp("       #GP: General Protection Fault\n", .{}),
        else => {},
    }
}

fn show_symbols(alloc: std.mem.Allocator, elf_path: []const u8) !void {
    dp("\n=== Symbole kernela: {s} ===\n", .{elf_path});

    const result = std.process.Child.run(.{
        .allocator = alloc,
        .argv = &[_][]const u8{ "nm", "--numeric-sort", "--defined-only", elf_path },
        .max_output_bytes = 10 * 1024 * 1024,
    }) catch |e| {
        dp("Blad nm: {} (zainstaluj binutils)\n", .{e});
        return;
    };
    defer alloc.free(result.stdout);
    defer alloc.free(result.stderr);

    dp("{s}\n", .{"-" ** 50});
    var lines = std.mem.splitScalar(u8, result.stdout, '\n');
    var count: usize = 0;
    while (lines.next()) |line| {
        if (line.len < 19) continue;
        if (line[17] == 'T' or line[17] == 't') {
            const name = if (line.len > 19) line[19..] else "?";
            dp("0x{s}  {s}\n", .{ line[0..16], name });
            count += 1;
        }
    }
    dp("\n{} funkcji\n", .{count});
}

fn exc_name(vec: u8) []const u8 {
    return switch (vec) {
        0x06 => "UD",
        0x08 => "DF",
        0x0D => "GP",
        0x0E => "PF",
        0x20 => "IRQ0",
        0x21 => "IRQ1",
        else => "??",
    };
}

fn phf(comptime T: type, line: []const u8, key: []const u8) ?T {
    const idx = std.mem.indexOf(u8, line, key) orelse return null;
    var s = idx + key.len;
    if (s + 2 <= line.len and line[s] == '0' and
        (line[s + 1] == 'x' or line[s + 1] == 'X')) s += 2;
    var e = s;
    while (e < line.len and isHex(line[e])) e += 1;
    if (e == s) return null;
    const v = std.fmt.parseInt(u64, line[s..e], 16) catch return null;
    return @intCast(v);
}

fn isHex(c: u8) bool {
    return (c >= '0' and c <= '9') or (c >= 'a' and c <= 'f') or (c >= 'A' and c <= 'F');
}
