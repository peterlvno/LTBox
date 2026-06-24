//! Pure-Rust port of the SKRoot (Lite) ARM64 Linux kernel root patcher.
//!
//! SKRoot patches the **kernel binary** (the image extracted from `boot.img`)
//! directly — locating functions from the embedded kallsyms with no kernel
//! source, symbol table, or rebuild — a different mechanism from the existing
//! root providers. It assembles and writes a `do_execve` hook plus SELinux /
//! audit / `filldir64` patches into spare kernel regions; a process that
//! `execve`s the embedded 48-character root key is then granted full
//! capabilities.
//!
//! Ported from `abcz316/SKRoot-linuxKernelRoot`
//! (`Lite_version/src/patch_kernel_root`), C++ → safe Rust with no C
//! dependencies: upstream's "capstone" use is really just raw AArch64 bitmask
//! checks, and asmjit is replaced by a small fixed-form instruction encoder.
//!
//! Layered bottom-up: [`insn`] (instruction predicates) → `asm` (encoder) →
//! `version` (kernel version) → kallsyms → symbols → offsets → patches.

pub mod asm;
pub mod func_end;
pub mod insn;
pub mod kallsyms;
pub mod offsets;
pub mod symbol_analyze;
pub mod version;
