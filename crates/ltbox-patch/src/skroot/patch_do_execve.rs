//! The `do_execve` root hook — port of upstream `patch_do_execve.{h,cpp}`.
//!
//! The heart of SKRoot: a stub planted in a spare region that intercepts
//! `execve`. When the execed pathname equals the embedded 48-byte ROOT_KEY, the
//! stub rewrites `current`'s credentials to a full root set, clears the seccomp
//! state, and falls through; otherwise it jumps straight back. The original
//! instruction overwritten by the entry branch is preserved inside the stub.
#![allow(dead_code)]

use super::asm::{Asm, Cond, ZR};
use super::patch_base::{CurrentMode, PatchBase};
use super::patch_bytes::PatchBytes;
use super::symbol_analyze::{KernelSymbolOffset, SymbolRegion};

/// `ROOT_KEY_LEN` — the embedded key placeholder size.
const ROOT_KEY_LEN: usize = 48;
/// `-MAX_ERRNO` magnitude, for the `IS_ERR(filename)` guard.
const MAX_ERRNO: u64 = 4095;
/// `TIF_SECCOMP` bit in `thread_info.flags`.
const TIF_SECCOMP: u32 = 11;
/// `offsetof(struct thread_info, flags)` — flags is the first field.
const THREAD_INFO_FLAGS: u32 = 0;
/// `THREAD_SIZE` (16 KiB).
const THREAD_SIZE: u64 = 0x4000;

/// Which execve entry point is hooked and where its `filename` argument lives.
#[derive(Debug, Clone, Copy)]
struct ExecveParam {
    /// File offset of the hooked instruction (already past any PAC/BTI entry).
    do_execve_addr: u64,
    /// Register holding `filename` (`x0` or `x1`).
    filename_reg: u32,
    /// `true` when that register is the string pointer itself, `false` when it
    /// points to a `struct filename` whose first field is the pointer.
    is_single_char_ptr: bool,
}

pub struct PatchDoExecve {
    param: ExecveParam,
}

impl PatchDoExecve {
    /// Pick the hooked execve function (largest available variant) for this
    /// kernel. Returns `None` if no usable execve symbol was resolved.
    pub fn new(
        base: &PatchBase,
        sym: &KernelSymbolOffset,
        version: (u32, u32, u32),
    ) -> Option<PatchDoExecve> {
        // (region, filename_reg, is_single_char_ptr), tried for the largest —
        // the upstream candidate list. `do_execve_common` is the lowest-priority
        // candidate with reg x0: like `do_execve` it takes `struct filename*` in
        // x0, so reading `[x0]` is correct. It only wins if it is the largest
        // region, and the `is_complete` gate already guarantees a primary execve
        // entry (do_execve/at/at_common) exists.
        let candidates: Vec<(SymbolRegion, u32, bool)> = if version < (3, 14, 0) {
            vec![(sym.do_execve_common, 0, true), (sym.do_execve, 0, true)]
        } else {
            vec![
                (sym.do_execve_file, 1, false),
                (sym.do_execveat_common, 1, false),
                (sym.do_execve, 0, false),
                (sym.do_execveat, 1, false),
                (sym.do_execve_common, 0, false),
            ]
        };

        let mut best: Option<(SymbolRegion, u32, bool)> = None;
        for (region, reg, single) in candidates {
            if !region.valid() || region.size == 0 {
                continue;
            }
            let better = match best {
                None => true,
                Some((b, _, _)) => region.size > b.size,
            };
            if better {
                best = Some((region, reg, single));
            }
        }

        let (region, filename_reg, is_single_char_ptr) = best?;
        let do_execve_addr = base.skip_pac_bti_at_func_start(region.offset);
        Some(PatchDoExecve {
            param: ExecveParam {
                do_execve_addr,
                filename_reg,
                is_single_char_ptr,
            },
        })
    }

    /// File offset of the hooked instruction (for the orchestration's jump).
    pub fn do_execve_addr(&self) -> u64 {
        self.param.do_execve_addr
    }

