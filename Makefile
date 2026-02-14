CC = x86_64-elf-gcc
LD = x86_64-elf-ld

CFLAGS = -ffreestanding -m64 -O2 -Wall -Wextra
LDFLAGS = -T linker.ld

SRC = src/kernel.c
OBJ = build/kernel.o
KERNEL = build/kernel.elf

ISO_DIR = iso
ISO_BOOT = $(ISO_DIR)/boot
ISO_NAME = CosinusOS.iso

LIMINE_DIR = limine

all: iso

build:
	mkdir -p build

$(OBJ): $(SRC) | build
	$(CC) $(CFLAGS) -c $(SRC) -o $(OBJ)

$(KERNEL): $(OBJ)
	$(LD) $(LDFLAGS) $(OBJ) -o $(KERNEL)

iso: $(KERNEL)
	mkdir -p $(ISO_BOOT)

	cp $(KERNEL) $(ISO_BOOT)/kernel.elf
	cp $(LIMINE_DIR)/limine.sys $(ISO_BOOT)/
	cp $(LIMINE_DIR)/limine-cd.bin $(ISO_BOOT)/
	cp $(LIMINE_DIR)/limine-efi-x86_64.efi $(ISO_BOOT)/
	cp limine.cfg $(ISO_BOOT)/

	xorriso -as mkisofs \
	-b boot/limine-cd.bin \
	-no-emul-boot \
	-boot-load-size 4 \
	-boot-info-table \
	-o $(ISO_NAME) $(ISO_DIR)

run: iso
	qemu-system-x86_64 -cdrom $(ISO_NAME)

debug: iso
	qemu-system-x86_64 -cdrom $(ISO_NAME) -s -S

clean:
	rm -rf build iso $(ISO_NAME)