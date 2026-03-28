// utils.zig — path parsing + string helpers (no_std safe)

// ─── Path iterator ────────────────────────────────────────────────────────────
// Iteruje przez komponenty ścieżki: "/foo/bar/baz" → "foo", "bar", "baz"

pub const PathIter = struct {
    path: []const u8,
    pos: usize,

    pub fn init(path: []const u8) PathIter {
        var pos: usize = 0;
        // Pomiń wiodące slashe
        while (pos < path.len and path[pos] == '/') pos += 1;
        return .{ .path = path, .pos = pos };
    }

    pub fn next(self: *PathIter) ?[]const u8 {
        if (self.pos >= self.path.len) return null;

        const start = self.pos;
        while (self.pos < self.path.len and self.path[self.pos] != '/') {
            self.pos += 1;
        }
        const component = self.path[start..self.pos];

        // Pomiń slashe po komponencie
        while (self.pos < self.path.len and self.path[self.pos] == '/') {
            self.pos += 1;
        }

        if (component.len == 0) return null;
        return component;
    }

    pub fn isAtEnd(self: *const PathIter) bool {
        return self.pos >= self.path.len;
    }
};

// ─── Path helpers ─────────────────────────────────────────────────────────────

pub fn isAbsolute(path: []const u8) bool {
    return path.len > 0 and path[0] == '/';
}

/// Zwraca nazwę pliku (ostatni komponent ścieżki)
pub fn basename(path: []const u8) []const u8 {
    var end = path.len;
    while (end > 0 and path[end - 1] == '/') end -= 1;
    if (end == 0) return "/";

    var start = end;
    while (start > 0 and path[start - 1] != '/') start -= 1;
    return path[start..end];
}

/// Zwraca katalog (wszystko przed ostatnim componentem)
pub fn dirname(path: []const u8) []const u8 {
    var end = path.len;
    while (end > 0 and path[end - 1] == '/') end -= 1;
    while (end > 0 and path[end - 1] != '/') end -= 1;
    while (end > 1 and path[end - 1] == '/') end -= 1;
    if (end == 0) return ".";
    return path[0..end];
}

// ─── String helpers ───────────────────────────────────────────────────────────

pub fn streq(a: []const u8, b: []const u8) bool {
    if (a.len != b.len) return false;
    for (a, b) |ca, cb| if (ca != cb) return false;
    return true;
}

pub fn streqZ(a: [*:0]const u8, b: []const u8) bool {
    var i: usize = 0;
    while (a[i] != 0) : (i += 1) {
        if (i >= b.len or a[i] != b[i]) return false;
    }
    return i == b.len;
}

/// Uppercase ASCII — dla FAT32 (case-insensitive filenames)
pub fn toUpper(c: u8) u8 {
    return if (c >= 'a' and c <= 'z') c - 32 else c;
}

pub fn streqCaseInsensitive(a: []const u8, b: []const u8) bool {
    if (a.len != b.len) return false;
    for (a, b) |ca, cb| if (toUpper(ca) != toUpper(cb)) return false;
    return true;
}

/// Zapisz u16 little-endian do bufora
pub fn writeU16LE(buf: []u8, offset: usize, val: u16) void {
    buf[offset + 0] = @truncate(val);
    buf[offset + 1] = @truncate(val >> 8);
}

/// Odczytaj u16 little-endian z bufora
pub fn readU16LE(buf: []const u8, offset: usize) u16 {
    return @as(u16, buf[offset]) | (@as(u16, buf[offset + 1]) << 8);
}

pub fn writeU32LE(buf: []u8, offset: usize, val: u32) void {
    buf[offset + 0] = @truncate(val);
    buf[offset + 1] = @truncate(val >> 8);
    buf[offset + 2] = @truncate(val >> 16);
    buf[offset + 3] = @truncate(val >> 24);
}

pub fn readU32LE(buf: []const u8, offset: usize) u32 {
    return @as(u32, buf[offset]) | (@as(u32, buf[offset + 1]) << 8) | (@as(u32, buf[offset + 2]) << 16) | (@as(u32, buf[offset + 3]) << 24);
}

pub fn writeU64LE(buf: []u8, offset: usize, val: u64) void {
    writeU32LE(buf, offset, @truncate(val));
    writeU32LE(buf, offset + 4, @truncate(val >> 32));
}

pub fn readU64LE(buf: []const u8, offset: usize) u64 {
    return @as(u64, readU32LE(buf, offset)) | (@as(u64, readU32LE(buf, offset + 4)) << 32);
}

// ─── FAT32 name helpers ───────────────────────────────────────────────────────

/// Konwertuj 8.3 FAT name (11 bajtów, space-padded) → normalny string
pub fn fat83ToString(fat_name: *const [11]u8, out: []u8) usize {
    var i: usize = 0;
    // Nazwa (pierwsze 8 bajtów)
    var name_end: usize = 8;
    while (name_end > 0 and fat_name[name_end - 1] == ' ') name_end -= 1;
    for (fat_name[0..name_end]) |c| {
        if (i >= out.len) break;
        out[i] = c;
        i += 1;
    }
    // Rozszerzenie (bajty 8–10)
    var ext_end: usize = 3;
    while (ext_end > 0 and fat_name[8 + ext_end - 1] == ' ') ext_end -= 1;
    if (ext_end > 0) {
        if (i < out.len) {
            out[i] = '.';
            i += 1;
        }
        for (fat_name[8 .. 8 + ext_end]) |c| {
            if (i >= out.len) break;
            out[i] = c;
            i += 1;
        }
    }
    return i;
}

/// Konwertuj normalny string → 8.3 FAT name (space-padded)
pub fn stringToFat83(name: []const u8, out: *[11]u8) void {
    @memset(out, ' ');
    var dot_pos: ?usize = null;
    for (name, 0..) |c, i| if (c == '.') {
        dot_pos = i;
    };

    const name_part = if (dot_pos) |d| name[0..d] else name;
    const ext_part = if (dot_pos) |d| name[@min(d + 1, name.len)..] else &[_]u8{};

    for (name_part[0..@min(name_part.len, 8)], 0..) |c, i| {
        out[i] = toUpper(c);
    }
    for (ext_part[0..@min(ext_part.len, 3)], 0..) |c, i| {
        out[8 + i] = toUpper(c);
    }
}
