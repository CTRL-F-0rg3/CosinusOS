// CosinusOS — userspace_loader.rs
// Multiboot2 module parser + ELF64/flat binary loader

use crate::mm::{PhysAddr, VirtAddr, PAGE_SIZE, PTE_W, PTE_U, mm_alloc, vmap, new_user_p4};
use crate::debug::{col, print, printc, num_str, hex_str};
use crate::threading::spawn_user_on_cr3;

pub static mut US_ENTRY: VirtAddr = 0;

// ── Multiboot2 structs ────────────────────────────────────────────────────────
#[repr(C, packed)] struct Mb2Hdr { total: u32, _res: u32 }
#[repr(C, packed)] struct Mb2Tag { typ: u32, sz: u32 }
#[repr(C, packed)] struct Mb2Mod { typ: u32, sz: u32, start: u32, end: u32 }

pub const MB2_OK: u64 = 0x36d76289;

pub unsafe fn mb2_module(info: u64) -> Option<(u64, u64)> {
    if info == 0 { return None; }
    let total = (*(info as *const Mb2Hdr)).total as u64;
    let mut off = 8u64;
    while off < total {
        let tag = &*((info + off) as *const Mb2Tag);
        if tag.typ == 0 { break; }
        if tag.typ == 3 {
            let m = &*((info + off) as *const Mb2Mod);
            return Some((m.start as u64, m.end as u64));
        }
        off += (tag.sz as u64 + 7) & !7;
    }
    None
}

// ── Główny loader ─────────────────────────────────────────────────────────────
pub unsafe fn load_userspace(mod_start: u64, mod_end: u64) -> bool {
    if mod_end <= mod_start { return false; }
    let mod_sz = (mod_end - mod_start) as usize;
    let elf    = mod_start as *const u8;
    let magic  = *(elf as *const u32);

    if magic != 0x464C457F {
        load_flat(elf, mod_sz, mod_start)
    } else {
        load_elf64(elf, mod_sz)
    }
}

// ── Flat binary ──────────────────────────────────────────────────────────────
unsafe fn load_flat(src: *const u8, size: usize, mod_start: u64) -> bool {
    printc("[US] Raw binary\n", col::LCYAN);
    let cr3 = new_user_p4();
    const BIN_BASE: u64 = 0x0040_0000;
    let pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
    for i in 0..pages {
        let phys = mm_alloc();
        vmap(cr3, BIN_BASE + i as u64 * PAGE_SIZE as u64, phys, PTE_W | PTE_U);
        let dst = phys as *mut u8;
        let n   = core::cmp::min(PAGE_SIZE, size - i * PAGE_SIZE);
        core::ptr::copy_nonoverlapping(src.add(i * PAGE_SIZE), dst, n);
        if n < PAGE_SIZE { core::ptr::write_bytes(dst.add(n), 0, PAGE_SIZE - n); }
    }
    US_ENTRY = BIN_BASE;
    printc("[US] Flat binary @ ", col::LCYAN);
    { let mut b = [0u8; 18]; print(hex_str(BIN_BASE, &mut b)); }
    print("\n");
    spawn_and_report("userspace", BIN_BASE, 0, cr3)
}

