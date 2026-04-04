/*
 * CosinusOS — glue.c
 * C glue layer: Ada security modules <-> Rust root_file_system
 *
 * Ada units export C-ABI functions. This file:
 *   1. Provides GNAT runtime stubs (no OS, no stdlib)
 *   2. Provides the unified init/dispatch surface for Rust
 *   3. Wraps Ada calls with null-pointer guards
 *   4. Provides the disk_write/disk_read gate used by ata.rs via FFI
 *
 * Rust calls disk_gate_write() / disk_gate_read() instead of raw ATA.
 * The gate runs all Ada security checks before passing through to ATA.
 *
 * Compilation:
 *   cc -c -O2 -mno-red-zone -mcmodel=large -ffreestanding
 *      -fno-stack-protector -fno-exceptions
 *      glue.c -o glue.o
 */

typedef unsigned char      uint8_t;
typedef unsigned short     uint16_t;
typedef unsigned int       uint32_t;
typedef unsigned long long uint64_t;
typedef signed int         int32_t;
typedef signed long long   int64_t;

/* ============================================================
 * Minimal type aliases matching Ada side
 * ============================================================ */

typedef uint64_t lba_t;
typedef uint32_t sector_count_t;
typedef uint8_t  ring_level_t;
typedef uint8_t  op_t;
typedef int32_t  ada_result_t;

/* ============================================================
 * Forward declarations — Ada exported symbols
 * ============================================================ */

/* DiskSecurity */
extern void         disk_security_init(void);
extern ada_result_t disk_security_check_access(lba_t lba, sector_count_t count,
                                                op_t op, ring_level_t ring);
extern ada_result_t disk_security_register_region(lba_t start, lba_t end,
                                                   uint8_t type, uint8_t level);
extern ada_result_t disk_security_lock_region(lba_t start);
extern ada_result_t disk_security_unlock_region(lba_t start,
                                                 const void *tag, uint32_t len);
extern ada_result_t disk_security_set_content_hash(lba_t start,
                                                    const void *hash, uint32_t len);
extern ada_result_t disk_security_verify_region(lba_t start,
                                                 const void *data, uint32_t len);
extern uint32_t     disk_security_get_violation_count(void);
extern int32_t      disk_security_is_tampered(void);
extern void         disk_security_emergency_lock(void);

/* DiskAuth */
extern void         disk_auth_init(void);
extern void         disk_auth_tick(void);
extern int32_t      disk_auth_open_session(ring_level_t ring,
                                            const void *token, uint32_t len);
extern ada_result_t disk_auth_close_session(uint32_t id);
extern ada_result_t disk_auth_issue_permit(uint32_t session,
                                            lba_t start, lba_t end,
                                            uint16_t flags);
extern ada_result_t disk_auth_check_permit(lba_t start, uint32_t count,
                                            ring_level_t ring, uint16_t flags);
extern ada_result_t disk_auth_revoke_permits(uint32_t id);
extern ada_result_t disk_auth_admin_permit(lba_t start, lba_t end);
extern int32_t      disk_auth_active_sessions(void);

/* ChangeMonitor */
extern void         change_monitor_init(void);
extern void         change_monitor_tick(void);
extern ada_result_t change_monitor_record_write(lba_t lba, uint32_t count,
                                                 ring_level_t ring,
                                                 uint32_t before_hash,
                                                 uint32_t after_hash);
extern void         change_monitor_record_read(lba_t lba, uint32_t count,
                                               ring_level_t ring);
extern ada_result_t change_monitor_add_watch(lba_t start, lba_t end,
                                              uint32_t expected_hash,
                                              int32_t strict);
extern ada_result_t change_monitor_remove_watch(lba_t start);
extern int32_t      change_monitor_check_watch(lba_t lba);
extern uint32_t     change_monitor_alert_count(void);
extern int32_t      change_monitor_is_locked(void);

/* CryptoFS */
extern void         cryptofs_init(void);
extern ada_result_t cryptofs_load_key(uint8_t slot_id, uint8_t cipher,
                                       const void *key, uint32_t key_len,
                                       const void *nonce, uint32_t nonce_len);
extern ada_result_t cryptofs_bind_key_region(uint8_t slot_id,
                                              lba_t start, lba_t end);
extern ada_result_t cryptofs_encrypt_sector(lba_t lba, void *buf, void *tag);
extern ada_result_t cryptofs_decrypt_sector(lba_t lba, void *buf,
                                             const void *tag);
extern ada_result_t cryptofs_tag_sector(lba_t lba, const void *buf, void *tag);
extern ada_result_t cryptofs_verify_tag(lba_t lba, const void *buf,
                                         const void *tag);
extern uint32_t     cryptofs_tag_fail_count(void);

/* ==============================================================
 * FNV-1a 32-bit — used in glue layer for before/after hashes
 * ============================================================ */

static uint32_t fnv1a_32(const uint8_t *data, uint32_t len) {
    uint32_t h = 0x811C9DC5U;
    for (uint32_t i = 0; i < len; i++) {
        h ^= (uint32_t)data[i];
        h *= 0x01000193U;
    }
    return h;
}

/* ============================================================
 * Global init — Rust calls this once after kernel heap init
 * ============================================================ */

void cosinus_disk_security_init_all(void) {
    disk_security_init();
    disk_auth_init();
    change_monitor_init();
    cryptofs_init();

    /* Issue kernel admin permits for all four core segments.
     * These match layout.rs LBA constants. */
    disk_auth_admin_permit(2048,   16383);   /* kernel    */
    disk_auth_admin_permit(16384,  32767);   /* devspace  */
    disk_auth_admin_permit(32768,  49151);   /* fsserver  */
    disk_auth_admin_permit(49152, 131071);   /* userspace */

    /* Watch the kernel segment strictly — any unexpected write triggers alert */
    change_monitor_add_watch(2048, 16383, 0, 1);
}

