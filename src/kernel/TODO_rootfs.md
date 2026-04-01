# CosinusOS — Root Filesystem (rootfs)

> Next major milestone after stable userspace execution.
> Goal: a minimal, CosinusOS-native VFS root that init and userspace programs
> can mount, traverse, and use as the single namespace anchor.

---

## 0. Background & Decisions

- VFS layer already has a `Zig` skeleton (`vfs/`) — rootfs plugs in as the
  first registered filesystem driver.
- Kernel uses `int 0x80` ABI with 14 syscalls — file syscalls (`open`, `read`,
  `write`, `close`, `stat`) need to be added or wired up.
- Target: rootfs lives on the initrd image (flat binary embedded in the kernel
  ELF or loaded by the bootloader as a separate module).
- Long-term: CSFS (CosinusOS native FS) on ATA/IDE; rootfs is the bootstrap
  before that driver is ready.

---

## 1. VFS Core Extensions

- [ ] Define `VNode` type: inode number, type (file/dir/symlink), size, permissions, driver pointer
- [ ] Define `VFS_Driver` trait/interface: `mount`, `lookup`, `read`, `write`, `readdir`, `stat`
- [ ] Implement mount table: array of `(path, driver)` pairs, max ~16 entries
- [ ] Implement `vfs_lookup(path)` — walk mount table, delegate to driver
- [ ] Implement `vfs_open` / `vfs_close` with per-process file descriptor table
- [ ] Implement `vfs_read` / `vfs_write` dispatch
- [ ] Implement `vfs_readdir` for directory listing
- [ ] Implement `vfs_stat` returning size + type

---

## 2. Initrd Format

Choose one format for the initial ramdisk:

- [ ] **Option A — flat CPIO (newc)** — standard, easy to generate with
  `find . | cpio -o -H newc > initrd.img`, widely supported
- [ ] **Option B — custom CosinusOS archive (CSFS-lite)** — simpler header,
  no POSIX metadata overhead, easier to parse in `no_std` Rust
- [ ] Decide and document the choice in `docs/initrd_format.md`
- [ ] Write initrd parser (Rust, `no_std`) that produces a `VFS_Driver` impl
- [ ] Embed initrd in kernel ELF via linker script section `.initrd` **or**
  receive as Multiboot2 module (already loaded at boot — check `MB2 magic` path)

---

## 3. `rootfs` Driver Implementation

- [ ] Create `src/fs/rootfs/` module
- [ ] Implement `RootfsDriver` backed by the parsed initrd image
- [ ] `mount("/", rootfs_driver)` called from kernel `init` sequence
- [ ] `lookup` walks the parsed file tree by path segments
- [ ] `read` returns bytes from the in-memory initrd region
- [ ] `readdir` enumerates directory entries
- [ ] All operations are read-only for now (initrd is ROM)

---

## 4. Syscall Wiring

Add file syscalls to the existing `int 0x80` handler:

- [ ] `sys_open(path: *const u8, flags: u32) -> i64` — returns fd or negative errno
- [ ] `sys_close(fd: i64) -> i64`
- [ ] `sys_read(fd: i64, buf: *mut u8, len: u64) -> i64` — returns bytes read
- [ ] `sys_write(fd: i64, buf: *const u8, len: u64) -> i64` — already exists for stdout, extend for files
- [ ] `sys_stat(path: *const u8, out: *mut StatBuf) -> i64`
- [ ] `sys_getdents(fd: i64, buf: *mut DirEnt, len: u64) -> i64`
- [ ] Wire all of the above through `syscall.rs` dispatch table
- [ ] Update `libcosinus` userspace shims to expose these as safe wrappers

---

## 5. Per-Process File Descriptor Table

- [ ] Add `fd_table: [Option<FileDescriptor>; 32]` to `Thread` / process struct
- [ ] `FileDescriptor`: vnode pointer, offset, flags (read/write/append)
- [ ] Reserve fd 0/1/2 as stdin/stdout/stderr (mapped to serial / framebuffer)
- [ ] `sys_open` allocates lowest free fd slot
- [ ] `sys_close` frees the slot and drops the vnode reference

---

## 6. Init Process Integration

- [ ] `init` (Rust `no_std` process) calls `sys_open("/etc/init.conf")` at startup
- [ ] Parse a minimal config: list of programs to exec at boot
- [ ] Each entry: path to ELF binary in rootfs + optional args
- [ ] `init` forks (or spawns via `sys_spawn`) each listed program
- [ ] On child exit, `init` supervisor loop restarts it (already partially implemented)

---

## 7. Build & Tooling

- [ ] Add `tools/mkrootfs.sh` — assembles a directory tree into the chosen
  initrd format and outputs `build/initrd.img`
- [ ] Add rootfs directory tree: `rootfs/bin/`, `rootfs/etc/`, `rootfs/dev/`
- [ ] Populate `rootfs/etc/init.conf` with initial program list
- [ ] Integrate `mkrootfs.sh` into `build.zig` as a step before kernel link
- [ ] Pass initrd to QEMU: `-initrd build/initrd.img` or embed in ELF

---

## 8. Testing Checklist

- [ ] Kernel boots and mounts rootfs without panic
- [ ] `sys_open("/etc/init.conf")` returns valid fd
- [ ] `sys_read` returns correct bytes
- [ ] `sys_readdir("/bin")` lists expected entries
- [ ] Init process reads config and spawns at least one userspace binary from rootfs
- [ ] Double-open same file returns independent fd with independent offset
- [ ] `sys_close` on invalid fd returns `-EBADF`
- [ ] Path traversal outside rootfs (`../../etc/passwd`) correctly rejected

---

## Order of Attack

```
initrd format decision
        │
        ▼
initrd parser (Rust no_std)
        │
        ▼
RootfsDriver (read-only VFS)
        │
        ▼
mount("/") in kernel init
        │
        ▼
fd table in Thread struct
        │
        ▼
sys_open / sys_read / sys_close / sys_stat
        │
        ▼
libcosinus wrappers
        │
        ▼
init reads /etc/init.conf
        │
        ▼
mkrootfs.sh + build.zig integration
```
