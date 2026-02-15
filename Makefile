# ===================================================
# Makefile CosinusOS - Kernel C + Userspace Rust
# ===================================================

CC = x86_64-elf-gcc
LD = x86_64-elf-ld
ASM = nasm
QEMU = qemu-system-x86_64
CARGO = cargo
OBJCOPY = objcopy

CFLAGS = -ffreestanding -m64 -O2 -Wall -Wextra -mno-red-zone \
         -mcmodel=kernel -fno-pie -fno-stack-protector \
         -Wno-unused-parameter -I./src -std=gnu11

LDFLAGS = -T linker.ld -nostdlib -z max-page-size=0x1000

# Directories
SRC_DIR = src
BUILD_DIR = build
USERSPACE_DIR = $(SRC_DIR)/userspace

# Boot
BOOT_ASM = $(SRC_DIR)/boot.asm
BOOT_BIN = $(BUILD_DIR)/boot.bin

# Kernel
KERNEL_C = $(SRC_DIR)/kernel.c
KERNEL_OBJ = $(BUILD_DIR)/kernel.o

# Userspace
USERSPACE_TARGET = x86_64-unknown-none
USERSPACE_ELF = $(USERSPACE_DIR)/target/$(USERSPACE_TARGET)/release/userspace
USERSPACE_BIN = $(BUILD_DIR)/userspace.bin
USERSPACE_OBJ = $(BUILD_DIR)/userspace.o

# Final output
KERNEL_ELF = $(BUILD_DIR)/kernel.elf
KERNEL_BIN = $(BUILD_DIR)/kernel.bin
OS_IMG = $(BUILD_DIR)/cosinusos.img

# ===================================================
# Main targets
# ===================================================

all: dirs $(OS_IMG)

dirs:
	mkdir -p $(BUILD_DIR)
	mkdir -p $(USERSPACE_DIR)/Aplication
	mkdir -p $(USERSPACE_DIR)/disk
	mkdir -p $(USERSPACE_DIR)/terminal

# ===================================================
# Bootloader
# ===================================================

$(BOOT_BIN): $(BOOT_ASM)
	$(ASM) -f bin $(BOOT_ASM) -o $(BOOT_BIN)

# ===================================================
# Kernel (C)
# ===================================================

$(KERNEL_OBJ): $(KERNEL_C)
	$(CC) $(CFLAGS) -c $(KERNEL_C) -o $(KERNEL_OBJ)

# ===================================================
# Userspace (Rust)
# ===================================================

# Target spec dla bare metal x86_64
$(BUILD_DIR)/x86_64-unknown-none.json:
	@echo '{'                                                          > $@
	@echo '  "llvm-target": "x86_64-unknown-none",'                  >> $@
	@echo '  "data-layout": "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-f80:128-n8:16:32:64-S128",' >> $@
	@echo '  "arch": "x86_64",'                                       >> $@
	@echo '  "target-endian": "little",'                             >> $@
	@echo '  "target-pointer-width": "64",'                          >> $@
	@echo '  "target-c-int-width": "32",'                            >> $@
	@echo '  "os": "none",'                                           >> $@
	@echo '  "executables": true,'                                    >> $@
	@echo '  "linker-flavor": "ld.lld",'                             >> $@
	@echo '  "linker": "rust-lld",'                                   >> $@
	@echo '  "panic-strategy": "abort",'                             >> $@
	@echo '  "disable-redzone": true,'                               >> $@
	@echo '  "features": "-mmx,-sse,+soft-float",'                   >> $@
	@echo '  "code-model": "kernel"'                                  >> $@
	@echo '}'                                                         >> $@

# Kompilacja Rust userspace
$(USERSPACE_ELF): $(BUILD_DIR)/x86_64-unknown-none.json
	@echo "==> Compiling Rust userspace..."
	cd $(USERSPACE_DIR) && \
	$(CARGO) rustc --release \
		--target $(USERSPACE_TARGET) \
		-Z build-std=core,alloc \
		-Z build-std-features=compiler-builtins-mem \
		-- -C relocation-model=static

# Konwersja ELF -> raw binary
$(USERSPACE_BIN): $(USERSPACE_ELF)
	@echo "==> Converting userspace to binary..."
	$(OBJCOPY) -O binary $< $@

