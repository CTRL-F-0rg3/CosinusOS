![Logo Cosinus OS](cosinusoslogo.jpg)

# Cosinus OS

**Next-generation microkernel. Lightweight. Open. Secure.**

---

## About the Project

Cosinus OS is a modern operating system designed from the ground up with developers in mind. It combines the lightweight nature of a microkernel, an open ecosystem, and an innovative approach to security.

The system is in the early development phase (Early Alpha) and has been actively developed for three months.

---

## Core Pillars

### Microkernel
Designed from scratch with security and modularity in mind. Every component is isolated, every service is optional.

### OrbitMesh (Planned)
A revolutionary software versioning and distribution system. Easier than Git, faster than traditional CDNs. Peer-to-peer architecture based on BitTorrent concepts.

### Security
A new generation of protection, designed from zero, not as an add-on.

### Open Ecosystem
Freedom to modify, own tools, direct distribution without intermediaries.

---

## Sinpr Programming Language

Sinpr is the proprietary programming language of Cosinus OS.

- Independent language, not a wrapper over Rust/C++
- Optimized for the creative process within Cosinus OS
- Combines low-level control with high-level ergonomics
- Full support for Unix executable files
- Status: Functional, currently being integrated with the system

---

## Cosinus OS Ecosystem

The following tools are already functional and awaiting integration with the system:

| Tool | Category | Description | Status |
|------|----------|-------------|--------|
| Brass Engine | Game Dev | Game engine optimized for Cosinus OS. Full support for Vulkan and modern rendering pipelines | Ready |
| PXD Editor | Creative | Graphic editor supporting raster and vector on a single canvas. Node system for advanced graphic operations | Ready |
| VC Browser | Web | Proprietary web browser optimized for security and privacy within the Cosinus ecosystem | Ready |
| Grid UI | Interface | Innovative interface based on tiles instead of windows. Virtual desktops in gallery form | Ready |
| Sinpr | Core | Low-level programming language focused on optimizing the creative process | Ready |

---

## System Requirements

- **Architecture**: x86_64
- **RAM**: Minimum 100 MB
- **Disk**: Approximately 500 MB free space (ISO + data)
- **Boot**: BIOS/UEFI (tested in QEMU)

---

## Installation and Running

> Warning: Early Alpha – project in active development. For testing purposes only.

### Option 1: QEMU (Recommended for testing)

```bash
# Download the latest ISO from Releases
wget [LINK_TO_ISO]

# Run in QEMU
qemu-system-x86_64 \
  -m 256 \
  -hda cosinus-os.iso \
  -boot d \
  -enable-kvm \
  -cpu host
=======