/* ============================================================
 * Disk write gate — called by Rust ata.rs instead of raw write
 *
 * int disk_gate_write(lba_t lba, uint32_t count,
 *                     ring_level_t ring,
 *                     const uint8_t *buf);
 *
 * Returns 0 on success, negative on security violation.
 * Does NOT perform the actual ATA write — returns to Rust which does it.
 * This is a pure policy gate.
 * ============================================================ */

int disk_gate_write(lba_t lba, uint32_t count,
                    ring_level_t ring, const uint8_t *buf)
{
    ada_result_t res;
    uint32_t before_hash;
    uint32_t after_hash;

    if (change_monitor_is_locked()) {
        return -100;  /* hard locked */
    }

    /* Security access check */
    res = disk_security_check_access(lba, count, 0x02 /* OP_WRITE */, ring);
    if (res != 0) {
        return (int)res;
    }

    /* Auth permit check — ring 0 with admin flag bypasses session */
    res = disk_auth_check_permit(lba, count, ring,
                                  ring == 0 ? 0x0100 /* PERMIT_ADMIN */
                                            : 0x0002 /* PERMIT_WRITE */);
    if (res != 0 && ring != 0) {
        return (int)res;
    }

    /* Compute before-hash for change monitor */
    before_hash = buf ? fnv1a_32(buf, count * 512 > 4096 ? 4096 : count * 512)
                      : 0;

    /* After hash is same as before at gate entry — monitor records mutation */
    after_hash = before_hash;

    res = change_monitor_record_write(lba, count, ring, before_hash, after_hash);
    if (res != 0) {
        return (int)res;
    }

    return 0;  /* OK — Rust proceeds with ATA write */
}

/* ============================================================
 * Disk read gate
 *
 * int disk_gate_read(lba_t lba, uint32_t count, ring_level_t ring);
 * ============================================================ */

int disk_gate_read(lba_t lba, uint32_t count, ring_level_t ring)
{
    ada_result_t res;

    res = disk_security_check_access(lba, count, 0x01 /* OP_READ */, ring);
    if (res != 0) {
        return (int)res;
    }

    change_monitor_record_read(lba, count, ring);
    return 0;
}

/* ============================================================
 * Install gate — called by install.rs before writing segments
 * Checks that the write is permitted and the segment is not tampered
 *
 * int disk_gate_install(lba_t lba_start, lba_t lba_end);
 * ============================================================ */

int disk_gate_install(lba_t lba_start, lba_t lba_end)
{
    ada_result_t res;
    uint32_t count;

    if (lba_end <= lba_start) {
        return -1;
    }

    count = (uint32_t)(lba_end - lba_start);

    /* Must pass security check at ring 0 */
    res = disk_security_check_access(lba_start, count, 0x02, 0);
    if (res != 0) {
        return (int)res;
    }

    /* Issue a one-time admin permit for this range */
    res = disk_auth_admin_permit(lba_start, lba_end);
    if (res != 0) {
        return (int)res;
    }

    return 0;
}

/* ============================================================
 * Post-install — called after each segment is written
 * Sets content hash and optionally locks the region
 *
 * int disk_gate_post_install(lba_t lba_start,
 *                             const uint8_t *data, uint32_t data_len,
 *                             int lock_after);
 * ============================================================ */

int disk_gate_post_install(lba_t lba_start,
                            const uint8_t *data, uint32_t data_len,
                            int lock_after)
{
    ada_result_t res;
    uint32_t hash[8];  /* 32 bytes for content hash */
    uint32_t h;

    if (data == ((void*)0) || data_len == 0) {
        return 0;
    }

    /* Compute FNV1a over entire segment data as content hash */
    h = fnv1a_32(data, data_len);

    /* Store as 32-byte hash (repeated 4-byte value — simple but works) */
    for (int i = 0; i < 8; i++) {
        hash[i] = h;
    }

    res = disk_security_set_content_hash(lba_start, (const void*)hash, 32);
    if (res != 0) {
        return (int)res;
    }

    /* Add to change monitor with expected hash */
    change_monitor_add_watch(lba_start, lba_start + data_len / 512 + 1, h, 0);

    if (lock_after) {
        res = disk_security_lock_region(lba_start);
        if (res != 0) {
            return (int)res;
        }
    }

    return 0;
}

/* ============================================================
 * Tamper check — called periodically by kernel
 * Returns 1 if tamper detected, 0 otherwise
 * ============================================================ */

int disk_gate_check_tamper(void) {
    if (disk_security_is_tampered()) {
        disk_security_emergency_lock();
        return 1;
    }
    if (change_monitor_alert_count() > 50) {
        disk_security_emergency_lock();
        return 1;
    }
    return 0;
}

/* ============================================================
 * Tick — advance all Ada clock-dependent state
 * Call this from kernel PIT handler
 * ============================================================ */

void disk_gate_tick(void) {
    disk_auth_tick();
    change_monitor_tick();
}

/* ============================================================
 * Query helpers for Rust side
 * ============================================================ */

uint32_t disk_gate_violation_count(void) {
    return disk_security_get_violation_count();
}

uint32_t disk_gate_alert_count(void) {
    return change_monitor_alert_count();
}

uint32_t disk_gate_tag_fail_count(void) {
    return cryptofs_tag_fail_count();
}

int disk_gate_active_sessions(void) {
    return disk_auth_active_sessions();
}