# Embeddowanie userspace jako obiekt
$(USERSPACE_OBJ): $(USERSPACE_BIN)
	@echo "==> Embedding userspace into kernel..."
	$(OBJCOPY) -I binary -O elf64-x86-64 -B i386:x86-64 \
		--rename-section .data=.userspace \
		--redefine-sym _binary_$(subst /,_,$(subst .,_,$(USERSPACE_BIN)))_start=_binary_userspace_bin_start \
		--redefine-sym _binary_$(subst /,_,$(subst .,_,$(USERSPACE_BIN)))_end=_binary_userspace_bin_end \
		$< $@

# ===================================================
# Link kernel + userspace
# ===================================================

$(KERNEL_ELF): $(KERNEL_OBJ) $(USERSPACE_OBJ)
	@echo "==> Linking kernel with embedded userspace..."
	$(LD) $(LDFLAGS) $(KERNEL_OBJ) $(USERSPACE_OBJ) -o $@

$(KERNEL_BIN): $(KERNEL_ELF)
	$(OBJCOPY) -O binary $< $@

# ===================================================
# Create disk image
# ===================================================

$(OS_IMG): $(BOOT_BIN) $(KERNEL_BIN)
	@echo "==> Creating bootable image..."
	cat $(BOOT_BIN) $(KERNEL_BIN) > $(OS_IMG)
	truncate -s 1440K $(OS_IMG)

# ===================================================
# Run targets
# ===================================================

run: all
	$(QEMU) -drive format=raw,file=$(OS_IMG) -m 128M -serial stdio

run-nokvm: all
	$(QEMU) -drive format=raw,file=$(OS_IMG) -m 128M -smp 2 -serial stdio -no-kvm

debug: all
	$(QEMU) -drive format=raw,file=$(OS_IMG) -m 128M -serial stdio -s -S

# ===================================================
# Info & testing
# ===================================================

info: all
	@echo "=== CosinusOS Build Info ==="
	@echo "Boot sector size:  $$(stat -c%s $(BOOT_BIN)) bytes"
	@echo "Kernel size:       $$(stat -c%s $(KERNEL_BIN)) bytes"
	@echo "Userspace size:    $$(stat -c%s $(USERSPACE_BIN)) bytes"
	@echo "Total image size:  $$(stat -c%s $(OS_IMG)) bytes"
	@echo ""
	@echo "=== Kernel ELF sections ==="
	@objdump -h $(KERNEL_ELF) | grep -E "\.text|\.data|\.bss|\.userspace"
	@echo ""
	@echo "=== Userspace symbols ==="
	@nm $(KERNEL_ELF) | grep userspace_bin

# Test tylko userspace (bez kernela)
test-userspace:
	@echo "==> Testing userspace compilation..."
	cd $(USERSPACE_DIR) && $(CARGO) check

# ===================================================
# Clean targets
# ===================================================

clean:
	rm -rf $(BUILD_DIR)/*
	cd $(USERSPACE_DIR) && $(CARGO) clean

clean-userspace:
	cd $(USERSPACE_DIR) && $(CARGO) clean
	rm -f $(USERSPACE_BIN) $(USERSPACE_OBJ)

# ===================================================
# Help
# ===================================================

help:
	@echo "CosinusOS Makefile"
	@echo "=================="
	@echo ""
	@echo "Targets:"
	@echo "  all              - Build everything (default)"
	@echo "  run              - Build and run in QEMU"
	@echo "  run-nokvm        - Run without KVM acceleration"
	@echo "  debug            - Run with GDB server (port 1234)"
	@echo "  info             - Show build information"
	@echo "  test-userspace   - Test Rust compilation only"
	@echo "  clean            - Remove all build artifacts"
	@echo "  clean-userspace  - Clean only Rust artifacts"
	@echo "  help             - Show this help"
	@echo ""
	@echo "Requirements:"
	@echo "  - x86_64-elf-gcc (cross-compiler)"
	@echo "  - nasm (assembler)"
	@echo "  - Rust nightly (with rust-src)"
	@echo "  - qemu-system-x86_64"

.PHONY: all clean clean-userspace dirs run run-nokvm debug info test-userspace help