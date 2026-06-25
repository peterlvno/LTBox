//! Pure-Rust port of the SKRoot (Lite) ARM64 Linux kernel root patcher.
//!
//! SKRoot patches the **kernel binary** (the image extracted from `boot.img`)
//! directly — locating functions from the embedded kallsyms with no kernel
//! source, symbol table, or rebuild — a different mechanism from the existing
//! root providers. The port is landing bottom-up: the core analysis layers,
//! patch-emission helpers, the `do_execve` root hook, directory hiding through
//! `filldir64`, SELinux `avc_denied` / `audit_log_start` hooks, and root
//! pipeline wiring are present.
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
pub mod init_cred;
pub mod insn;
pub mod kallsyms;
pub mod offsets;
pub mod patch_audit_log_start;
pub mod patch_avc_denied;
pub mod patch_base;
pub mod patch_bytes;
pub mod patch_current_avc_check;
pub mod patch_do_execve;
pub mod patch_filldir64;
pub mod patch_plan;
pub mod symbol_analyze;
pub mod version;
