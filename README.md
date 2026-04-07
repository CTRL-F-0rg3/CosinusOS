
# Cosinus OS

![Cosinus OS Logo](cosinusoslogo.jpg)

<p align="center">
  <strong>Next-generation microkernel. Lightweight. Open. Secure.</strong>
</p>

<p align="center">
  <a href="https://github.com/yourusername/cosinus-os/releases"><img src="https://img.shields.io/badge/version-0.3.x--0.4.x--beta-blue" alt="Version"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-green" alt="License"></a>
  <a href="#"><img src="https://img.shields.io/badge/architecture-x86__64-orange" alt="Architecture"></a>
  <a href="#"><img src="https://img.shields.io/badge/built%20with-Zig-f69a1b" alt="Built with Zig"></a>
</p>

---

##  About

Cosinus OS is a modern, lightweight operating system built from the ground up with developers and power users in mind. At its core lies a custom microkernel designed for security, modularity, and performance. 

The system prioritizes desktop usability while maintaining tight integration with developer toolchains. A key feature is its **Linux Compatibility Layer**, enabling seamless execution of Linux applications directly within the Cosinus ecosystem.

>  **Early Beta Status**  
> Cosinus OS is currently in an early beta phase (`v0.3.x` → `v0.4.x` transitional mapping). It is fully bootable and functional, but still under active development. Recommended for testing, experimentation, and early adoption.

---

##  Core Pillars

###  Microkernel Architecture
Designed from scratch with security and isolation in mind. Every system component runs in user space, services are optional, and failures are strictly contained.

###  Linux Compatibility Layer
A native compatibility layer enabling execution of Linux ELF binaries and applications on Cosinus OS. Designed to bridge the software ecosystem gap while preserving system integrity.

###  Security-First Design
Protection is not an afterthought. Security is baked into the kernel, memory management, and service isolation from day one.

###  Hybrid Ecosystem
Freedom to modify, own tools, and direct distribution. Core system components are open, while select critical infrastructure remains proprietary to ensure security, stability, and controlled evolution.

---

##  System Requirements

| Component | Specification |
|-----------|---------------|
| **Architecture** | `x86_64` (RISC-V support planned) |
| **RAM** | Minimum: `100 MB` \| Recommended: `210 MB` |
| **Storage** | ~`500 MB` free space (ISO + runtime data) |
| **Boot** | BIOS / UEFI (tested in QEMU & bare metal) |
| **Build Tool** | Zig (`0.13+` recommended) |

---

##  Installation & Running

###  Option 1: Pre-built ISO (Recommended)
Download the latest stable ISO from [GitHub Releases](https://github.com/yourusername/cosinus-os/releases).  
It boots directly in any major virtual machine (QEMU, VirtualBox, VMware) and is also compatible with real hardware via USB/DVD.

###  Option 2: Build from Source
The project uses `zig build` as its primary build and run system. It handles compilation, asset bundling, and automatically launches a configured QEMU instance.

```bash
# Clone the repository
git clone https://github.com/yourusername/cosinus-os.git
cd cosinus-os

# Build & run in one command
zig build run
```
>  **Tip:** Ensure `qemu-system-x86_64` is installed on your host machine for the automated VM launch feature to work.

---

##  Ecosystem & Tools

The following tools are functional and integrated or pending full integration. Note that some components are intentionally kept proprietary for security and ecosystem control.

| Tool | Category | Status | Notes |
|------|----------|--------|-------|
| **Brass Engine** | Game Dev | ✅ Ready | Vulkan-optimized engine for Cosinus OS |
| **VCCat Browser** | Web | ✅ Ready | Replaces VC Browser. Security & privacy focused |
| **Grid UI** | Interface | ✅ Integrated | Tile-based desktop environment (part of main repo) |
| **Sinpr** | Core Language | 🔜 In Dev | Proprietary low-level language. Public repo planned. |
| **OrbitMesh** | Distribution | 🔒 Closed | P2P versioning/distribution system. Proprietary for security. |
| **PXD Editor** | Creative | ⚠️ Deprecated | Replaced by OrbitMesh pipeline. |

>  **Note on Licensing & Source:** Core kernel, drivers, and `Grid UI` are open. `Sinpr` and `OrbitMesh` remain closed-source to ensure security, stability, and controlled distribution. `Sinpr` will receive a public repository in a future release.

---

##  Versioning & Roadmap

Cosinus OS follows a transitional beta versioning scheme: `0.3.x.x` → `0.4.x.x`.  
This phase focuses on:
- Stabilizing the microkernel & Linux compatibility layer
- Refining `Grid UI` and core system services
- Preparing `Sinpr` compiler toolchain for public access
- Hardening `OrbitMesh` distribution pipeline

Major milestones will shift to `1.x.x` once the core ecosystem reaches production stability.

---

## 💬 Community & Support

-  **Bug Reports & Features:** [GitHub Issues](https://github.com/yourusername/cosinus-os/issues)
-  **Discussions:** Join our [Reddit Community](https://reddit.com/r/cosinusos) *(replace with actual link)*

>  **Disclaimer:** This is an independent, community-driven project. Do not use for critical production workloads yet.

---

##  License

Distributed under the [MIT License](LICENSE).  
`© 2026 Cosinus OS Project`