// ── ELF64 loader ─────────────────────────────────────────────────────────────
unsafe fn load_elf64(elf: *const u8, _sz: usize) -> bool {
    let e_type      = *(elf.add(0x10) as *const u16);
    let e_entry_raw = *(elf.add(0x18) as *const u64);
    let e_phoff     = *(elf.add(0x20) as *const u64);
    let e_phentsize = *(elf.add(0x36) as *const u16) as usize;
    let e_phnum     = *(elf.add(0x38) as *const u16) as usize;

    // ET_DYN (PIE) → ładuj od 0x400000; ET_EXEC → zachowaj oryginalne adresy
    let load_base: u64 = if e_type == 3 { 0x0040_0000 } else { 0 };
    let e_entry = load_base + e_entry_raw;

    printc("[US] ELF64 ", col::LCYAN);
    if e_type == 2 { print("ET_EXEC"); } else { print("ET_DYN"); }
    print(" entry=");
    { let mut b = [0u8; 18]; print(hex_str(e_entry, &mut b)); }
    print(" phnum=");
    { let mut nb = [0u8; 24]; print(num_str(e_phnum, &mut nb)); }
    print("\n");

    let cr3 = new_user_p4();

    for i in 0..e_phnum {
        let ph      = elf.add(e_phoff as usize + i * e_phentsize);
        let p_type  = *(ph as *const u32);
        let p_flags = *(ph.add(0x04) as *const u32);

        // Loguj każdy segment na serial dla debugowania
        crate::debug::serial_print("[ELF] ph[");
        { let mut b=[0u8;24]; crate::debug::serial_print(num_str(i, &mut b)); }
        crate::debug::serial_print("] type=");
        { let mut b=[0u8;18]; crate::debug::serial_print(hex_str(p_type as u64, &mut b)); }
        crate::debug::serial_print(" flags=");
        { let mut b=[0u8;18]; crate::debug::serial_print(hex_str(p_flags as u64, &mut b)); }
        crate::debug::serial_print("\n");

        if p_type != 1 { continue; } // PT_LOAD = 1

        let p_offset = *(ph.add(0x08) as *const u64);
        let p_vaddr  = *(ph.add(0x10) as *const u64);
        let p_filesz = *(ph.add(0x20) as *const u64);
        let p_memsz  = *(ph.add(0x28) as *const u64);

        // Loguj PT_LOAD szczegółowo
        crate::debug::serial_print("[ELF] PT_LOAD vaddr=");
        { let mut b=[0u8;18]; crate::debug::serial_print(hex_str(p_vaddr, &mut b)); }
        crate::debug::serial_print(" filesz=");
        { let mut b=[0u8;24]; crate::debug::serial_print(num_str(p_filesz as usize, &mut b)); }
        crate::debug::serial_print(" memsz=");
        { let mut b=[0u8;24]; crate::debug::serial_print(num_str(p_memsz as usize, &mut b)); }
        crate::debug::serial_print("\n");

        if p_memsz == 0 { continue; }

        // Ogranicz rozmiar segmentu — max 8MB na segment
        let p_memsz_clamped = core::cmp::min(p_memsz, 8 * 1024 * 1024);

        // Upewnij się że vaddr jest w bezpiecznym zakresie userspace
        // (powyżej 0x1000, poniżej 0x8000_0000)
        let effective_vaddr = load_base + p_vaddr;
        if effective_vaddr < 0x1000 {
            crate::debug::serial_print("[ELF] SKIP: vaddr too low\n");
            continue;
        }
        if effective_vaddr >= 0x8000_0000 {
            crate::debug::serial_print("[ELF] SKIP: vaddr too high\n");
            continue;
        }

        // Uprawnienia: zawsze PTE_U, W tylko gdy p_flags ma bit W (0x2)
        let mut perm = PTE_U;
        if p_flags & 0x2 != 0 { perm |= PTE_W; }

        let seg_start = effective_vaddr & !(PAGE_SIZE as u64 - 1);
        let seg_end   = (effective_vaddr + p_memsz_clamped + PAGE_SIZE as u64 - 1)
                        & !(PAGE_SIZE as u64 - 1);
        let pages     = ((seg_end - seg_start) / PAGE_SIZE as u64) as usize;

        crate::debug::serial_print("[ELF] mapping ");
        { let mut b=[0u8;24]; crate::debug::serial_print(num_str(pages, &mut b)); }
        crate::debug::serial_print(" pages at ");
        { let mut b=[0u8;18]; crate::debug::serial_print(hex_str(seg_start, &mut b)); }
        crate::debug::serial_print("\n");

        let mut vaddr = seg_start;
        while vaddr < seg_end {
            let phys = mm_alloc();
            if phys == 0 {
                crate::debug::serial_print("[ELF] OOM at vaddr=");
                { let mut b=[0u8;18]; crate::debug::serial_print(hex_str(vaddr, &mut b)); }
                crate::debug::serial_print("\n");
                return false;
            }

            // Wyczyść stronę
            core::ptr::write_bytes(phys as *mut u8, 0, PAGE_SIZE);
            vmap(cr3, vaddr, phys, perm);

            // Kopiuj dane pliku na tę stronę
            // Zakres pliku: [p_offset .. p_offset + p_filesz]
            // Zakres wirt:  [effective_vaddr .. effective_vaddr + p_filesz]
            // Dla strony vaddr:
            //   strona pokrywa [vaddr, vaddr + PAGE_SIZE)
            //   dane pliku:    [effective_vaddr + page_off_in_seg, ...)
            if p_filesz > 0 {
                // Offset danych segmentu w ramach tej strony
                let page_vaddr_end = vaddr + PAGE_SIZE as u64;
                let seg_data_start = effective_vaddr;
                let seg_data_end   = effective_vaddr + p_filesz;

                // Czy ta strona nakłada się z danymi pliku?
                if vaddr < seg_data_end && page_vaddr_end > seg_data_start {
                    // Zakres kopiowania w tej stronie
                    let copy_vstart = core::cmp::max(vaddr, seg_data_start);
                    let copy_vend   = core::cmp::min(page_vaddr_end, seg_data_end);
                    let copy_len    = (copy_vend - copy_vstart) as usize;

                    // Offset w pliku
                    let file_offset = p_offset + (copy_vstart - seg_data_start);
                    // Offset w stronie fizycznej
                    let dst_offset  = (copy_vstart - vaddr) as usize;

                    let src = elf.add(file_offset as usize);
                    let dst = (phys as *mut u8).add(dst_offset);
                    core::ptr::copy_nonoverlapping(src, dst, copy_len);
                }
            }

            vaddr += PAGE_SIZE as u64;
        }

        // Log na VGA
        print("  [SEG] vaddr=");
        { let mut b = [0u8; 18]; print(hex_str(effective_vaddr, &mut b)); }
        print(" filesz="); { let mut b=[0u8;24]; print(num_str(p_filesz as usize, &mut b)); }
        print(" memsz=");  { let mut b=[0u8;24]; print(num_str(p_memsz_clamped as usize, &mut b)); }
        print("\n");
    }

    crate::debug::serial_print("[ELF] all segments loaded, spawning\n");
    US_ENTRY = e_entry;
    spawn_and_report("userspace", e_entry, 0, cr3)
}

