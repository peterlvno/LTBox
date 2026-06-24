//! Shared patch-emission primitives — port of upstream `patch_base.{h,cpp}`.
//!
//! [`PatchBase`] holds the per-kernel facts the individual patches need (the
//! located `init_cred`, the Huawei KTI address, and how this kernel obtains
//! `current`) and emits the common instruction snippets: fetching `current`,
//! a stack-guarded `bl`, a PAC-aware `ret`, and an unconditional jump patch.
//!
//! Upstream emits with asmjit; here the snippets are built with the in-crate
//! [`asm`] encoder.
#![allow(dead_code)]

use super::asm::{Asm, SP, ZR};
use super::init_cred::{self, InitCred};
use super::insn;
use super::offsets;
use super::patch_bytes::PatchBytes;

/// `THREAD_SIZE` (16 KiB) — used to mask `sp` down to the `thread_info` base.
const THREAD_SIZE: u64 = 0x4000;
/// `offsetof(struct thread_info, task)` — `{ flags, addr_limit, task }`.
const TASK_OFFSET: u32 = 16;
/// A kernel with more `mrs …, sp_el0` sites than this stores `current` via
/// `sp_el0` rather than the legacy SP-mask path.
const MRS_SP_EL0_THRESHOLD: usize = 5000;

/// How this kernel materializes `current`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CurrentMode {
    /// `CONFIG_THREAD_INFO_IN_TASK`: `sp_el0` *is* the `task_struct`.
    SpEl0IsTask,
    /// `sp_el0` is the `thread_info`; load `task` from it.
    SpEl0IsThreadInfo,
    /// Legacy: mask `sp` to the `thread_info`, then load `task`.
    SpMask,
}

/// Per-kernel patch context.
pub struct PatchBase<'a> {
    buf: &'a [u8],
    init_cred: InitCred,
    huawei_kti: Option<u64>,
    current_mode: CurrentMode,
    /// Absolute address the next emitted word will live at, so `adrp` page math
    /// is correct. Set with [`PatchBase::set_kti_calc_base`] before emitting a
    /// routine that may reference the Huawei KTI.
    kti_calc_base: u64,
}

impl<'a> PatchBase<'a> {
    /// Build the context. Returns `None` if `init_cred` could not be located
    /// (upstream aborts here).
    pub fn new(buf: &'a [u8], cred_uid_offset: u64, huawei_kti: Option<u64>) -> Option<Self> {
        let init_cred = init_cred::find_init_cred(buf, cred_uid_offset)?;
        let current_mode = detect_current_mode(buf);
        Some(PatchBase {
            buf,
            init_cred,
            huawei_kti: huawei_kti.filter(|&a| a != 0),
            current_mode,
            kti_calc_base: 0,
        })
    }

    pub fn init_cred(&self) -> &InitCred {
        &self.init_cred
    }

    pub fn is_huawei(&self) -> bool {
        self.huawei_kti.is_some()
    }

    /// Set the absolute file address at which the routine currently being
    /// assembled will be placed (so `adrp` resolves correctly).
    pub fn set_kti_calc_base(&mut self, base: u64) {
        self.kti_calc_base = base;
    }

    /// Skip a leading PAC sign / `bti` at a function entry.
    pub fn skip_pac_bti_at_func_start(&self, addr: u64) -> u64 {
        let i = addr as usize;
        if i + 4 > self.buf.len() {
            return addr;
        }
        let w = rd32(self.buf, i);
        if insn::is_pac_or_bti(w) {
            addr + 4
        } else {
            addr
        }
    }

    /// Emit `b (jump_addr - patch_addr)` as a patch at `patch_addr`. Returns the
    /// number of bytes written, or 0 on failure. A `patch_addr` of 0 (the
    /// unresolved-symbol sentinel) is a no-op, like the other patch helpers.
    pub fn patch_jump(&self, patch_addr: u64, jump_addr: u64, out: &mut Vec<PatchBytes>) -> usize {
        if patch_addr == 0 {
            return 0;
        }
        let mut a = Asm::new();
        a.b_off((jump_addr as i64 - patch_addr as i64) as i32);
        match a.to_bytes() {
            Ok(bytes) if !bytes.is_empty() => {
                let len = bytes.len();
                out.push(PatchBytes {
                    addr: patch_addr,
                    bytes,
                });
                len
            }
            _ => 0,
        }
    }

