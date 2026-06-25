//! Current-credential allow helper for the SELinux hooks — port of upstream
//! `patch_current_avc_check.{h,cpp}`.
//!
//! The helper returns `x10 = 1` when the current task already has uid/euid 0,
//! securebits 0, and every capability set at or above the located full-cap
//! value. The avc/audit hooks call this and skip denial/logging for that state.
#![allow(dead_code)]

use super::asm::{Asm, Cond, ZR};
use super::patch_base::PatchBase;
use super::patch_bytes::PatchBytes;
use super::symbol_analyze::SymbolRegion;

const CRED_UID_INFO_SIZE: u32 = 32;
const CRED_EUID_OFFSET: u32 = 16;

#[derive(Default)]
pub struct PatchCurrentAvcCheck;

impl PatchCurrentAvcCheck {
    pub fn new() -> Self {
        PatchCurrentAvcCheck
    }

    pub fn patch_current_avc_check_bl_func(
        &self,
        base: &mut PatchBase,
        region: &SymbolRegion,
        task_struct_cred_offset: u64,
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
        let cred_euid_start = ic.atomic_usage_size as u32 + CRED_EUID_OFFSET;

        let mut a = Asm::new();
        let end = a.new_label();
        let cycle_cap = a.new_label();

        a.mov_reg_x(10, ZR);
        base.emit_get_current(&mut a, 11);
        a.ldr_x_uoff(11, 11, task_struct_cred_offset as u32);
        a.ldr_w_uoff(12, 11, cred_euid_start);
        a.cbnz_w(12, end);
        a.add_imm_x(11, 11, ic.atomic_usage_size as u32 + CRED_UID_INFO_SIZE);
        a.ldr_w_post(13, 11, ic.securebits_size as i32);
        a.cbnz_w(13, end);
        a.mov_imm_x_asmjit(12, ic.cap_ability_max);
        a.mov_imm_x_asmjit(13, ic.cap_cnt as u64);
        a.bind(cycle_cap);
        a.ldr_x_post(14, 11, 8);
        a.cmp_reg_x(14, 12);
        a.b_cond(Cond::Cc, end);
        a.subs_imm_x(13, 13, 1);
        a.b_cond(Cond::Ne, cycle_cap);
        a.mov_imm_x_asmjit(10, 1);
        a.bind(end);
        a.ret();

        let bytes = match a.to_bytes() {
            Ok(b) if !b.is_empty() => b,
            _ => return 0,
        };
        let size = bytes.len();
        if size as u64 > region.size {
            return 0;
        }

        out.push(PatchBytes {
            addr: hook_addr,
            bytes,
        });
        size
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skroot::insn;

    const MRS_SP_EL0: u32 = 0xD538_4100;

    fn region(off: u64, size: u64) -> SymbolRegion {
        SymbolRegion { offset: off, size }
    }

    fn fake_kernel_with_init_cred() -> Vec<u8> {
        let mut buf = vec![0u8; 0x9000];
        for i in 0..5001 {
            let off = i * 4;
            buf[off..off + 4].copy_from_slice(&MRS_SP_EL0.to_le_bytes());
        }
        let pat = test_init_cred_pattern();
        let anchor = 0x8800;
        buf[anchor - pat.len()..anchor].copy_from_slice(&pat);
        buf
    }

    fn test_init_cred_pattern() -> Vec<u8> {
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

    #[test]
    fn emits_current_root_check_helper() {
        let buf = fake_kernel_with_init_cred();
        let mut base = PatchBase::new(&buf, 8, None).expect("init_cred found");
        let mut out = Vec::new();

        let n = PatchCurrentAvcCheck::new().patch_current_avc_check_bl_func(
            &mut base,
            &region(0x4000, 0x100),
            0x838,
            &mut out,
        );

        assert!(n > 0);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].addr, 0x4000);

        let words: Vec<u32> = out[0]
            .bytes
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(words[0], 0xAA1F_03EA); // mov x10, xzr
        assert!(insn::is_mrs_sp_el0(words[1]));
        assert!(words.iter().any(|&w| insn::is_cbnz(w)));
        assert!(words.iter().any(|&w| insn::is_bcond(w)));
        assert!(insn::is_ret(*words.last().unwrap()));
    }

    #[test]
    fn fails_when_region_too_small() {
        let buf = fake_kernel_with_init_cred();
        let mut base = PatchBase::new(&buf, 8, None).unwrap();
        let mut out = Vec::new();

        let n = PatchCurrentAvcCheck::new().patch_current_avc_check_bl_func(
            &mut base,
            &region(0x4000, 16),
            0x838,
            &mut out,
        );

        assert_eq!(n, 0);
        assert!(out.is_empty());
    }
}
