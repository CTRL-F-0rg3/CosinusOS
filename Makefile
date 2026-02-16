# ==============================
# CosinusOS Makefile (GRUB + Mikrojądro)
# ==============================

BUILD_DIR = build
ISO_DIR = iso
GRUB_DIR = $(ISO_DIR)/boot/grub

# Kernel
KERNEL = $(BUILD_DIR)/kernel.elf
BOOT_OBJ = $(BUILD_DIR)/boot.o
KERNEL_C_OBJ = $(BUILD_DIR)/kernel.o
USERSPACE_OBJ = $(BUILD_DIR)/userspace.o
LINKER = linker.ld

# Userspace - SPRAWDŹ CO GENERUJE CARGO!
USERSPACE_DIR = src/userspace
# Cargo robi 'userspace' (ELF), musisz go przekonwertować na .bin
USERSPACE_ELF = $(USERSPACE_DIR)/target/x86_64-unknown-none/release/userspace
USERSPACE_BIN = $(BUILD_DIR)/userspace_raw.bin

# Narzędzia
NASM = nasm
LD = x86_64-elf-ld
CC = x86_64-elf-gcc
OBJCOPY = x86_64-elf-objcopy
CARGO = cargo
GRUB_MKRESCUE = grub-mkrescue
QEMU = qemu-system-x86_64

CFLAGS = -m64 -ffreestanding -O2 -nostdlib -fno-stack-protector -fno-pic -mno-red-zone -Wall -Wextra

all: iso

# ==============================
# Bootloader (ASM)
# ==============================

$(BOOT_OBJ): boot.asm
	@echo "==> Kompiluję bootloader..."
	mkdir -p $(BUILD_DIR)
	$(NASM) -f elf64 boot.asm -o $(BOOT_OBJ)

# ==============================
# Kernel (C) - TERAZ WŁĄCZONE!
# ==============================

$(KERNEL_C_OBJ): src/kernel.c
	@echo "==> Kompiluję kernel (C)..."
	mkdir -p $(BUILD_DIR)
	$(CC) $(CFLAGS) -c src/kernel.c -o $(KERNEL_C_OBJ)

# ==============================
# Userspace (Rust) -> ELF -> BIN -> OBIEKT
# ==============================

# Krok 1: Rust buduje ELF
$(USERSPACE_ELF):
	@echo "==> Buduję userspace (Rust)..."
	cd $(USERSPACE_DIR) && $(CARGO) build --release

# Krok 2: ELF -> BIN (surowe bajty)
$(USERSPACE_BIN): $(USERSPACE_ELF)
	@echo "==> Ekstrahuję surowy binar z ELF..."
	mkdir -p $(BUILD_DIR)
	$(OBJCOPY) -O binary $(USERSPACE_ELF) $(USERSPACE_BIN)

# Krok 3: BIN -> OBIEKT ELF z symbolami
$(USERSPACE_OBJ): $(USERSPACE_BIN)
	@echo "==> Konwertuję bin na obiekt linkowalny..."
	$(OBJCOPY) -I binary -O elf64-x86-64 -B i386:x86-64 \
		--rename-section .data=.userspace \
		$(USERSPACE_BIN) $(USERSPACE_OBJ)

# ==============================
# Linkowanie - WSZYSTKIE ZALEŻNOŚCI
# ==============================

$(KERNEL): $(BOOT_OBJ) $(KERNEL_C_OBJ) $(USERSPACE_OBJ)
	@echo "==> Linkuję kernel..."
	$(LD) -n -T $(LINKER) \
		$(BOOT_OBJ) \
		$(KERNEL_C_OBJ) \
		$(USERSPACE_OBJ) \
		-o $(KERNEL)

# ==============================
# ISO
# ==============================

iso: $(KERNEL)
	@echo "==> Tworzę ISO..."
	mkdir -p $(GRUB_DIR)
	cp $(KERNEL) $(ISO_DIR)/boot/kernel.elf

	echo 'set timeout=0' > $(GRUB_DIR)/grub.cfg
	echo 'set default=0' >> $(GRUB_DIR)/grub.cfg
	echo 'menuentry "CosinusOS" {' >> $(GRUB_DIR)/grub.cfg
	echo '  multiboot2 /boot/kernel.elf' >> $(GRUB_DIR)/grub.cfg
	echo '  boot' >> $(GRUB_DIR)/grub.cfg
	echo '}' >> $(GRUB_DIR)/grub.cfg

	$(GRUB_MKRESCUE) -o $(BUILD_DIR)/cosinusos.iso $(ISO_DIR)

# ==============================
# Uruchamianie
# ==============================

run: iso
	@echo "==> Uruchamiam w QEMU..."
	$(QEMU) -cdrom $(BUILD_DIR)/cosinusos.iso -m 512M -serial stdio

clean:
	@echo "==> Czyszczenie..."
	rm -rf $(BUILD_DIR)/*
	rm -rf $(ISO_DIR)/boot/kernel.elf
	rm -rf $(GRUB_DIR)/grub.cfg
	cd $(USERSPACE_DIR) && cargo clean

.PHONY: all iso run clean