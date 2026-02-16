/*==========================================
Cosinus OS Microkernel Rust version 
============================================ */
// multi boot

pub const MAGIC: u32 = 0xe85250d6;
pub const ARCH_I386: u32 = 0;
pub const HEADER_TAG_END: u32 = 0;

struct multiboot_header {
    uint32_t magic;
    uint32_t architecture;
    uint32_t header_length;
    uint32_t checksum;
} __attribute__((packed));