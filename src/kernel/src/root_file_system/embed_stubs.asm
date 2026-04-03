section .data

global _binary_kernel_elf_start
global _binary_kernel_elf_end
global _binary_kernel_elf_size
global _binary_devspace_elf_start
global _binary_devspace_elf_end
global _binary_devspace_elf_size
global _binary_fs_server_bin_start
global _binary_fs_server_bin_end
global _binary_fs_server_bin_size
global _binary_userspace_bin_start
global _binary_userspace_bin_end
global _binary_userspace_bin_size

_binary_kernel_elf_start:    db 0
_binary_kernel_elf_end:      db 0
_binary_kernel_elf_size:     dq 0
_binary_devspace_elf_start:  db 0
_binary_devspace_elf_end:    db 0
_binary_devspace_elf_size:   dq 0
_binary_fs_server_bin_start: db 0
_binary_fs_server_bin_end:   db 0
_binary_fs_server_bin_size:  dq 0
_binary_userspace_bin_start: db 0
_binary_userspace_bin_end:   db 0
_binary_userspace_bin_size:  dq 0