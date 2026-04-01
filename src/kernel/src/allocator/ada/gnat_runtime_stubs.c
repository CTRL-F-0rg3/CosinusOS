/* CosinusOS — allocator/ada/gnat_runtime_stubs.c
 *
 * Minimal stubs for GNAT runtime check functions used by Ada code
 * compiled with -mcmodel=large (no full Ada runtime available in kernel).
 *
 * Each stub sends a message over debugcon (port 0xE9) then halts.
 * In a release build these can be replaced with pure HLT.
 */

static void serial_str(const char *s) {
    while (*s) {
        __asm__ volatile ("outb %0, $0xe9" :: "a"(*s));
        s++;
    }
}

__attribute__((noreturn))
static void runtime_panic(const char *msg) {
    serial_str("[GNAT RUNTIME] ");
    serial_str(msg);
    serial_str("\n");
    for (;;) __asm__ volatile ("cli; hlt");
}

/* Index out of bounds */
__attribute__((noreturn))
void __gnat_rcheck_CE_Index_Check(const char *file, int line) {
    (void)file; (void)line;
    runtime_panic("index check failed");
}

/* Integer overflow */
__attribute__((noreturn))
void __gnat_rcheck_CE_Overflow_Check(const char *file, int line) {
    (void)file; (void)line;
    runtime_panic("overflow check failed");
}

/* Range check (subtype constraint) */
__attribute__((noreturn))
void __gnat_rcheck_CE_Range_Check(const char *file, int line) {
    (void)file; (void)line;
    runtime_panic("range check failed");
}
