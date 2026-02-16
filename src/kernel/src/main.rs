/*==========================================
Cosinus OS Microkernel Rust version 
============================================ */
// multi boot

pub const MAGIC: u64 = 0xe85250d6;
pub const ARCH_I386: u64 = 0;
pub const HEADER_TAG_END: u64 = 0;

//truct
#[repr(C)]
pub struct MultibootHeaderTag {
    pub typ: u16,
    pub flags: u16,
    pub size: u32,
}

#[repr(C)]
pub struct MultibootHeader {
    pub magic: u32,
    pub architecture: u32,
    pub header_length: u32,
    pub checksum: u32,
}

#[repr(C)]
pub struct MultibootHeaderRaw {
    pub header: MultibootHeader,
    pub end_tag: MultibootHeaderTag,
}
#[no_mangle]
#[link_section = ".multiboot"]
#[used]
pub static MULTIBOOT_HEADER: MultibootHeaderRaw = {
    
    const HEADER: MultibootHeader = MultibootHeader {
        magic: 0xe85250d6, // MULTIBOOT2_MAGIC
        architecture: 0,    // MULTIBOOT_ARCH_I386
        header_length: size_of::<MultibootHeaderRaw>() as u32, // sizeof(multiboot_header)
        checksum: {
          
            let sum = 0xe85250d6 + 0 + size_of::<MultibootHeaderRaw>() as u64;
            sum.wrapping_neg() as u32
        },
    };

    const END_TAG: MultibootHeaderTag = MultibootHeaderTag {
        typ: 0, 
        flags: 0,
        size: size_of::<MultibootHeaderTag>() as u32, // 8
    };

    
    MultibootHeaderRaw {
        header: HEADER,
        end_tag: END_TAG,
    }
};
//TODO reszta tłumaczenia jądra i drobne poprawki 