    /// Assemble and place the hook stub in `region`. Returns the stub size in
    /// bytes, or 0 if it does not fit / could not be built. Also emits the entry
    /// branch from `do_execve_addr` into the stub.
    pub fn patch_do_execve(
        &self,
        base: &mut PatchBase,
        region: &SymbolRegion,
        cred_offset: u64,
        seccomp_offset: u64,
        out: &mut Vec<PatchBytes>,
    ) -> usize {
        let hook_addr = region.offset;
        if hook_addr == 0 {
            return 0;
        }
        if base.is_huawei() {
            base.set_kti_calc_base(hook_addr);
        }

        let ic = base.init_cred().clone();
        let cap_cnt = ic.cap_cnt;
        let p = self.param;
        let jump_back = p.do_execve_addr + 4;

        let mut a = Asm::new();
        let end = a.new_label();
        let cycle = a.new_label();

        // 0..48: ROOT_KEY placeholder. 48..52: the original instruction slot
        // (a `mov x0, x0` here, overwritten with the real bytes after assembly).
        a.embed(&[0u8; ROOT_KEY_LEN]);
        a.mov_reg_x(0, 0);

        // IS_ERR(filename) guard.
        a.mov_imm_x_asmjit(11, MAX_ERRNO.wrapping_neg());
        a.cmp_reg_x(p.filename_reg, 11);
        a.b_cond(Cond::Cs, end);

        // x11 = filename string pointer.
        if p.is_single_char_ptr {
            a.mov_reg_x(11, p.filename_reg);
        } else {
            a.ldr_x_uoff(11, p.filename_reg, 0);
        }

        // x12 -> embedded key (PC-relative back to offset 0).
        let key_off = a.offset() as i32;
        a.adr(12, -key_off);

        // strcmp(filename, key).
        a.bind(cycle);
        a.ldrb_w_post(14, 11, 1);
        a.ldrb_w_post(15, 12, 1);
        a.cmp_reg_w(14, 15);
        a.b_cond(Cond::Ne, end);
        a.cbnz_w(15, cycle);

        // --- match: grant root ------------------------------------------------
        base.emit_get_current(&mut a, 12); // x12 = current task
        a.ldr_x_uoff(14, 12, cred_offset as u32); // x14 = cred
        a.add_imm_x(14, 14, ic.atomic_usage_size as u32); // skip usage counter
        // zero the 32-byte uid/gid block.
        for _ in 0..4 {
            a.str_x_post(ZR, 14, 8);
        }
        // zero securebits.
        a.str_w_post(ZR, 14, ic.securebits_size as i32);
        // write the full capability set.
        a.mov_imm_x_asmjit(13, ic.cap_ability_max);
        a.stp_x_post(13, 13, 14, 16);
        a.stp_x_post(13, 13, 14, 16);
        if cap_cnt >= 5 {
            a.str_x_post(13, 14, 8);
        }

        // clear TIF_SECCOMP in thread_info.flags (atomically).
        a.mov_imm_x_asmjit(15, 1u64 << TIF_SECCOMP);
        let flags_reg = match base.current_mode() {
            CurrentMode::SpEl0IsTask => 12,
            CurrentMode::SpEl0IsThreadInfo => {
                a.mrs_sp_el0(13);
                base.emit_huawei_kti_add(&mut a, 13);
                13
            }
            CurrentMode::SpMask => {
                a.mov_x_sp(13);
                a.and_imm_x(13, 13, !(THREAD_SIZE - 1));
                13
            }
        };
        let _ = THREAD_INFO_FLAGS; // flags at offset 0 → ldaxr/stlxr base only
        // Single-shot ldaxr/bic/stlxr, as upstream. The exclusive store's status
        // (w15) is intentionally ignored: the unconditional `seccomp.mode = 0`
        // below is the real disable (SECCOMP_MODE_DISABLED makes the syscall
        // path allow regardless of TIF_SECCOMP), so a rare failed store here is
        // benign and a retry loop is unnecessary.
        a.ldaxr_x(14, flags_reg);
        a.bic_reg_x(14, 14, 15);
        a.stlxr_w_x(15, 14, flags_reg);

        // zero seccomp.mode (the actual seccomp disable).
        a.str_w_uoff(ZR, 12, seccomp_offset as u32);

        // --- end: jump back past the overwritten instruction ------------------
        a.bind(end);
        let b_pos = a.offset() as i64;
        a.b_off((jump_back as i64 - (hook_addr as i64 + b_pos)) as i32);

        let mut bytes = match a.to_bytes() {
            Ok(b) if !b.is_empty() => b,
            _ => return 0,
        };

        // Preserve the original instruction: splice the 4 bytes at do_execve_addr
        // into the placeholder slot at offset ROOT_KEY_LEN.
        let i = p.do_execve_addr as usize;
        if i + 4 > base.buf().len() {
            return 0;
        }
        bytes[ROOT_KEY_LEN..ROOT_KEY_LEN + 4].copy_from_slice(&base.buf()[i..i + 4]);

        let size = bytes.len();
        if size as u64 > region.size {
            return 0; // not enough kernel space
        }

        out.push(PatchBytes {
            addr: hook_addr,
            bytes,
        });
        // Entry branch: do_execve_addr → stub entry (just past the key).
        base.patch_jump(p.do_execve_addr, hook_addr + ROOT_KEY_LEN as u64, out);
        size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skroot::insn;

    fn region(off: u64, size: u64) -> SymbolRegion {
        SymbolRegion { offset: off, size }
    }

    /// Build a kernel image: a do_execveat_common stub at `exec_off` plus an
    /// `init_cred` pattern near `ic_anchor` so PatchBase constructs.
    fn fake_kernel(exec_off: usize, ic_anchor: usize, len: usize) -> Vec<u8> {
        let mut buf = vec![0u8; len];
        // do_execveat_common body: paciasp; <orig>; ret
        let words = [0xD503_233Fu32, 0xAA01_03E0, 0xD65F_03C0];
        for (k, w) in words.iter().enumerate() {
            buf[exec_off + k * 4..exec_off + k * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        let pat = test_init_cred_pattern();
        buf[ic_anchor - pat.len()..ic_anchor].copy_from_slice(&pat);
        buf
    }

    fn test_init_cred_pattern() -> Vec<u8> {
        // usage8=4, 32 zero, sec8=0, then [0, max, max, max] u64.
        let max: u64 = 0x1FF_FFFF_FFFF;
        let mut p = Vec::new();
        p.extend_from_slice(&4u64.to_le_bytes());
        p.extend_from_slice(&[0u8; 32]);
        p.extend_from_slice(&0u64.to_le_bytes());
        p.extend_from_slice(&0u64.to_le_bytes());
        for _ in 0..3 {
            p.extend_from_slice(&max.to_le_bytes());
        }
        p
    }

    fn base_for(buf: &[u8]) -> PatchBase<'_> {
        PatchBase::new(buf, 8, None).expect("init_cred found")
    }

    fn sym_with_execve(off: u64, size: u64) -> KernelSymbolOffset {
        KernelSymbolOffset {
            do_execveat_common: region(off, size),
            ..Default::default()
        }
    }

    #[test]
    fn emits_stub_with_key_and_preserved_insn() {
        let exec_off = 0x2000usize;
        let region_off = 0x4000usize;
        let buf = fake_kernel(exec_off, 0x100, 0x8000);
        let mut base = base_for(&buf);
        let sym = sym_with_execve(exec_off as u64, 0x40);
        let pde = PatchDoExecve::new(&base, &sym, (6, 1, 0)).expect("execve picked");

        // do_execve_addr skips the paciasp entry.
        assert_eq!(pde.do_execve_addr(), exec_off as u64 + 4);

        let mut out = Vec::new();
        let n = pde.patch_do_execve(
            &mut base,
            &region(region_off as u64, 0x300),
            0x838,
            0x900,
            &mut out,
        );
        assert!(n > ROOT_KEY_LEN, "stub should be larger than the key");

        // Two writes: the stub, then the entry branch.
        assert_eq!(out.len(), 2);
        let stub = &out[0];
        assert_eq!(stub.addr, region_off as u64);
        // First 48 bytes are the key placeholder (all zero).
        assert!(stub.bytes[..ROOT_KEY_LEN].iter().all(|&b| b == 0));
        // The original instruction (orig at exec_off+4 = 0xAA0103E0) is spliced
        // into the slot at offset 48.
        let slot = u32::from_le_bytes(stub.bytes[48..52].try_into().unwrap());
        assert_eq!(slot, 0xAA01_03E0);
        // The stub ends with a branch back.
        let last = u32::from_le_bytes(stub.bytes[n - 4..n].try_into().unwrap());
        assert!(insn::is_b(last));

        // Entry branch overwrites do_execve_addr with a `b` into the stub.
        let entry = &out[1];
        assert_eq!(entry.addr, exec_off as u64 + 4);
        assert!(insn::is_b(u32::from_le_bytes(
            entry.bytes[..4].try_into().unwrap()
        )));
    }

    #[test]
    fn fails_when_region_too_small() {
        let exec_off = 0x2000usize;
        let buf = fake_kernel(exec_off, 0x100, 0x8000);
        let mut base = base_for(&buf);
        let sym = sym_with_execve(exec_off as u64, 0x40);
        let pde = PatchDoExecve::new(&base, &sym, (6, 1, 0)).unwrap();
        let mut out = Vec::new();
        // A region of 16 bytes cannot hold the stub.
        let n = pde.patch_do_execve(&mut base, &region(0x4000, 16), 0x838, 0x900, &mut out);
        assert_eq!(n, 0);
        assert!(out.is_empty());
    }

    #[test]
    fn no_execve_symbol_is_none() {
        let buf = fake_kernel(0x2000, 0x100, 0x8000);
        let base = base_for(&buf);
        let sym = KernelSymbolOffset::default();
        assert!(PatchDoExecve::new(&base, &sym, (6, 1, 0)).is_none());
    }
}