// ── Helper: spawn + log ───────────────────────────────────────────────────────
unsafe fn spawn_and_report(name: &str, entry: u64, arg: u64, cr3: PhysAddr) -> bool {
    let tid = spawn_user_on_cr3(name, entry, arg, cr3);
    if tid >= 0 {
        printc("[US] Watek #", col::LGREEN);
        { let mut b = [0u8; 24]; print(num_str(tid as usize, &mut b)); }
        print(" OK\n");
        true
    } else {
        printc("[US] Brak slotow!\n", col::LRED);
        false
    }
}
// ── Embedded userspace blob ───────────────────────────────────────────────────
unsafe extern "C" {
    static _userspace_blob_start: u8;
    static _userspace_blob_end:   u8;
}

pub unsafe fn load_embedded() -> bool {
    let start = core::ptr::addr_of!(_userspace_blob_start) as u64;
    let end   = core::ptr::addr_of!(_userspace_blob_end)   as u64;

    if start == 0 || end <= start || (end - start) < 4 {
        crate::debug::serial_print("[EMB] brak embedded blob\n");
        return false;
    }

    crate::debug::serial_print("[EMB] blob @ ");
    { let mut b=[0u8;18]; crate::debug::serial_print(hex_str(start, &mut b)); }
    crate::debug::serial_print(" size=");
    { let mut b=[0u8;24]; crate::debug::serial_print(num_str((end-start) as usize, &mut b)); }
    crate::debug::serial_print("\n");

    load_userspace(start, end)
}