    /// Emit the sequence that loads `current` (`task_struct *`) into `Xx`.
    pub fn emit_get_current(&self, a: &mut Asm, x: u32) {
        match self.current_mode {
            CurrentMode::SpEl0IsTask => {
                a.mrs_sp_el0(x);
                self.emit_huawei_kti_add(a, x);
            }
            CurrentMode::SpEl0IsThreadInfo => {
                a.mrs_sp_el0(x);
                self.emit_huawei_kti_add(a, x);
                a.ldr_x_uoff(x, x, TASK_OFFSET);
            }
            CurrentMode::SpMask => {
                a.mov_x_sp(x);
                a.and_imm_x(x, x, !(THREAD_SIZE - 1));
                a.ldr_x_uoff(x, x, TASK_OFFSET);
            }
        }
    }

    /// Emit a `bl target` bracketed by an `x29/x30` save/restore so the call
    /// does not clobber the host frame (upstream `emit_safe_bl`).
    pub fn emit_safe_bl(&self, a: &mut Asm, func_base_addr: u64, target: u64) {
        a.stp_x_pre(29, 30, SP, -16);
        let bl_addr = func_base_addr + a.offset() as u64;
        let diff = target as i64 - bl_addr as i64;
        a.bl_off(diff as i32);
        a.ldp_x_post(29, 30, SP, 16);
    }

    /// Emit the correct authenticated `ret` for a function whose entry was
    /// `entry_insn` (so a PAC-signed return address is balanced).
    pub fn emit_ret_by_entry_insn(&self, a: &mut Asm, entry_insn: u32) {
        if insn::is_paciaz(entry_insn) {
            a.autiaz();
        } else if insn::is_paciasp(entry_insn) {
            a.autiasp();
        } else if insn::is_pacibz(entry_insn) {
            a.autibz();
        } else if insn::is_pacibsp(entry_insn) {
            a.autibsp();
        }
        a.ret();
    }

    /// On Huawei kernels, add the KTI randomization base to `Xx` (the pointer
    /// obfuscation Huawei applies to `current`). No-op otherwise.
    fn emit_huawei_kti_add(&self, a: &mut Asm, x: u32) {
        let Some(kti) = self.huawei_kti else {
            return;
        };
        // Scratch must differ from the destination, or the add below would be
        // overwritten and the restore would leave `Xx` unadjusted. Upstream
        // hardcodes x1; pick another when the caller's current-reg is x1.
        let scratch = if x == 1 { 2 } else { 1 };
        // RegProtectGuard(scratch): stp scratch, xzr, [sp, #-16]!
        a.stp_x_pre(scratch, ZR, SP, -16);
        let cur_abs = self.kti_calc_base + a.offset() as u64;
        a.adrp(scratch, cur_abs, kti);
        let lo12 = (kti & 0xFFF) as u32;
        a.ldr_x_uoff(scratch, scratch, lo12);
        a.add_reg_x(x, x, scratch);
        // guard restore: ldp scratch, xzr, [sp], #16
        a.ldp_x_post(scratch, ZR, SP, 16);
    }
}

fn rd32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

/// Count `mrs …, sp_el0` sites across the whole image.
fn count_mrs_sp_el0(buf: &[u8]) -> usize {
    let mut cnt = 0;
    let mut i = 0;
    while i + 4 <= buf.len() {
        if insn::is_mrs_sp_el0(rd32(buf, i)) {
            cnt += 1;
        }
        i += 4;
    }
    cnt
}

/// Decide how the kernel obtains `current` (upstream `is_CONFIG_THREAD_INFO_IN_TASK`
/// / `is_CURRENT_FROM_SP_EL0_THREAD_INFO`).
fn detect_current_mode(buf: &[u8]) -> CurrentMode {
    let mrs_count = count_mrs_sp_el0(buf);
    if mrs_count <= MRS_SP_EL0_THRESHOLD {
        return CurrentMode::SpMask;
    }
    if current_from_sp_el0_thread_info(buf, mrs_count) {
        CurrentMode::SpEl0IsThreadInfo
    } else {
        CurrentMode::SpEl0IsTask
    }
}

/// Fraction-based test: if ≥10% of `mrs sp_el0` sites immediately load
/// `thread_info.task` (offset 16) and then a large field (>0x400), this kernel
/// treats `sp_el0` as `thread_info` rather than the task itself.
fn current_from_sp_el0_thread_info(buf: &[u8], mrs_count: usize) -> bool {
    if mrs_count == 0 {
        return false;
    }
    let mut hits = 0usize;
    let max = buf.len().saturating_sub(4);
    let mut x = 0usize;
    while x < max {
        if !insn::is_mrs_sp_el0(rd32(buf, x)) {
            x += 4;
            continue;
        }
        // Find the next return that closes this function.
        let mut end = 0usize;
        let mut y = x + 4;
        while y < max {
            let w = rd32(buf, y);
            if insn::is_ret(w) || insn::is_retaa(w) || insn::is_retab(w) {
                end = y;
                break;
            }
            y += 4;
        }
        if end != 0 {
            let loads = offsets::current_task_field_loads(buf, x, end);
            if loads.len() >= 2
                && loads[0].1 == TASK_OFFSET as i64
                && loads.iter().any(|&(_, off)| off > 0x400)
            {
                hits += 1;
            }
        }
        x += 4;
    }
    (hits as f32 / mrs_count as f32) >= 0.1
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACIASP: u32 = 0xD503_233F;
    const NOP: u32 = 0xD503_201F;
    const RET: u32 = 0xD65F_03C0;

    fn base_with(buf: &[u8], mode: CurrentMode) -> PatchBase<'_> {
        // Construct directly so tests don't depend on a real init_cred scan.
        PatchBase {
            buf,
            init_cred: InitCred {
                head: vec![0u8; 8],
                atomic_usage_size: 8,
                securebits_size: 8,
                cap_cnt: 5,
                cap_ability_max: 0x1FF_FFFF_FFFF,
                offset: 0,
            },
            huawei_kti: None,
            current_mode: mode,
            kti_calc_base: 0,
        }
    }

    fn words(bytes: &[u8]) -> Vec<u32> {
        bytes
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }

    #[test]
    fn get_current_sp_el0_is_task() {
        let buf = [0u8; 4];
        let pb = base_with(&buf, CurrentMode::SpEl0IsTask);
        let mut a = Asm::new();
        pb.emit_get_current(&mut a, 0);
        let w = words(&a.to_bytes().unwrap());
        assert_eq!(w.len(), 1);
        assert!(insn::is_mrs_sp_el0(w[0]));
    }

    #[test]
    fn get_current_sp_el0_is_thread_info() {
        let buf = [0u8; 4];
        let pb = base_with(&buf, CurrentMode::SpEl0IsThreadInfo);
        let mut a = Asm::new();
        pb.emit_get_current(&mut a, 9);
        let w = words(&a.to_bytes().unwrap());
        // mrs x9, sp_el0 ; ldr x9, [x9, #16]
        assert_eq!(w.len(), 2);
        assert!(insn::is_mrs_sp_el0(w[0]));
        assert_eq!(w[1], 0xF940_0000 | ((16 / 8) << 10) | (9 << 5) | 9);
    }

    #[test]
    fn get_current_sp_mask() {
        let buf = [0u8; 4];
        let pb = base_with(&buf, CurrentMode::SpMask);
        let mut a = Asm::new();
        pb.emit_get_current(&mut a, 1);
        let w = words(&a.to_bytes().unwrap());
        // mov x1, sp ; and x1, x1, #~0x3FFF ; ldr x1, [x1, #16]
        assert_eq!(w.len(), 3);
        assert_eq!(w[0], 0x9100_0000 | (SP << 5) | 1); // add x1, sp, #0
        assert!(insn::is_and_imm(w[1]));
        assert_eq!(w[2], 0xF940_0000 | ((16 / 8) << 10) | (1 << 5) | 1);
    }

    #[test]
    fn safe_bl_brackets_with_stp_ldp() {
        let buf = [0u8; 4];
        let pb = base_with(&buf, CurrentMode::SpEl0IsTask);
        let mut a = Asm::new();
        pb.emit_safe_bl(&mut a, 0x1000, 0x800);
        let w = words(&a.to_bytes().unwrap());
        // stp x29,x30,[sp,#-16]! ; bl <-0x804> ; ldp x29,x30,[sp],#16
        assert_eq!(w.len(), 3);
        assert!(insn::is_stp_pre(w[0]));
        assert!(insn::is_bl(w[1]));
        // bl at 0x1004 → target 0x800 → diff -0x804 → imm26 = -0x201
        let imm = (w[1] & 0x03FF_FFFF) as i32;
        let sext = (imm << 6) >> 6;
        assert_eq!(sext, -0x201);
    }

    #[test]
    fn ret_by_entry_balances_pac() {
        let buf = [0u8; 4];
        let pb = base_with(&buf, CurrentMode::SpEl0IsTask);
        let mut a = Asm::new();
        pb.emit_ret_by_entry_insn(&mut a, PACIASP);
        let w = words(&a.to_bytes().unwrap());
        assert_eq!(w.len(), 2);
        assert_eq!(w[0], 0xD503_23BF); // autiasp
        assert!(insn::is_ret(w[1]));

        let mut a = Asm::new();
        pb.emit_ret_by_entry_insn(&mut a, NOP);
        let w = words(&a.to_bytes().unwrap());
        assert_eq!(w.len(), 1);
        assert!(insn::is_ret(w[0]));
    }

    #[test]
    fn patch_jump_emits_b() {
        let buf = [0u8; 4];
        let pb = base_with(&buf, CurrentMode::SpEl0IsTask);
        let mut out = Vec::new();
        let n = pb.patch_jump(0x100, 0x180, &mut out);
        assert_eq!(n, 4);
        assert_eq!(out[0].addr, 0x100);
        let w = words(&out[0].bytes)[0];
        assert!(insn::is_b(w));

        // patch_addr 0 (unresolved symbol) is a no-op.
        let mut out = Vec::new();
        assert_eq!(pb.patch_jump(0, 0x180, &mut out), 0);
        assert!(out.is_empty());
    }

    #[test]
    fn skip_pac_entry() {
        let buf = [PACIASP.to_le_bytes(), RET.to_le_bytes()].concat();
        let pb = base_with(&buf, CurrentMode::SpEl0IsTask);
        assert_eq!(pb.skip_pac_bti_at_func_start(0), 4);
        assert_eq!(pb.skip_pac_bti_at_func_start(4), 4); // ret is not pac/bti
    }

    #[test]
    fn huawei_kti_scratch_differs_from_dest() {
        let buf = [0u8; 4];
        let mut pb = base_with(&buf, CurrentMode::SpEl0IsTask);
        // calc base and KTI are same-space file offsets, so adrp is in range.
        pb.kti_calc_base = 0x10_0000;
        pb.huawei_kti = Some(0x12_3000);
        // current dest = x1: the helper must use a scratch other than x1, so
        // the final `add x1, x1, scratch` keeps x1's adjusted value.
        let mut a = Asm::new();
        pb.emit_get_current(&mut a, 1);
        let w = words(&a.to_bytes().unwrap());
        // mrs x1 ; stp ; adrp ; ldr ; add x1,x1,x2 ; ldp
        let add = *w
            .iter()
            .find(|&&w| (w & 0xFFE0_FC00) == 0x8B00_0000)
            .unwrap();
        let rd = add & 0x1F;
        let rm = (add >> 16) & 0x1F;
        assert_eq!(rd, 1);
        assert_ne!(rm, 1, "scratch must differ from the destination");
    }
}
